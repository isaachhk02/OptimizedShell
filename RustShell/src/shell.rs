// Shell ligero (modo por defecto) + instalación. Port parcial de
// CppShell/src/shell.cpp.
//
// Primera iteración: barra de tareas acoplada abajo (reserva el área de trabajo
// con SHAppBarMessage) con botón Inicio, botones de las ventanas abiertas y reloj,
// y un menú Inicio con acciones (Configuración, Explorador, Administrador de
// tareas, Ejecutar, Bloquear, Reiniciar, Apagar). --install / --uninstall escriben
// el valor Shell de Winlogon (shell del usuario actual). El escritorio, el menú
// Inicio con búsqueda de apps, los flyouts (volumen/Wi-Fi/batería) y la bandeja
// del sistema se añadirán en iteraciones siguientes.
#![allow(non_snake_case, non_upper_case_globals)]

use std::cell::RefCell;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, SYSTEMTIME, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Shutdown::{ExitWindowsEx, EWX_REBOOT, EWX_SHUTDOWN, SHTDN_REASON_FLAG_PLANNED};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::UI::Shell::{SHAppBarMessage, ABM_NEW, ABM_REMOVE, ABM_SETPOS, ABE_BOTTOM, APPBARDATA, ShellExecuteW};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, HOT_KEY_MODIFIERS};

use crate::ui::gdip::{Align, Gfx, Image};
use crate::ui::{self, theme, window};

const BAR_H: i32 = 48;

// Modificadores de hotkey.
const MOD_CONTROL: HOT_KEY_MODIFIERS = 0x0002;
const MOD_SHIFT: HOT_KEY_MODIFIERS = 0x0004;
const MOD_WIN: HOT_KEY_MODIFIERS = 0x0008;
// Identificadores de hotkey.
const HK_EXPLORER: i32 = 1;
const HK_RUN: i32 = 2;
const HK_TASKMGR: i32 = 3;

thread_local! {
    static WALLPAPER: RefCell<Option<Image>> = RefCell::new(None);
}

fn load_wallpaper() {
    let mut buf = [0u16; 520];
    let path = unsafe {
        if windows_sys::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
            windows_sys::Win32::UI::WindowsAndMessaging::SPI_GETDESKWALLPAPER,
            buf.len() as u32, buf.as_mut_ptr() as *mut _, 0) != 0 {
            let end = buf.iter().position(|&c| c == 0).unwrap_or(0);
            String::from_utf16_lossy(&buf[..end])
        } else {
            String::new()
        }
    };
    if !path.is_empty() {
        WALLPAPER.with(|w| *w.borrow_mut() = Image::load(&path));
    }
}

unsafe extern "system" fn desktop_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            window::paint_buffered(hwnd, |g: &Gfx, w, h| {
                g.clear(theme::rgbc(12, 14, 20));
                WALLPAPER.with(|wp| {
                    if let Some(img) = wp.borrow().as_ref() {
                        g.draw_image_cover(img, 0, 0, w, h);
                    }
                });
            });
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

struct AppBtn {
    hwnd: HWND,
    title: String,
    rect: RECT,
}

struct State {
    hwnd: HWND,
    apps: Vec<AppBtn>,
    start_rc: RECT,
    clock_rc: RECT,
    hover: isize, // -1 ninguno, -2 inicio, >=0 índice de app
    menu: HWND,
}

thread_local! {
    static ENUM_ACC: RefCell<Vec<AppBtn>> = RefCell::new(Vec::new());
}

