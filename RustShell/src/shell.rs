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
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::{FindFirstFileW, FindNextFileW, FindClose, WIN32_FIND_DATAW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN};
use windows_sys::Win32::UI::Shell::{SHAppBarMessage, ABM_NEW, ABM_REMOVE, ABM_SETPOS, ABE_BOTTOM, APPBARDATA, ShellExecuteW, SHGetFolderPathW, SHGetKnownFolderItem, FOLDERID_AppsFolder, BHID_EnumItems, KF_FLAG_DEFAULT, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING};
use windows_sys::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED};
use windows_sys::core::GUID;
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

// Alto de cada fila del menú y margen superior.
const MENU_W: i32 = 320;
const MENU_ITEM_H: i32 = 36;
const MENU_PAD: i32 = 8;

// Una entrada del menú Inicio: una acción del sistema, un separador o una app.
enum Entry {
    Action { glyph: &'static str, label: &'static str, action: usize },
    Sep(&'static str),
    App { name: String, path: String },
}

thread_local! {
    static MENU_HOVER: RefCell<isize> = RefCell::new(-1);
    static MENU_ENTRIES: RefCell<Vec<Entry>> = RefCell::new(Vec::new());
    static MENU_SCROLL: RefCell<i32> = RefCell::new(0);
}

// Carpetas conocidas del menú Inicio (usuario + común).
const CSIDL_PROGRAMS: i32 = 0x0002;
const CSIDL_COMMON_PROGRAMS: i32 = 0x0017;

fn known_folder(csidl: i32) -> Option<String> {
    let mut buf = [0u16; 260];
    unsafe {
        if SHGetFolderPathW(std::ptr::null_mut(), csidl, std::ptr::null_mut(), 0, buf.as_mut_ptr()) == 0 {
            let p = from_utf16(&buf);
            if !p.is_empty() { return Some(p); }
        }
    }
    None
}

fn scan_dir(dir: &str, out: &mut Vec<(String, String)>, depth: u32) {
    if depth > 5 {
        return;
    }
    let pattern = format!("{}\\*", dir);
    let mut fd: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let h = unsafe { FindFirstFileW(ui::wide(&pattern).as_ptr(), &mut fd) };
    if h == INVALID_HANDLE_VALUE {
        return;
    }
    loop {
        let name = from_utf16(&fd.cFileName);
        if name != "." && name != ".." && (fd.dwFileAttributes & FILE_ATTRIBUTE_HIDDEN) == 0 {
            let full = format!("{}\\{}", dir, name);
            if fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                scan_dir(&full, out, depth + 1);
            } else {
                let lower = name.to_ascii_lowercase();
                if lower.ends_with(".lnk") || lower.ends_with(".url") {
                    // la extensión es ASCII de 4 bytes, el corte es seguro
                    let disp = name[..name.len() - 4].to_string();
                    out.push((disp, full));
                }
            }
        }
        if unsafe { FindNextFileW(h, &mut fd) } == 0 {
            break;
        }
    }
    unsafe { FindClose(h) };
}

// --------------------------- enumeración de shell:AppsFolder (apps Store + escritorio)
// windows-sys no define las interfaces COM, solo funciones/constantes, así que se
// declaran aquí las vtablas mínimas que se necesitan.
type PFN = unsafe extern "system" fn();

#[repr(C)]
struct IUnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut core::ffi::c_void, *const GUID, *mut *mut core::ffi::c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32,
    release: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32,
}

#[repr(C)]
struct IShellItemVtbl {
    base: IUnknownVtbl,
    bind_to_handler: unsafe extern "system" fn(*mut core::ffi::c_void, *mut core::ffi::c_void, *const GUID, *const GUID, *mut *mut core::ffi::c_void) -> i32,
    get_parent: PFN,
    get_display_name: unsafe extern "system" fn(*mut core::ffi::c_void, i32, *mut *mut u16) -> i32,
    get_attributes: PFN,
    compare: PFN,
}

#[repr(C)]
struct IEnumShellItemsVtbl {
    base: IUnknownVtbl,
    next: unsafe extern "system" fn(*mut core::ffi::c_void, u32, *mut *mut core::ffi::c_void, *mut u32) -> i32,
    skip: PFN,
    reset: PFN,
    clone: PFN,
}

// IID_IShellItem {43826D1E-E718-42EE-BC55-A1E261C37BFE}
const IID_ISHELLITEM: GUID = GUID::from_u128(0x43826d1e_e718_42ee_bc55_a1e261c37bfe);
// IID_IEnumShellItems {70629033-E363-4A28-A567-0DB78006E6D7}
const IID_IENUMSHELLITEMS: GUID = GUID::from_u128(0x70629033_e363_4a28_a567_0db78006e6d7);

unsafe fn vtbl<'a, T>(p: *mut core::ffi::c_void) -> &'a T {
    &**(p as *const *const T)
}

