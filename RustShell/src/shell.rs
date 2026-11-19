// Shell ligero (modo por defecto) + instalación. Port parcial de
// CppShell/src/shell.cpp.
//
// Barra de tareas acoplada (arriba o abajo, reservando el área de trabajo con
// SHAppBarMessage) con botón Inicio, botones de las ventanas abiertas y reloj, y
// un menú Inicio con acciones (Configuración, Explorador, Administrador de
// tareas, Ejecutar, Bloquear, Reiniciar, Apagar). Todo lo que se ve aquí —alto,
// posición, alineación, ancho de los botones, títulos, reloj (12/24 h,
// segundos, fecha), intervalo de refresco, tamaño del menú y qué elementos
// incluye— sale de `ui::config` y se puede cambiar en caliente desde
// Configuración. --install / --uninstall escriben el valor Shell de Winlogon.
#![allow(non_snake_case, non_upper_case_globals)]

use std::cell::RefCell;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, SYSTEMTIME, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::UI::Shell::{SHAppBarMessage, ABM_NEW, ABM_REMOVE, ABM_SETPOS, ABE_BOTTOM, ABE_TOP, APPBARDATA, ShellExecuteW};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::startmenu;
use crate::ui::config::{self, Id};
use crate::ui::gdip::{Align, Gfx};
use crate::ui::{self, theme, window};

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

/// Ancho que necesita el reloj según lo que muestre.
fn clock_width(c: &config::Config) -> i32 {
    if !c.b(Id::BarraMostrarReloj) {
        return 0;
    }
    let mut w = 70;
    if c.b(Id::BarraSegundos) {
        w += 20;
    }
    if !c.b(Id::BarraReloj24h) {
        w += 22;
    }
    if c.b(Id::BarraFecha) {
        w = w.max(96);
    }
    ui::s(w)
}

fn layout(st: &mut State) {
    let c = config::current();
    let mut rc = er();
    unsafe { GetClientRect(st.hwnd, &mut rc) };
    let (w, h) = (rc.right, rc.bottom);
    let start_w = if c.b(Id::BarraMostrarInicio) { ui::s(52) } else { 0 };
    st.start_rc = RECT { left: 0, top: 0, right: start_w, bottom: h };
    let cw = clock_width(&c);
    st.clock_rc = RECT { left: w - cw, top: 0, right: w, bottom: h };

    let bw = if c.b(Id::BarraMostrarTitulos) {
        ui::s(c.get(Id::BarraAnchoBoton))
    } else {
        ui::s(44)
    };
    let gap = ui::s(6);
    let left_edge = st.start_rc.right + if start_w > 0 { ui::s(8) } else { ui::s(6) };
    let right_edge = st.clock_rc.left - if cw > 0 { ui::s(8) } else { ui::s(6) };
    let n = st.apps.len() as i32;
    let total = (n * bw + (n - 1).max(0) * gap).max(0);
    let mut x = if c.get(Id::BarraAlineacion) == 1 {
        // Centrado en el hueco libre, sin solaparse con el botón Inicio.
        (((left_edge + right_edge) - total) / 2).max(left_edge)
    } else {
        left_edge
    };
    let pad = (h / 8).max(ui::s(4));
    for a in st.apps.iter_mut() {
        a.rect = RECT { left: x, top: pad, right: (x + bw).min(right_edge), bottom: h - pad };
        x += bw + gap;
    }
}

fn clock_text(c: &config::Config, now: &SYSTEMTIME) -> String {
    let (h, suffix) = if c.b(Id::BarraReloj24h) {
        (now.wHour, "")
    } else {
        let h12 = match now.wHour % 12 {
            0 => 12,
            v => v,
        };
        (h12, if now.wHour < 12 { " a. m." } else { " p. m." })
    };
    if c.b(Id::BarraSegundos) {
        format!("{:02}:{:02}:{:02}{}", h, now.wMinute, now.wSecond, suffix)
    } else {
        format!("{:02}:{:02}{}", h, now.wMinute, suffix)
    }
}

fn paint(st: &State) {
    let c = config::current();
    window::paint_buffered(st.hwnd, |g: &Gfx, w, h| {
        let s = theme::style();
        g.clear(s.taskbar_bg);
        g.fill_rect(0, 0, w, h, s.taskbar_bg);
        // botón Inicio
        let sr = st.start_rc;
        if sr.right > sr.left {
            if st.hover == -2 {
                g.rounded((sr.left + ui::s(4)) as f32, (sr.top + ui::s(6)) as f32,
                          (sr.right - sr.left - ui::s(8)) as f32, (sr.bottom - sr.top - ui::s(12)) as f32,
                          ui::sf(6.0), Some(s.task_button_hover), None);
            }
            g.text_in("\u{E700}", sr.left as f32, sr.top as f32, (sr.right - sr.left) as f32,
                      (sr.bottom - sr.top) as f32, 16.0, false, s.accent, Align::Center, Align::Center, true);
        }
        // botones de apps
        let labels = c.b(Id::BarraMostrarTitulos);
        for (i, a) in st.apps.iter().enumerate() {
            let r = a.rect;
            if r.right <= r.left + ui::s(8) {
                continue;
            }
            let bg = if st.hover == i as isize { s.task_button_hover } else { s.task_button };
            g.rounded(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32,
                      ui::sf(6.0), Some(bg), None);
            if labels {
                g.text_in(&a.title, (r.left + ui::s(10)) as f32, r.top as f32,
                          (r.right - r.left - ui::s(16)) as f32, (r.bottom - r.top) as f32,
                          11.5, false, s.text, Align::Near, Align::Center, false);
            } else {
                g.text_in("\u{E737}", r.left as f32, r.top as f32, (r.right - r.left) as f32,
                          (r.bottom - r.top) as f32, 11.0, false, s.text_dim, Align::Center, Align::Center, true);
            }
        }
        // reloj
        if c.b(Id::BarraMostrarReloj) {
            let mut now: SYSTEMTIME = unsafe { std::mem::zeroed() };
            unsafe { GetLocalTime(&mut now) };
            let cr = st.clock_rc;
            let cw = (cr.right - cr.left - ui::s(10)) as f32;
            if c.b(Id::BarraFecha) {
                g.text_in(&clock_text(&c, &now), cr.left as f32, (cr.top + ui::s(5)) as f32, cw,
                          ui::s(18) as f32, 12.5, false, s.text, Align::Center, Align::Near, false);
                g.text_in(&format!("{:02}/{:02}/{:04}", now.wDay, now.wMonth, now.wYear),
                          cr.left as f32, (cr.top + ui::s(24)) as f32, cw, ui::s(16) as f32,
                          10.0, false, s.text_dim, Align::Center, Align::Near, false);
            } else {
                g.text_in(&clock_text(&c, &now), cr.left as f32, cr.top as f32, cw,
                          (cr.bottom - cr.top) as f32, 12.5, false, s.text, Align::Center, Align::Center, false);
            }
        }
    });
}