fn er() -> RECT {
    RECT { left: 0, top: 0, right: 0, bottom: 0 }
}
fn in_rect(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn from_utf16(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

unsafe extern "system" fn enum_proc(hwnd: HWND, _lp: LPARAM) -> i32 {
    if IsWindowVisible(hwnd) == 0 {
        return 1;
    }
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex & WS_EX_TOOLWINDOW != 0 {
        return 1;
    }
    // Solo ventanas de nivel superior sin dueño.
    if GetWindow(hwnd, GW_OWNER) != std::ptr::null_mut() {
        return 1;
    }
    let len = GetWindowTextLengthW(hwnd);
    if len == 0 {
        return 1;
    }
    let mut buf = vec![0u16; len as usize + 1];
    GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    let title = from_utf16(&buf);
    if title.is_empty() {
        return 1;
    }
    ENUM_ACC.with(|a| a.borrow_mut().push(AppBtn { hwnd, title, rect: er() }));
    1
}

fn refresh_apps(st: &mut State) {
    let own = st.hwnd;
    ENUM_ACC.with(|a| a.borrow_mut().clear());
    unsafe { EnumWindows(Some(enum_proc), 0) };
    let mut apps = ENUM_ACC.with(|a| a.borrow_mut().drain(..).collect::<Vec<_>>());
    apps.retain(|a| a.hwnd != own && a.hwnd != st.menu);
    st.apps = apps;
    layout(st);
}

fn layout(st: &mut State) {
    let mut rc = er();
    unsafe { GetClientRect(st.hwnd, &mut rc) };
    let (w, h) = (rc.right, rc.bottom);
    st.start_rc = RECT { left: 0, top: 0, right: ui::s(52), bottom: h };
    st.clock_rc = RECT { left: w - ui::s(96), top: 0, right: w, bottom: h };
    let mut x = st.start_rc.right + ui::s(8);
    let bw = ui::s(170);
    for a in st.apps.iter_mut() {
        a.rect = RECT { left: x, top: ui::s(6), right: (x + bw).min(st.clock_rc.left - ui::s(8)), bottom: h - ui::s(6) };
        x += bw + ui::s(6);
    }
}

fn paint(st: &State) {
    window::paint_buffered(st.hwnd, |g: &Gfx, w, h| {
        let s = theme::style();
        g.clear(s.taskbar_bg);
        g.fill_rect(0, 0, w, h, s.taskbar_bg);
        // botón Inicio
        let sr = st.start_rc;
        if st.hover == -2 {
            g.rounded((sr.left + ui::s(4)) as f32, (sr.top + ui::s(6)) as f32, (sr.right - sr.left - ui::s(8)) as f32, (sr.bottom - sr.top - ui::s(12)) as f32, ui::sf(6.0), Some(s.task_button_hover), None);
        }
        g.text_in("\u{E700}", sr.left as f32, sr.top as f32, (sr.right - sr.left) as f32, (sr.bottom - sr.top) as f32, 16.0, false, s.accent, Align::Center, Align::Center, true);
        // botones de apps
        for (i, a) in st.apps.iter().enumerate() {
            let r = a.rect;
            if r.right <= r.left {
                continue;
            }
            let bg = if st.hover == i as isize { s.task_button_hover } else { s.task_button };
            g.rounded(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32, ui::sf(6.0), Some(bg), None);
            g.text_in(&a.title, (r.left + ui::s(10)) as f32, r.top as f32, (r.right - r.left - ui::s(16)) as f32, (r.bottom - r.top) as f32, 11.5, false, s.text, Align::Near, Align::Center, false);
        }
        // reloj
        let mut now: SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut now) };
        let cr = st.clock_rc;
        g.text_in(&format!("{:02}:{:02}", now.wHour, now.wMinute), cr.left as f32, (cr.top + ui::s(5)) as f32, (cr.right - cr.left - ui::s(10)) as f32, ui::s(18) as f32, 12.5, false, s.text, Align::Center, Align::Near, false);
        g.text_in(&format!("{:02}/{:02}/{:04}", now.wDay, now.wMonth, now.wYear), cr.left as f32, (cr.top + ui::s(24)) as f32, (cr.right - cr.left - ui::s(10)) as f32, ui::s(16) as f32, 10.0, false, s.text_dim, Align::Center, Align::Near, false);
    });
}

// ------------------------------------------------------------ menú Inicio
const MENU_ITEMS: [(&str, &str); 7] = [
    ("\u{E713}", "Configuración"),
    ("\u{E8B7}", "Explorador de archivos"),
    ("\u{E9D9}", "Administrador de tareas"),
    ("\u{E756}", "Ejecutar..."),
    ("\u{E72E}", "Bloquear"),
    ("\u{E777}", "Reiniciar"),
    ("\u{E7E8}", "Apagar"),
];

thread_local! {
    static MENU_HOVER: RefCell<isize> = RefCell::new(-1);
}

fn own_exe() -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) };
    String::from_utf16_lossy(&buf[..n as usize])
}