fn from_pwstr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe {
        let mut len = 0usize;
        while *p.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    }
}

// Enumera FOLDERID_AppsFolder: apps de la Store (UWP) y de escritorio en una sola
// lista. Devuelve (nombre visible, destino de lanzamiento "shell:AppsFolder\\<parse>").
fn enum_apps_folder() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    unsafe {
        let mut folder: *mut core::ffi::c_void = std::ptr::null_mut();
        if SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, std::ptr::null_mut(),
                                &IID_ISHELLITEM, &mut folder) != 0 || folder.is_null() {
            return out;
        }
        let fv: &IShellItemVtbl = vtbl(folder);
        let mut en: *mut core::ffi::c_void = std::ptr::null_mut();
        let hr = (fv.bind_to_handler)(folder, std::ptr::null_mut(), &BHID_EnumItems, &IID_IENUMSHELLITEMS, &mut en);
        if hr == 0 && !en.is_null() {
            let ev: &IEnumShellItemsVtbl = vtbl(en);
            loop {
                let mut item: *mut core::ffi::c_void = std::ptr::null_mut();
                let mut fetched: u32 = 0;
                if (ev.next)(en, 1, &mut item, &mut fetched) != 0 || fetched == 0 || item.is_null() {
                    break;
                }
                let iv: &IShellItemVtbl = vtbl(item);
                let mut name_p: *mut u16 = std::ptr::null_mut();
                let mut parse_p: *mut u16 = std::ptr::null_mut();
                if (iv.get_display_name)(item, SIGDN_NORMALDISPLAY, &mut name_p) == 0
                    && (iv.get_display_name)(item, SIGDN_PARENTRELATIVEPARSING, &mut parse_p) == 0
                {
                    let name = from_pwstr(name_p);
                    let parse = from_pwstr(parse_p);
                    let lower = name.to_lowercase();
                    let skip = lower.starts_with("uninstall")
                        || lower.starts_with("desinstalar")
                        || parse.to_lowercase().starts_with("http")
                        || name.is_empty();
                    if !skip {
                        out.push((name, format!("shell:AppsFolder\\{}", parse)));
                    }
                }
                if !name_p.is_null() { CoTaskMemFree(name_p as *const _); }
                if !parse_p.is_null() { CoTaskMemFree(parse_p as *const _); }
                (iv.base.release)(item);
            }
            (ev.base.release)(en);
        }
        (fv.base.release)(folder);
    }
    out
}

// Busca las aplicaciones instaladas, ordenadas alfabéticamente y sin duplicados.
// Prefiere shell:AppsFolder (incluye apps de la Store); si no hay ninguna, recurre
// a escanear los accesos directos del menú Inicio.
fn scan_apps() -> Vec<(String, String)> {
    let mut out = enum_apps_folder();
    if out.is_empty() {
        for csidl in [CSIDL_PROGRAMS, CSIDL_COMMON_PROGRAMS] {
            if let Some(dir) = known_folder(csidl) {
                scan_dir(&dir, &mut out, 0);
            }
        }
    }
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    out.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
    out
}

fn rebuild_entries() {
    let mut v: Vec<Entry> = Vec::new();
    for (i, (glyph, label)) in MENU_ITEMS.iter().enumerate() {
        v.push(Entry::Action { glyph, label, action: i });
    }
    v.push(Entry::Sep("Aplicaciones"));
    for (name, path) in scan_apps() {
        v.push(Entry::App { name, path });
    }
    MENU_ENTRIES.with(|e| *e.borrow_mut() = v);
    MENU_SCROLL.with(|s| *s.borrow_mut() = 0);
}

fn menu_content_h() -> i32 {
    let n = MENU_ENTRIES.with(|e| e.borrow().len()) as i32;
    MENU_PAD * 2 + n * ui::s(MENU_ITEM_H)
}

// Índice de la entrada bajo la coordenada vertical (en cliente), o -1.
fn menu_hit(y: i32) -> isize {
    let scroll = MENU_SCROLL.with(|s| *s.borrow());
    let i = (y - ui::s(MENU_PAD) + scroll) / ui::s(MENU_ITEM_H);
    let n = MENU_ENTRIES.with(|e| e.borrow().len()) as i32;
    if i >= 0 && i < n { i as isize } else { -1 }
}