// ------------------------------------------------------------ menú Inicio
// El menú (estilo Windows 7, con lista de programas y búsqueda) vive en
// `startmenu`; aquí solo quedan el lanzador de la propia aplicación y la
// creación de su ventana.

fn own_exe() -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) };
    String::from_utf16_lossy(&buf[..n as usize])
}

/// Lanza otra instancia de OptimizedShell con los argumentos dados
/// (`--settings`, `--explorer <ruta>`, `--taskmgr`, `--lock`...).
pub fn launch_own(args: &str) {
    let exe = own_exe();
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), ui::wide("open").as_ptr(), ui::wide(&exe).as_ptr(),
                      ui::wide(args).as_ptr(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

fn toggle_menu(st: &State) {
    startmenu::toggle(st.menu, st.hwnd);
}

// ------------------------------------------------------------ colocación
/// Rectángulo que ocupa la barra según la configuración.
fn bar_rect() -> RECT {
    let c = config::current();
    let sw = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let sh = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let bar_h = ui::s(c.get(Id::BarraAlto));
    if c.get(Id::BarraPosicion) == 1 {
        RECT { left: 0, top: 0, right: sw, bottom: bar_h }
    } else {
        RECT { left: 0, top: sh - bar_h, right: sw, bottom: sh }
    }
}

/// (Re)coloca la ventana, actualiza la reserva del área de trabajo y el timer.
fn apply_placement(st: &mut State, first: bool) {
    let c = config::current();
    let r = bar_rect();
    unsafe {
        let mut abd: APPBARDATA = std::mem::zeroed();
        abd.cbSize = std::mem::size_of::<APPBARDATA>() as u32;
        abd.hWnd = st.hwnd;
        if !first {
            SHAppBarMessage(ABM_REMOVE, &mut abd);
        }
        let topmost = if c.b(Id::BarraSiempreVisible) { HWND_TOPMOST } else { HWND_NOTOPMOST };
        SetWindowPos(st.hwnd, topmost, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_SHOWWINDOW);
        if c.b(Id::BarraReservarArea) {
            let mut abd: APPBARDATA = std::mem::zeroed();
            abd.cbSize = std::mem::size_of::<APPBARDATA>() as u32;
            abd.hWnd = st.hwnd;
            abd.uEdge = if c.get(Id::BarraPosicion) == 1 { ABE_TOP } else { ABE_BOTTOM };
            abd.rc = r;
            SHAppBarMessage(ABM_NEW, &mut abd);
            SHAppBarMessage(ABM_SETPOS, &mut abd);
        }
        // El reloj con segundos necesita refresco cada segundo.
        let mut interval = c.get(Id::BarraRefresco) as u32;
        if c.b(Id::BarraMostrarReloj) && c.b(Id::BarraSegundos) {
            interval = interval.min(1000);
        }
        SetTimer(st.hwnd, 1, interval, None);
        window::apply_frame_opaque(st.hwnd, c.dark());
    }
    layout(st);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let st_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if msg == ui::config_changed_msg() {
        ui::reload_config();
        if !st_ptr.is_null() {
            apply_placement(&mut *st_ptr, false);
            ui::apply_perf();
            InvalidateRect(hwnd, std::ptr::null(), 0);
            InvalidateRect((*st_ptr).menu, std::ptr::null(), 0);
        }
        return 0;
    }
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            if !st_ptr.is_null() {
                paint(&*st_ptr);
            }
            0
        }
        WM_DISPLAYCHANGE => {
            if !st_ptr.is_null() {
                apply_placement(&mut *st_ptr, false);
                InvalidateRect(hwnd, std::ptr::null(), 0);
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
        WM_RBUTTONUP => {
            // Botón derecho en la barra: abre Configuración en la página de la
            // barra de tareas.
            launch_own("--settings barra");
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

        startmenu::register(hinst);

        let r = bar_rect();
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class_name.as_ptr(),
            ui::wide("OptimizedShell").as_ptr(),
            WS_POPUP,
            r.left, r.top, r.right - r.left, r.bottom - r.top,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return 1;
        }
        let menu = startmenu::create(hinst);

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
        apply_placement(&mut *raw, true);
        refresh_apps(&mut *raw);
        ShowWindow(hwnd, SW_SHOW);
        ui::apply_perf();

        let code = window::message_loop();
        ui::shutdown();
        code
    }
}