fn launch_own(args: &str) {
    let exe = own_exe();
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), ui::wide("open").as_ptr(), ui::wide(&exe).as_ptr(),
                      ui::wide(args).as_ptr(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

fn menu_action(i: usize) {
    match i {
        0 => launch_own("--settings"),
        1 => launch_own("--explorer"),
        2 => launch_own("--taskmgr"),
        3 => unsafe {
            ShellExecuteW(std::ptr::null_mut(), ui::wide("open").as_ptr(), ui::wide("explorer.exe").as_ptr(),
                          ui::wide("shell:::{2559a1f3-21d7-11d4-bdaf-00c04f60b9f0}").as_ptr(), std::ptr::null(), SW_SHOWNORMAL);
        },
        4 => launch_own("--lock"),
        5 => unsafe { ExitWindowsEx(EWX_REBOOT, SHTDN_REASON_FLAG_PLANNED); },
        6 => unsafe { ExitWindowsEx(EWX_SHUTDOWN, SHTDN_REASON_FLAG_PLANNED); },
        _ => {}
    }
}

unsafe extern "system" fn menu_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let item_h = ui::s(40);
            window::paint_buffered(hwnd, |g: &Gfx, w, h| {
                let s = theme::style();
                g.clear(theme::argb(0, 0, 0, 0));
                g.rounded(0.0, 0.0, w as f32, h as f32, ui::sf(10.0), Some(s.menu_bg), Some((s.menu_border, 1.0)));
                let hov = MENU_HOVER.with(|m| *m.borrow());
                for (i, (glyph, label)) in MENU_ITEMS.iter().enumerate() {
                    let y = ui::s(8) + i as i32 * item_h;
                    if hov == i as isize {
                        g.rounded(ui::s(6) as f32, y as f32, (w - ui::s(12)) as f32, item_h as f32, ui::sf(6.0), Some(s.menu_hover), None);
                    }
                    g.text_in(glyph, ui::s(16) as f32, y as f32, ui::s(24) as f32, item_h as f32, 13.0, false, s.accent, Align::Center, Align::Center, true);
                    g.text_in(label, ui::s(48) as f32, y as f32, (w - ui::s(56)) as f32, item_h as f32, 13.0, false, s.text, Align::Near, Align::Center, false);
                }
            });
            0
        }
        WM_MOUSEMOVE => {
            let y = ((lp >> 16) & 0xFFFF) as i16 as i32;
            let i = (y - ui::s(8)) / ui::s(40);
            let v = if i >= 0 && (i as usize) < MENU_ITEMS.len() { i as isize } else { -1 };
            MENU_HOVER.with(|m| *m.borrow_mut() = v);
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_LBUTTONDOWN => {
            let y = ((lp >> 16) & 0xFFFF) as i16 as i32;
            let i = (y - ui::s(8)) / ui::s(40);
            ShowWindow(hwnd, SW_HIDE);
            if i >= 0 && (i as usize) < MENU_ITEMS.len() {
                menu_action(i as usize);
            }
            0
        }
        WM_ACTIVATE => {
            if (wp & 0xFFFF) == WA_INACTIVE as usize {
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn toggle_menu(st: &State) {
    unsafe {
        if IsWindowVisible(st.menu) != 0 {
            ShowWindow(st.menu, SW_HIDE);
            return;
        }
        let mut br = er();
        GetWindowRect(st.hwnd, &mut br);
        let w = ui::s(300);
        let h = ui::s(8) * 2 + MENU_ITEMS.len() as i32 * ui::s(40);
        let x = br.left + ui::s(8);
        let y = br.top - h - ui::s(8);
        SetWindowPos(st.menu, HWND_TOPMOST, x, y, w, h, SWP_SHOWWINDOW);
        SetForegroundWindow(st.menu);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let st_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            if !st_ptr.is_null() {
                paint(&*st_ptr);
            }
            0
        }
        WM_TIMER => {
            if !st_ptr.is_null() {
                refresh_apps(&mut *st_ptr);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_MOUSEMOVE => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                let mut hov: isize = -1;
                if in_rect(&st.start_rc, x, y) {
                    hov = -2;
                } else {
                    for (i, a) in st.apps.iter().enumerate() {
                        if in_rect(&a.rect, x, y) {
                            hov = i as isize;
                            break;
                        }
                    }
                }
                if hov != st.hover {
                    st.hover = hov;
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                if in_rect(&st.start_rc, x, y) {
                    toggle_menu(st);
                    return 0;
                }
                for a in &st.apps {
                    if in_rect(&a.rect, x, y) {
                        if IsIconic(a.hwnd) != 0 {
                            ShowWindow(a.hwnd, SW_RESTORE);
                        }
                        SetForegroundWindow(a.hwnd);
                        break;
                    }
                }
            }
            0
        }
        WM_HOTKEY => {
            match wp as i32 {
                HK_EXPLORER => launch_own("--explorer"),
                HK_RUN => menu_action(3),
                HK_TASKMGR => launch_own("--taskmgr"),
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            // libera la reserva del área de trabajo
            let mut abd: APPBARDATA = std::mem::zeroed();
            abd.cbSize = std::mem::size_of::<APPBARDATA>() as u32;
            abd.hWnd = hwnd;
            SHAppBarMessage(ABM_REMOVE, &mut abd);
            if !st_ptr.is_null() {
                drop(Box::from_raw(st_ptr));
            }
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ------------------------------------------------------------ instalación
pub fn install(enable: bool) -> i32 {
    const SUB: &str = "Software\\Microsoft\\Windows NT\\CurrentVersion\\Winlogon";
    unsafe {
        let mut k: HKEY = std::ptr::null_mut();
        if RegCreateKeyExW(HKEY_CURRENT_USER, ui::wide(SUB).as_ptr(), 0, std::ptr::null(), 0,
                           KEY_SET_VALUE, std::ptr::null(), &mut k, std::ptr::null_mut()) != 0 {
            return 1;
        }
        let r = if enable {
            let exe = ui::wide(&own_exe());
            RegSetValueExW(k, ui::wide("Shell").as_ptr(), 0, REG_SZ, exe.as_ptr() as *const u8, (exe.len() * 2) as u32)
        } else {
            let def = ui::wide("explorer.exe");
            RegSetValueExW(k, ui::wide("Shell").as_ptr(), 0, REG_SZ, def.as_ptr() as *const u8, (def.len() * 2) as u32)
        };
        RegCloseKey(k);
        let msg = if enable {
            "OptimizedShell se ha establecido como shell del usuario actual.\nCierra sesión para aplicarlo."
        } else {
            "Se ha restaurado el Explorador de Windows como shell.\nCierra sesión para aplicarlo."
        };
        MessageBoxW(std::ptr::null_mut(), ui::wide(msg).as_ptr(), ui::wide("OptimizedShell").as_ptr(), MB_ICONINFORMATION);
        if r != 0 { 1 } else { 0 }
    }
}

pub fn run() -> i32 {
    unsafe {
        let hinst = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null());
        ui::init(hinst);

        let class_name = ui::wide("OShellRustTaskbar");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        RegisterClassW(&wc);

        let menu_class = ui::wide("OShellRustStartMenu");
        let wcm = WNDCLASSW { lpfnWndProc: Some(menu_proc), lpszClassName: menu_class.as_ptr(), hInstance: hinst,
            style: 0, cbClsExtra: 0, cbWndExtra: 0, hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW), hbrBackground: std::ptr::null_mut(), lpszMenuName: std::ptr::null() };
        RegisterClassW(&wcm);

        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        let bar_h = ui::s(BAR_H);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class_name.as_ptr(),
            ui::wide("OptimizedShell").as_ptr(),
            WS_POPUP,
            0, sh - bar_h, sw, bar_h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return 1;
        }
        let menu = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            menu_class.as_ptr(),
            ui::wide("Inicio").as_ptr(),
            WS_POPUP,
            0, 0, ui::s(300), ui::s(300),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );

        // ventana de escritorio con el fondo, al fondo del z-order
        let desk_class = ui::wide("OShellRustDesktop");
        let wcd = WNDCLASSW { lpfnWndProc: Some(desktop_proc), lpszClassName: desk_class.as_ptr(), hInstance: hinst,
            style: CS_HREDRAW | CS_VREDRAW, cbClsExtra: 0, cbWndExtra: 0, hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW), hbrBackground: std::ptr::null_mut(), lpszMenuName: std::ptr::null() };
        RegisterClassW(&wcd);
        load_wallpaper();
        let desktop = CreateWindowExW(
            WS_EX_NOACTIVATE,
            desk_class.as_ptr(),
            ui::wide("Escritorio").as_ptr(),
            WS_POPUP,
            0, 0, sw, sh - bar_h,
            std::ptr::null_mut(), std::ptr::null_mut(), hinst, std::ptr::null(),
        );
        if !desktop.is_null() {
            SetWindowPos(desktop, HWND_BOTTOM, 0, 0, sw, sh - bar_h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }

        // hotkeys globales
        RegisterHotKey(hwnd, HK_EXPLORER, MOD_WIN, b'E' as u32);
        RegisterHotKey(hwnd, HK_RUN, MOD_WIN, b'R' as u32);
        RegisterHotKey(hwnd, HK_TASKMGR, MOD_CONTROL | MOD_SHIFT, 0x1B); // Ctrl+Shift+Esc

        // reserva el área de trabajo (la barra queda siempre visible)
        let mut abd: APPBARDATA = std::mem::zeroed();
        abd.cbSize = std::mem::size_of::<APPBARDATA>() as u32;
        abd.hWnd = hwnd;
        abd.uEdge = ABE_BOTTOM;
        abd.rc = RECT { left: 0, top: sh - bar_h, right: sw, bottom: sh };
        SHAppBarMessage(ABM_NEW, &mut abd);
        SHAppBarMessage(ABM_SETPOS, &mut abd);

        let st = Box::new(State {
            hwnd,
            apps: Vec::new(),
            start_rc: er(),
            clock_rc: er(),
            hover: -1,
            menu,
        });
        let raw = Box::into_raw(st);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
        window::apply_frame_opaque(hwnd, theme::current().dark);
        layout(&mut *raw);
        refresh_apps(&mut *raw);
        ShowWindow(hwnd, SW_SHOW);
        SetTimer(hwnd, 1, 1500, None);
        ui::set_process_eco(true);
        ui::trim_working_set();

        let code = window::message_loop();
        ui::shutdown();
        code
    }
}