fn launch_path(path: &str) {
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), ui::wide("open").as_ptr(), ui::wide(path).as_ptr(),
                      std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
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
            let item_h = ui::s(MENU_ITEM_H);
            let pad = ui::s(MENU_PAD);
            window::paint_buffered(hwnd, |g: &Gfx, w, h| {
                let s = theme::style();
                g.clear(theme::argb(0, 0, 0, 0));
                g.rounded(0.0, 0.0, w as f32, h as f32, ui::sf(10.0), Some(s.menu_bg), Some((s.menu_border, 1.0)));
                let hov = MENU_HOVER.with(|m| *m.borrow());
                let scroll = MENU_SCROLL.with(|m| *m.borrow());
                MENU_ENTRIES.with(|e| {
                    for (i, entry) in e.borrow().iter().enumerate() {
                        let y = pad - scroll + i as i32 * item_h;
                        if y + item_h <= pad || y >= h - pad {
                            continue; // fuera de la zona visible
                        }
                        match entry {
                            Entry::Sep(label) => {
                                g.text_in(label, ui::s(16) as f32, y as f32, (w - ui::s(32)) as f32, item_h as f32, 10.5, false, s.text_dim, Align::Near, Align::Center, false);
                            }
                            Entry::Action { glyph, label, .. } => {
                                if hov == i as isize {
                                    g.rounded(ui::s(6) as f32, y as f32, (w - ui::s(12)) as f32, item_h as f32, ui::sf(6.0), Some(s.menu_hover), None);
                                }
                                g.text_in(glyph, ui::s(16) as f32, y as f32, ui::s(24) as f32, item_h as f32, 13.0, false, s.accent, Align::Center, Align::Center, true);
                                g.text_in(label, ui::s(48) as f32, y as f32, (w - ui::s(56)) as f32, item_h as f32, 12.5, false, s.text, Align::Near, Align::Center, false);
                            }
                            Entry::App { name, .. } => {
                                if hov == i as isize {
                                    g.rounded(ui::s(6) as f32, y as f32, (w - ui::s(12)) as f32, item_h as f32, ui::sf(6.0), Some(s.menu_hover), None);
                                }
                                g.text_in("\u{E8B7}", ui::s(16) as f32, y as f32, ui::s(24) as f32, item_h as f32, 12.0, false, s.text_dim, Align::Center, Align::Center, true);
                                g.text_in(name, ui::s(48) as f32, y as f32, (w - ui::s(56)) as f32, item_h as f32, 12.5, false, s.text, Align::Near, Align::Center, false);
                            }
                        }
                    }
                });
            });
            0
        }
        WM_MOUSEMOVE => {
            let y = ((lp >> 16) & 0xFFFF) as i16 as i32;
            let v = match menu_hit(y) {
                i if i >= 0 => {
                    // los separadores no se resaltan
                    let is_sep = MENU_ENTRIES.with(|e| matches!(e.borrow().get(i as usize), Some(Entry::Sep(_))));
                    if is_sep { -1 } else { i }
                }
                _ => -1,
            };
            if MENU_HOVER.with(|m| m.replace(v)) != v {
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp >> 16) & 0xFFFF) as i16 as i32;
            let mut rc = er();
            GetClientRect(hwnd, &mut rc);
            let max = (menu_content_h() - rc.bottom).max(0);
            let step = ui::s(MENU_ITEM_H) * 3;
            MENU_SCROLL.with(|m| {
                let nv = (*m.borrow() - delta / 120 * step).clamp(0, max);
                *m.borrow_mut() = nv;
            });
            MENU_HOVER.with(|m| *m.borrow_mut() = -1);
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_LBUTTONDOWN => {
            let y = ((lp >> 16) & 0xFFFF) as i16 as i32;
            let hit = menu_hit(y);
            if hit < 0 {
                return 0;
            }
            // decide la acción sin mantener el préstamo del RefCell durante el lanzamiento
            enum Act { None, Action(usize), Launch(String) }
            let act = MENU_ENTRIES.with(|e| match e.borrow().get(hit as usize) {
                Some(Entry::Action { action, .. }) => Act::Action(*action),
                Some(Entry::App { path, .. }) => Act::Launch(path.clone()),
                _ => Act::None,
            });
            if matches!(act, Act::None) {
                return 0;
            }
            ShowWindow(hwnd, SW_HIDE);
            match act {
                Act::Action(i) => menu_action(i),
                Act::Launch(p) => launch_path(&p),
                Act::None => {}
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
        rebuild_entries();
        MENU_HOVER.with(|m| *m.borrow_mut() = -1);
        let mut br = er();
        GetWindowRect(st.hwnd, &mut br);
        let w = ui::s(MENU_W);
        // alto deseado por el contenido, limitado al espacio sobre la barra
        let avail = (br.top - ui::s(16)).max(ui::s(200));
        let h = menu_content_h().min(avail);
        let x = br.left + ui::s(8);
        let y = br.top - h - ui::s(8);
        SetWindowPos(st.menu, HWND_TOPMOST, x, y, w, h, SWP_SHOWWINDOW);
        SetForegroundWindow(st.menu);
        InvalidateRect(st.menu, std::ptr::null(), 0);
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
        // COM (STA) para enumerar shell:AppsFolder en el menú Inicio.
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
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
