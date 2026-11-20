// Shell ligero (modo por defecto) + instalación. Port parcial de
// CppShell/src/shell.cpp.
//
// Barra de tareas acoplada (arriba o abajo, reservando el área de trabajo con
// SHAppBarMessage) con botón Inicio —a la izquierda o centrado, con el
// logotipo de Windows sacado de shell32.dll—, botones de las ventanas abiertas
// con su icono real, el área de notificación (volumen, red, batería,
// Bluetooth y campana de avisos) y el reloj.
//
// Todo lo que se ve aquí sale de `ui::config` y se puede cambiar en caliente
// desde Configuración. --install / --uninstall escriben el valor Shell de
// Winlogon.
#![allow(non_snake_case, non_upper_case_globals)]

use std::cell::RefCell;
use std::collections::HashMap;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SYSTEMTIME, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TRACKMOUSEEVENT, TME_LEAVE};
use windows_sys::Win32::UI::Shell::{
    ExtractIconExW, SHAppBarMessage, ShellExecuteW, ABE_BOTTOM, ABE_TOP, ABM_NEW, ABM_REMOVE,
    ABM_SETPOS, APPBARDATA,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::notify;
use crate::startmenu;
use crate::sysinfo::Net;
use crate::tray::{self, Tray};
use crate::ui::config::{self, Id};
use crate::ui::gdip::{Align, Gfx};
use crate::ui::{self, theme, window};

/// Clase de la ventana de la barra. `notify::post` la busca por este nombre
/// para mandarle avisos desde otros procesos.
pub const CLASS: &str = "OShellRustTaskbar";
pub const TIP_CLASS: &str = "OShellRustTip";

struct AppBtn {
    hwnd: HWND,
    title: String,
    rect: RECT,
}

/// Icono anclado a la barra: se dibuja siempre, haya o no ventana abierta.
struct PinBtn {
    /// Lo que se abre al pulsarlo (ejecutable, acceso directo o carpeta).
    path: String,
    label: String,
    /// Ventana del programa si ya está abierto (null si no lo está).
    running: HWND,
    /// Falso cuando el ajuste lo esconde o el programa ya tiene su botón.
    visible: bool,
    rect: RECT,
}

struct State {
    hwnd: HWND,
    apps: Vec<AppBtn>,
    /// Iconos anclados, en el orden de taskbar.ini.
    pins: Vec<PinBtn>,
    start_rc: RECT,
    clock_rc: RECT,
    /// -1 ninguno, -2 inicio, -3..=-7 bandeja (-3 - índice), >=0 índice de app.
    hover: isize,
    menu: HWND,
    quick: HWND,
    notif: notify::Windows,
    tip: HWND,
    tray: Tray,
    desktop: HWND,
    tracking: bool,
    /// Estado anterior, para avisar solo de los cambios.
    last_net_up: Option<bool>,
    last_charging: Option<bool>,
    last_battery_level: i32,
}

/// Codificación del hover de la bandeja (para no usar otro campo).
fn tray_hover(i: usize) -> isize {
    -3 - i as isize
}
fn tray_hover_index(h: isize) -> Option<usize> {
    if h <= -3 && h > PIN_HOVER_BASE {
        Some((-3 - h) as usize)
    } else {
        None
    }
}

/// Los iconos anclados usan el otro extremo del rango, para no pisar a la
/// bandeja (-3 y siguientes) ni al botón Inicio (-2).
const PIN_HOVER_BASE: isize = -1000;
fn pin_hover(i: usize) -> isize {
    PIN_HOVER_BASE - i as isize
}
fn pin_hover_index(h: isize) -> Option<usize> {
    if h <= PIN_HOVER_BASE {
        Some((PIN_HOVER_BASE - h) as usize)
    } else {
        None
    }
}

thread_local! {
    static ENUM_ACC: RefCell<Vec<AppBtn>> = RefCell::new(Vec::new());
    /// Icono por ventana; se vacía al refrescar la lista de ventanas.
    static WIN_ICONS: RefCell<HashMap<isize, isize>> = RefCell::new(HashMap::new());
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
    apps.retain(|a| {
        a.hwnd != own && a.hwnd != st.menu && a.hwnd != st.quick && a.hwnd != st.tip
            && a.hwnd != st.notif.toasts && a.hwnd != st.notif.center && a.hwnd != st.desktop
    });
    // Las ventanas que han desaparecido dejan de ocupar sitio en la caché.
    WIN_ICONS.with(|c| {
        let live: Vec<isize> = apps.iter().map(|a| a.hwnd as isize).collect();
        c.borrow_mut().retain(|k, _| live.contains(k));
    });
    st.apps = apps;
    refresh_pins(st);
    layout(st);
}

/// Ruta del ejecutable dueño de una ventana.
fn window_exe(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return None;
        }
        let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if proc.is_null() {
            return None;
        }
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(proc, 0, buf.as_mut_ptr(), &mut len);
        windows_sys::Win32::Foundation::CloseHandle(proc);
        if ok == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Nombre del archivo de una ruta, en minúsculas.
fn file_name_lower(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Relee taskbar.ini y marca qué anclados tienen ya una ventana abierta: así
/// el icono anclado activa esa ventana en vez de lanzar otra copia.
fn refresh_pins(st: &mut State) {
    let c = config::current();
    let paths = crate::apps::taskbar_pins();
    let show = c.b(Id::BarraAnclados);
    let keep = c.b(Id::BarraAncladosSiempre);
    // Ejecutable de cada ventana de la barra (una sola consulta por ventana).
    let windows: Vec<(HWND, String)> = st
        .apps
        .iter()
        .map(|a| (a.hwnd, window_exe(a.hwnd).unwrap_or_default().to_lowercase()))
        .collect();
    st.pins = paths
        .iter()
        .map(|path| {
            let exe = crate::apps::pin_exe_name(path);
            let full = path.to_lowercase();
            let running = windows
                .iter()
                .find(|(_, w)| !w.is_empty() && (*w == full || file_name_lower(w) == exe))
                .map(|(h, _)| *h)
                .unwrap_or(std::ptr::null_mut());
            PinBtn {
                label: crate::apps::pin_label(path),
                path: path.clone(),
                visible: show && (keep || running.is_null()),
                running,
                rect: er(),
            }
        })
        .collect();
}

/// Pulsar un icono anclado: si el programa ya está abierto se trae su ventana
/// (y se minimiza si ya estaba delante); si no, se lanza.
fn activate_pin(st: &mut State, i: usize) {
    let Some(p) = st.pins.get(i) else { return };
    let win = p.running;
    let path = p.path.clone();
    unsafe {
        if !win.is_null() && IsWindow(win) != 0 {
            if GetForegroundWindow() == win {
                ShowWindow(win, SW_MINIMIZE);
            } else {
                if IsIconic(win) != 0 {
                    ShowWindow(win, SW_RESTORE);
                }
                SetForegroundWindow(win);
            }
            return;
        }
    }
    crate::apps::open_pin(&path);
}

// ------------------------------------------------------------ iconos
/// Icono de una ventana: el que declara ella misma y, si no tiene, el de su
/// ejecutable. Cacheado por ventana (pedirlo en cada repintado sería caro).
fn window_icon(hwnd: HWND) -> HICON {
    let key = hwnd as isize;
    if let Some(h) = WIN_ICONS.with(|c| c.borrow().get(&key).copied()) {
        return h as HICON;
    }
    let icon = resolve_window_icon(hwnd);
    WIN_ICONS.with(|c| c.borrow_mut().insert(key, icon as isize));
    icon
}

fn resolve_window_icon(hwnd: HWND) -> HICON {
    unsafe {
        // ICON_SMALL2 (2) es el que usa el propio shell de Windows.
        for msg in [2usize, 0, 1] {
            let mut res: usize = 0;
            // Con tiempo de espera: una aplicación colgada no puede bloquear la barra.
            SendMessageTimeoutW(hwnd, WM_GETICON, msg, 0, SMTO_ABORTIFHUNG, 120, &mut res);
            if res != 0 {
                return res as HICON;
            }
        }
        for idx in [GCLP_HICONSM, GCLP_HICON] {
            let h = GetClassLongPtrW(hwnd, idx);
            if h != 0 {
                return h as HICON;
            }
        }
        // Último recurso: el icono del ejecutable del proceso.
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return std::ptr::null_mut();
        }
        let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if proc.is_null() {
            return std::ptr::null_mut();
        }
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(proc, 0, buf.as_mut_ptr(), &mut len);
        windows_sys::Win32::Foundation::CloseHandle(proc);
        if ok == 0 {
            return std::ptr::null_mut();
        }
        crate::apps::icon(&String::from_utf16_lossy(&buf[..len as usize]))
    }
}

thread_local! {
    /// Icono del botón Inicio extraído de una DLL del sistema.
    static START_ICON: RefCell<Option<(i32, i32, isize)>> = RefCell::new(None);
}

/// Icono del botón Inicio según la configuración (`None` = dibujarlo).
fn start_icon() -> HICON {
    let c = config::current();
    let which = c.get(Id::BarraIconoInicio);
    let index = c.get(Id::BarraIconoIndice);
    if which != 1 && which != 2 {
        return std::ptr::null_mut();
    }
    START_ICON.with(|cell| {
        let mut cell = cell.borrow_mut();
        if let Some((w, i, h)) = *cell {
            if w == which && i == index {
                return h as HICON;
            }
        }
        // Con la ruta completa: ExtractIconEx no siempre resuelve el nombre
        // suelto según cuál sea el directorio de trabajo del proceso.
        let dll = if which == 1 { "shell32.dll" } else { "imageres.dll" };
        let mut sysdir = [0u16; 260];
        let n = unsafe {
            windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
                sysdir.as_mut_ptr(), sysdir.len() as u32)
        };
        let path = if n > 0 {
            format!("{}\\{}", String::from_utf16_lossy(&sysdir[..n as usize]), dll)
        } else {
            dll.to_string()
        };
        let mut large: HICON = std::ptr::null_mut();
        let mut small: HICON = std::ptr::null_mut();
        unsafe {
            ExtractIconExW(ui::wide(&path).as_ptr(), index, &mut large, &mut small, 1);
            if !large.is_null() && !small.is_null() {
                // Se queda el grande (se escala mejor); el otro se libera.
                DestroyIcon(small);
            }
        }
        let h = if !large.is_null() { large } else { small };
        *cell = Some((which, index, h as isize));
        h
    })
}

/// Logotipo de Windows dibujado con el color de acento (opción «Dibujado» y
/// reserva si la DLL no da el icono).
fn draw_logo(g: &Gfx, s: &theme::Style, cx: i32, cy: i32, size: i32) {
    let gap = (size as f32 * 0.12).max(1.0);
    let q = (size as f32 - gap) / 2.0;
    let x = cx as f32 - size as f32 / 2.0;
    let y = cy as f32 - size as f32 / 2.0;
    for (dx, dy) in [(0.0, 0.0), (q + gap, 0.0), (0.0, q + gap), (q + gap, q + gap)] {
        g.rounded(x + dx, y + dy, q, q, ui::sf(1.5), Some(s.accent), None);
    }
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

    // Derecha: reloj y, a su izquierda, el área de notificación.
    let cw = clock_width(&c);
    st.clock_rc = RECT { left: w - cw, top: 0, right: w, bottom: h };
    let tray_left = st.tray.layout(st.clock_rc.left, h);

    let start_w = if c.b(Id::BarraMostrarInicio) { ui::s(52) } else { 0 };
    let bw = if c.b(Id::BarraMostrarTitulos) {
        ui::s(c.get(Id::BarraAnchoBoton))
    } else {
        ui::s(44)
    };
    let gap = ui::s(6);
    let n = st.apps.len() as i32;
    let apps_w = (n * bw + (n - 1).max(0) * gap).max(0);
    // Los iconos anclados van entre Inicio y los botones de las ventanas.
    let pin_w = ui::s(44);
    let m = st.pins.iter().filter(|p| p.visible).count() as i32;
    let pins_w = (m * pin_w + (m - 1).max(0) * gap).max(0);

    let left_margin = ui::s(6);
    let right_edge = tray_left - if tray_left < st.clock_rc.left { ui::s(4) } else { ui::s(6) };
    let centered_start = c.b(Id::BarraInicioCentro);
    let sep = ui::s(8);

    // Ancho del bloque Inicio + anclados + ventanas, con separación solo entre
    // las partes que existen.
    let mut group = start_w;
    if pins_w > 0 {
        group += if group > 0 { sep } else { 0 } + pins_w;
    }
    if apps_w > 0 {
        group += if group > 0 { sep } else { 0 } + apps_w;
    }

    let (start_x, mut x) = if centered_start {
        // Inicio, anclados y botones forman un bloque centrado (Windows 11).
        let gx = (((left_margin + right_edge) - group) / 2).max(left_margin);
        (gx, gx + start_w + if start_w > 0 && (pins_w > 0 || apps_w > 0) { sep } else { 0 })
    } else {
        let gx = 0;
        let first = start_w + if start_w > 0 { sep } else { left_margin };
        let bloque = pins_w + if pins_w > 0 && apps_w > 0 { sep } else { 0 } + apps_w;
        let ax = if c.get(Id::BarraAlineacion) == 1 {
            (((first + right_edge) - bloque) / 2).max(first)
        } else {
            first
        };
        (gx, ax)
    };
    st.start_rc = RECT { left: start_x, top: 0, right: start_x + start_w, bottom: h };

    let pad = (h / 8).max(ui::s(4));
    for p in st.pins.iter_mut() {
        if !p.visible {
            p.rect = er();
            continue;
        }
        p.rect = RECT { left: x, top: pad, right: (x + pin_w).min(right_edge), bottom: h - pad };
        x += pin_w + gap;
    }
    if pins_w > 0 && apps_w > 0 {
        x += sep - gap;
    }
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
        // ---- botón Inicio
        let sr = st.start_rc;
        if sr.right > sr.left {
            if st.hover == -2 {
                g.rounded((sr.left + ui::s(4)) as f32, (sr.top + ui::s(6)) as f32,
                          (sr.right - sr.left - ui::s(8)) as f32, (sr.bottom - sr.top - ui::s(12)) as f32,
                          ui::sf(6.0), Some(s.task_button_hover), None);
            }
            let cx = (sr.left + sr.right) / 2;
            let cy = (sr.top + sr.bottom) / 2;
            let size = ui::s(20);
            match c.get(Id::BarraIconoInicio) {
                1 | 2 => {
                    let icon = start_icon();
                    if icon.is_null() {
                        draw_logo(g, &s, cx, cy, size);
                    } else {
                        g.draw_hicon(icon, cx - size / 2, cy - size / 2, size);
                    }
                }
                3 => draw_logo(g, &s, cx, cy, size),
                _ => g.text_in("\u{E700}", sr.left as f32, sr.top as f32, (sr.right - sr.left) as f32,
                               (sr.bottom - sr.top) as f32, 16.0, false, s.accent, Align::Center,
                               Align::Center, true),
            }
        }
        // ---- iconos anclados
        let pin_isz = ui::s(c.get(Id::BarraAncladosTamano));
        for (i, p) in st.pins.iter().enumerate() {
            if !p.visible {
                continue;
            }
            let r = p.rect;
            if r.right <= r.left + ui::s(8) {
                continue;
            }
            let running = !p.running.is_null();
            let active = running && unsafe { GetForegroundWindow() } == p.running;
            let bg = if st.hover == pin_hover(i) {
                s.task_button_hover
            } else if active {
                s.task_button_active
            } else {
                s.task_button
            };
            g.rounded(r.left as f32, r.top as f32, (r.right - r.left) as f32,
                      (r.bottom - r.top) as f32, ui::sf(6.0), Some(bg), None);
            let hgt = r.bottom - r.top;
            let icon = crate::apps::icon(&p.path);
            let ix = (r.left + r.right) / 2 - pin_isz / 2;
            if icon.is_null() {
                g.text_in("\u{E718}", r.left as f32, r.top as f32, (r.right - r.left) as f32,
                          hgt as f32, 11.0, false, s.text_dim, Align::Center, Align::Center, true);
            } else {
                g.draw_hicon(icon, ix, r.top + (hgt - pin_isz) / 2, pin_isz);
            }
            // Punto o subrayado: el programa está abierto / en primer plano.
            if running {
                let uw = if active { ((r.right - r.left) / 3).max(ui::s(10)) } else { ui::s(5) };
                let ux = (r.left + r.right) / 2 - uw / 2;
                g.rounded(ux as f32, (r.bottom - ui::s(3)) as f32, uw as f32, ui::s(2) as f32,
                          ui::sf(1.0), Some(s.accent), None);
            }
        }
        // ---- botones de las ventanas
        let labels = c.b(Id::BarraMostrarTitulos);
        let icons = c.b(Id::BarraIconosApps);
        let isz = ui::s(c.get(Id::BarraTamanoIcono));
        for (i, a) in st.apps.iter().enumerate() {
            let r = a.rect;
            if r.right <= r.left + ui::s(8) {
                continue;
            }
            let active = unsafe { GetForegroundWindow() } == a.hwnd;
            let bg = if st.hover == i as isize {
                s.task_button_hover
            } else if active {
                s.task_button_active
            } else {
                s.task_button
            };
            g.rounded(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32,
                      ui::sf(6.0), Some(bg), None);
            if active {
                // Subrayado de la ventana en primer plano.
                let uw = ((r.right - r.left) / 3).max(ui::s(10));
                let ux = (r.left + r.right) / 2 - uw / 2;
                g.rounded(ux as f32, (r.bottom - ui::s(3)) as f32, uw as f32, ui::s(2) as f32,
                          ui::sf(1.0), Some(s.accent), None);
            }
            let hgt = r.bottom - r.top;
            let mut tx = r.left + ui::s(10);
            if icons {
                let icon = window_icon(a.hwnd);
                let ix = if labels { r.left + ui::s(8) } else { (r.left + r.right) / 2 - isz / 2 };
                if icon.is_null() {
                    g.text_in("\u{E737}", ix as f32, r.top as f32, isz as f32, hgt as f32, 11.0, false,
                              s.text_dim, Align::Center, Align::Center, true);
                } else {
                    g.draw_hicon(icon, ix, r.top + (hgt - isz) / 2, isz);
                }
                tx = r.left + ui::s(8) + isz + ui::s(8);
            }
            if labels {
                g.text_in(&a.title, tx as f32, r.top as f32, (r.right - tx - ui::s(8)) as f32, hgt as f32,
                          11.5, false, s.text, Align::Near, Align::Center, false);
            } else if !icons {
                g.text_in("\u{E737}", r.left as f32, r.top as f32, (r.right - r.left) as f32, hgt as f32,
                          11.0, false, s.text_dim, Align::Center, Align::Center, true);
            }
        }
        // ---- área de notificación
        if c.b(Id::BandejaMostrar) {
            if c.b(Id::BandejaSeparador) && !st.tray.items.is_empty() {
                let x = st.tray.items[0].rect.left - ui::s(5);
                g.line(x as f32, ui::s(10) as f32, x as f32, (h - ui::s(10)) as f32, s.separator, 1.0);
            }
            st.tray.paint(g, &s, st.hover);
        }
        // ---- reloj
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

// ------------------------------------------------------------ tooltip
// Ventana de una sola línea que explica el icono bajo el puntero: en la
// bandeja da el estado completo (SSID, carga, volumen) y en los botones de
// ventana el título entero cuando no cabe.
thread_local! {
    static TIP_TEXT: RefCell<String> = RefCell::new(String::new());
}

unsafe extern "system" fn tip_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            window::paint_buffered(hwnd, |g: &Gfx, w, h| {
                let s = theme::style();
                g.clear(theme::argb(0, 0, 0, 0));
                g.rounded(0.0, 0.0, w as f32, h as f32, ui::sf(5.0), Some(s.menu_bg),
                          Some((s.menu_border, 1.0)));
                TIP_TEXT.with(|t| {
                    g.text_in(&t.borrow(), ui::s(9) as f32, 0.0, (w - ui::s(18)) as f32, h as f32, 11.0,
                              false, s.text, Align::Near, Align::Center, false);
                });
            });
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn show_tip(st: &State, text: &str, center_x: i32) {
    if st.tip.is_null() || text.is_empty() {
        return;
    }
    TIP_TEXT.with(|t| *t.borrow_mut() = text.to_string());
    let w = ui::s(18) + (text.chars().count() as i32 * ui::s(7)).clamp(ui::s(40), ui::s(420));
    let h = ui::s(28);
    let bar = bar_rect();
    let sw = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let x = (center_x - w / 2).clamp(ui::s(4), (sw - w - ui::s(4)).max(0));
    let y = if config::current().get(Id::BarraPosicion) == 1 {
        bar.bottom + ui::s(6)
    } else {
        bar.top - h - ui::s(6)
    };
    unsafe {
        SetWindowPos(st.tip, HWND_TOPMOST, x, y.max(0), w, h,
                     SWP_SHOWWINDOW | SWP_NOACTIVATE);
        InvalidateRect(st.tip, std::ptr::null(), 0);
    }
}

fn hide_tip(st: &State) {
    if !st.tip.is_null() {
        unsafe { ShowWindow(st.tip, SW_HIDE) };
    }
}

/// Texto del tooltip para lo que hay bajo el puntero, y su centro en x.
fn tip_for(st: &State) -> Option<(String, i32)> {
    if let Some(i) = tray_hover_index(st.hover) {
        let r = st.tray.items.get(i)?.rect;
        return Some((st.tray.tooltip(i), (r.left + r.right) / 2));
    }
    if let Some(i) = pin_hover_index(st.hover) {
        let p = st.pins.get(i)?;
        let sufijo = if p.running.is_null() { "" } else { " (abierto)" };
        return Some((format!("{}{}", p.label, sufijo), (p.rect.left + p.rect.right) / 2));
    }
    if st.hover == -2 {
        return Some(("Inicio".to_string(), (st.start_rc.left + st.start_rc.right) / 2));
    }
    if st.hover >= 0 {
        let a = st.apps.get(st.hover as usize)?;
        return Some((a.title.clone(), (a.rect.left + a.rect.right) / 2));
    }
    None
}

// ------------------------------------------------------------ avisos del sistema
/// Compara el estado recién muestreado con el anterior y avisa de los cambios
/// que importan: batería baja, enchufado/desenchufado y red caída o vuelta.
fn report_changes(st: &mut State) {
    if !config::current().b(Id::NotifActivadas) {
        return;
    }
    let snap = st.tray.snap.clone();
    let net_up = !matches!(snap.net, Net::Off);
    if let Some(prev) = st.last_net_up {
        if prev != net_up {
            let note = if net_up {
                let detail = match &snap.net {
                    Net::Wifi { ssid, .. } => format!("Conectado a {}.", ssid),
                    _ => "Conexión por cable restablecida.".to_string(),
                };
                notify::Note {
                    app: "Red".into(), title: "Conexión restablecida".into(), body: detail,
                    kind: notify::Kind::Success, time: String::new(), read: false,
                }
            } else {
                notify::Note {
                    app: "Red".into(), title: "Sin conexión".into(),
                    body: "No hay ninguna interfaz de red operativa.".into(),
                    kind: notify::Kind::Warning, time: String::new(), read: false,
                }
            };
            raise(st, note);
        }
    }
    st.last_net_up = Some(net_up);

    let b = snap.battery;
    if b.present {
        if let Some(prev) = st.last_charging {
            if prev != b.charging {
                let note = notify::Note {
                    app: "Energía".into(),
                    title: if b.charging { "Cargando".into() } else { "Con batería".into() },
                    body: if b.percent >= 0 {
                        format!("Carga actual: {} %.", b.percent)
                    } else {
                        String::new()
                    },
                    kind: notify::Kind::Info, time: String::new(), read: false,
                };
                raise(st, note);
            }
        }
        st.last_charging = Some(b.charging);
        // Umbrales de aviso: solo al cruzarlos hacia abajo y sin cargador.
        if !b.charging && b.percent >= 0 {
            for (umbral, kind, title) in [
                (10, notify::Kind::Error, "Batería crítica"),
                (20, notify::Kind::Warning, "Batería baja"),
            ] {
                if b.percent <= umbral && st.last_battery_level > umbral {
                    raise(st, notify::Note {
                        app: "Energía".into(), title: title.into(),
                        body: format!("Queda un {} % de batería. Conecta el cargador.", b.percent),
                        kind, time: String::new(), read: false,
                    });
                    break;
                }
            }
        }
        st.last_battery_level = if b.percent >= 0 { b.percent } else { 100 };
    }
}

/// Entrega un aviso generado dentro del propio shell (sin pasar por WM_COPYDATA).
fn raise(st: &State, mut note: notify::Note) {
    note.time = {
        let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut t) };
        format!("{:02}:{:02}", t.wHour, t.wMinute)
    };
    notify::receive(&st.notif, note);
}

// ------------------------------------------------------------ menú Inicio
/// Ruta completa de este ejecutable.
pub fn own_exe() -> String {
    let mut buf = [0u16; 512];
    let n = unsafe {
        windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(
            std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32)
    };
    String::from_utf16_lossy(&buf[..n as usize])
}

/// Herramientas que tienen ejecutable propio, con el flag que las pedía
/// cuando todo vivía en un solo binario.
const TOOLS: &[(&str, &str)] = &[
    ("--explorer", "OptimizedExplorer.exe"),
    ("--taskmgr", "OptimizedTaskmgr.exe"),
    ("--logon", "OptimizedLogonUI.exe"),
    ("--lock", "OptimizedLogonUI.exe"),
];

/// Ejecutable que atiende estos argumentos: el de la herramienta si está
/// instalado junto a este binario, y si no el propio (que sigue sabiendo
/// hacerlo todo). Los argumentos no cambian: cada herramienta entiende su
/// propio flag y se queda con el resto (la ruta, la pestaña, `--demo`...).
fn exe_for(args: &str) -> String {
    let first = args.split_whitespace().next().unwrap_or("");
    if let Some((_, tool)) = TOOLS.iter().find(|(flag, _)| *flag == first) {
        if let Some(dir) = std::path::Path::new(&own_exe()).parent() {
            let cand = dir.join(tool);
            if cand.is_file() {
                return cand.to_string_lossy().to_string();
            }
        }
    }
    own_exe()
}

/// Lanza la herramienta que pidan estos argumentos (`--settings`,
/// `--explorer <ruta>`, `--taskmgr`, `--lock`...): su propio ejecutable si
/// está instalado, o otra instancia de OptimizedShell si no.
pub fn launch_own(args: &str) {
    let exe = exe_for(args);
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), ui::wide("open").as_ptr(), ui::wide(&exe).as_ptr(),
                      ui::wide(args).as_ptr(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

fn toggle_menu(st: &State) {
    startmenu::toggle(st.menu, st.hwnd);
}

/// Vuelve a leer los anclados y repinta (después de anclar o desanclar).
fn pins_changed(st: &mut State) {
    refresh_pins(st);
    layout(st);
    ui::broadcast_config_changed();
    unsafe { InvalidateRect(st.hwnd, std::ptr::null(), 0) };
}

/// Menú del botón derecho sobre un icono anclado.
fn pin_menu(st: &mut State, i: usize, sx: i32, sy: i32) {
    let Some(p) = st.pins.get(i) else { return };
    let (path, label, win) = (p.path.clone(), p.label.clone(), p.running);
    let abierto = !win.is_null();
    let items = vec![
        ui::menu::Item::header(&label),
        ui::menu::Item::new(1, if abierto { "Nueva ventana" } else { "Abrir" }).glyph("\u{E8A7}"),
        ui::menu::Item::new(2, "Ejecutar como administrador").glyph("\u{E7EF}"),
        ui::menu::Item::new(3, "Abrir la ubicación del archivo").glyph("\u{EC50}"),
        ui::menu::Item::sep(),
        ui::menu::Item::new(4, "Cerrar la ventana").glyph("\u{E8BB}").enabled(abierto),
        ui::menu::Item::new(5, "Desanclar de la barra de tareas").glyph("\u{E77A}").danger(),
    ];
    match ui::menu::popup(st.hwnd, sx, sy, items) {
        1 => crate::apps::open_pin(&path),
        2 => crate::apps::launch_elevated(&path),
        3 => {
            if let Some(dir) = crate::apps::parent_dir(&path) {
                launch_own(&format!("--explorer \"{}\"", dir));
            }
        }
        4 => {
            if abierto {
                unsafe { PostMessageW(win, WM_CLOSE, 0, 0) };
            }
        }
        5 => {
            crate::apps::taskbar_unpin(&path);
            pins_changed(st);
        }
        _ => {}
    }
}

/// Menú del botón derecho sobre el botón de una ventana abierta.
fn app_menu(st: &mut State, i: usize, sx: i32, sy: i32) {
    let Some(a) = st.apps.get(i) else { return };
    let (win, title) = (a.hwnd, a.title.clone());
    let exe = window_exe(win);
    let anclado = exe.as_deref().map(crate::apps::taskbar_is_pinned).unwrap_or(false);
    let mut items = vec![
        ui::menu::Item::header(&title),
        ui::menu::Item::new(1, "Traer al frente").glyph("\u{E8A7}"),
        ui::menu::Item::new(2, "Minimizar").glyph("\u{E921}"),
        ui::menu::Item::new(3, "Maximizar").glyph("\u{E922}"),
    ];
    if exe.is_some() {
        items.push(ui::menu::Item::sep());
        items.push(
            ui::menu::Item::new(4, if anclado {
                "Desanclar de la barra de tareas"
            } else {
                "Anclar a la barra de tareas"
            })
            .glyph("\u{E718}"),
        );
    }
    items.push(ui::menu::Item::sep());
    items.push(ui::menu::Item::new(5, "Cerrar").glyph("\u{E8BB}").danger());
    match ui::menu::popup(st.hwnd, sx, sy, items) {
        1 => unsafe {
            if IsIconic(win) != 0 {
                ShowWindow(win, SW_RESTORE);
            }
            SetForegroundWindow(win);
        },
        2 => unsafe {
            ShowWindow(win, SW_MINIMIZE);
        },
        3 => unsafe {
            ShowWindow(win, if IsZoomed(win) != 0 { SW_RESTORE } else { SW_MAXIMIZE });
        },
        4 => {
            if let Some(path) = exe {
                crate::apps::taskbar_toggle_pin(&path);
                pins_changed(st);
            }
        }
        5 => unsafe {
            PostMessageW(win, WM_CLOSE, 0, 0);
        },
        _ => {}
    }
}

// ------------------------------------------------------------ atajos globales
// Los que el shell de Windows reserva para sí: con OptimizedShell como shell
// no los atiende nadie, así que los registra la barra.
const HK_EXPLORER: i32 = 1; // Win+E
const HK_RUN: i32 = 2; // Win+R
const HK_SETTINGS: i32 = 3; // Win+I
const HK_DESKTOP: i32 = 4; // Win+D
const HK_TASKMGR: i32 = 5; // Ctrl+Mayús+Esc
const HK_START: i32 = 6; // Ctrl+Esc
const HK_NOTIF: i32 = 7; // Win+N
const HK_LOCK: i32 = 8; // Win+L

fn register_hotkeys(hwnd: HWND) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, MOD_CONTROL, MOD_SHIFT, MOD_WIN, VK_ESCAPE,
    };
    unregister_hotkeys(hwnd);
    if !config::current().b(Id::HotkeysActivas) {
        return;
    }
    unsafe {
        RegisterHotKey(hwnd, HK_EXPLORER, MOD_WIN, b'E' as u32);
        RegisterHotKey(hwnd, HK_RUN, MOD_WIN, b'R' as u32);
        RegisterHotKey(hwnd, HK_SETTINGS, MOD_WIN, b'I' as u32);
        RegisterHotKey(hwnd, HK_DESKTOP, MOD_WIN, b'D' as u32);
        RegisterHotKey(hwnd, HK_NOTIF, MOD_WIN, b'N' as u32);
        RegisterHotKey(hwnd, HK_LOCK, MOD_WIN, b'L' as u32);
        RegisterHotKey(hwnd, HK_TASKMGR, MOD_CONTROL | MOD_SHIFT, VK_ESCAPE as u32);
        RegisterHotKey(hwnd, HK_START, MOD_CONTROL, VK_ESCAPE as u32);
    }
}

fn unregister_hotkeys(hwnd: HWND) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey;
    unsafe {
        for id in HK_EXPLORER..=HK_LOCK {
            UnregisterHotKey(hwnd, id);
        }
    }
}

fn run_hotkey(st: &mut State, id: i32) {
    match id {
        HK_EXPLORER => launch_own(&format!(
            "--explorer \"{}\"",
            std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into())
        )),
        HK_RUN => unsafe {
            ShellExecuteW(std::ptr::null_mut(), ui::wide("open").as_ptr(),
                          ui::wide("explorer.exe").as_ptr(),
                          ui::wide("shell:::{2559a1f3-21d7-11d4-bdaf-00c04f60b9f0}").as_ptr(),
                          std::ptr::null(), SW_SHOWNORMAL);
        },
        HK_SETTINGS => launch_own("--settings"),
        HK_DESKTOP => crate::desktop::show_desktop(),
        HK_TASKMGR => launch_own("--taskmgr"),
        HK_START => toggle_menu(st),
        HK_NOTIF => {
            let x = st
                .tray
                .items
                .iter()
                .find(|i| i.kind == tray::Kind::Bell)
                .map(|i| (i.rect.left + i.rect.right) / 2)
                .unwrap_or(st.clock_rc.left);
            notify::toggle_center(st.notif.center, x);
            unsafe { InvalidateRect(st.hwnd, std::ptr::null(), 0) };
        }
        HK_LOCK => launch_own("--lock"),
        _ => {}
    }
}

// ------------------------------------------------------------ colocación
/// Rectángulo que ocupa la barra según la configuración.
pub fn bar_rect() -> RECT {
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
            let st = &mut *st_ptr;
            // El icono del botón Inicio puede haber cambiado de origen.
            START_ICON.with(|c| *c.borrow_mut() = None);
            refresh_pins(st);
            apply_placement(st, false);
            register_hotkeys(hwnd);
            ui::apply_perf();
            // Si el escritorio estaba desactivado y se acaba de activar, hay
            // que crearlo: la ventana no existía. (Al desactivarlo se esconde
            // sola al recibir este mismo mensaje.)
            if st.desktop.is_null() && config::current().b(config::Id::EscritorioActivo) {
                st.desktop = crate::desktop::create(ui::hinst());
            }
            InvalidateRect(hwnd, std::ptr::null(), 0);
            InvalidateRect(st.menu, std::ptr::null(), 0);
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
        WM_COPYDATA => {
            // Aviso enviado por otra instancia de OptimizedShell.
            if !st_ptr.is_null() && lp != 0 {
                let cds = &*(lp as *const COPYDATASTRUCT);
                if let Some(note) = notify::decode(cds) {
                    let st = &mut *st_ptr;
                    if notify::receive(&st.notif, note) {
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                }
            }
            1
        }
        WM_DISPLAYCHANGE => {
            if !st_ptr.is_null() {
                apply_placement(&mut *st_ptr, false);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_HOTKEY => {
            if !st_ptr.is_null() {
                run_hotkey(&mut *st_ptr, wp as i32);
            }
            0
        }
        crate::traynotify::WM_TRAY_CHANGED => {
            // Una aplicación ha añadido o quitado su icono de la bandeja.
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                layout(st);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_TIMER => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                if wp == 2 {
                    // Se ha cumplido la espera: enseña el tooltip de lo que hay debajo.
                    KillTimer(hwnd, 2);
                    if let Some((text, cx)) = tip_for(st) {
                        show_tip(st, &text, cx);
                    }
                    return 0;
                }
                refresh_apps(st);
                if config::current().b(Id::BandejaMostrar) && st.tray.sample(false) {
                    report_changes(st);
                    layout(st);
                }
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_MOUSEMOVE => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                if !st.tracking {
                    let mut tme: TRACKMOUSEEVENT = std::mem::zeroed();
                    tme.cbSize = std::mem::size_of::<TRACKMOUSEEVENT>() as u32;
                    tme.dwFlags = TME_LEAVE;
                    tme.hwndTrack = hwnd;
                    TrackMouseEvent(&mut tme);
                    st.tracking = true;
                }
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                let mut hov: isize = -1;
                if in_rect(&st.start_rc, x, y) && st.start_rc.right > st.start_rc.left {
                    hov = -2;
                } else if let Some(i) = st.tray.hit(x, y) {
                    hov = tray_hover(i);
                } else if let Some(i) = st.pins.iter().position(|p| p.visible && in_rect(&p.rect, x, y)) {
                    hov = pin_hover(i);
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
                    hide_tip(st);
                    KillTimer(hwnd, 2);
                    if hov != -1 {
                        // Pequeña espera antes del tooltip, como el resto de Windows.
                        SetTimer(hwnd, 2, 600, None);
                    }
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_MOUSELEAVE => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                st.tracking = false;
                st.hover = -1;
                KillTimer(hwnd, 2);
                hide_tip(st);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_MOUSEWHEEL => {
            // La rueda sobre el icono de volumen lo sube y lo baja.
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                if let Some(i) = tray_hover_index(st.hover) {
                    if st.tray.items.get(i).map(|t| t.kind) == Some(tray::Kind::Volume) {
                        let delta = ((wp >> 16) & 0xFFFF) as i16 as i32;
                        let v = (st.tray.snap.volume + (delta / 120) * 2).clamp(0, 100);
                        crate::sysinfo::set_volume(v);
                        st.tray.snap.volume = v;
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                hide_tip(st);
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                if in_rect(&st.start_rc, x, y) && st.start_rc.right > st.start_rc.left {
                    toggle_menu(st);
                    return 0;
                }
                if let Some(i) = st.tray.hit(x, y) {
                    let r = st.tray.items[i].rect;
                    let cx = (r.left + r.right) / 2;
                    match st.tray.items[i].kind {
                        tray::Kind::App(j) => {
                            // El clic es del programa dueño del icono, no nuestro.
                            if let Some(a) = st.tray.apps.get(j) {
                                let mut pt = POINT { x, y };
                                windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut pt);
                                crate::traynotify::forward_click(a, WM_LBUTTONUP, pt.x, pt.y);
                            }
                        }
                        tray::Kind::Bell => {
                            st.tray.sample(true);
                            notify::toggle_center(st.notif.center, cx);
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        }
                        _ => {
                            st.tray.sample(true);
                            tray::toggle_quick(st.quick, cx, &st.tray.snap);
                        }
                    }
                    return 0;
                }
                if let Some(i) = st.pins.iter().position(|p| p.visible && in_rect(&p.rect, x, y)) {
                    activate_pin(st, i);
                    return 0;
                }
                for a in &st.apps {
                    if in_rect(&a.rect, x, y) {
                        // Pulsar la ventana en primer plano la minimiza, como el shell real.
                        if GetForegroundWindow() == a.hwnd {
                            ShowWindow(a.hwnd, SW_MINIMIZE);
                        } else {
                            if IsIconic(a.hwnd) != 0 {
                                ShowWindow(a.hwnd, SW_RESTORE);
                            }
                            SetForegroundWindow(a.hwnd);
                        }
                        break;
                    }
                }
            }
            0
        }
        WM_RBUTTONUP => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                // Menús de los iconos anclados y de los botones de ventana.
                let pin = st.pins.iter().position(|p| p.visible && in_rect(&p.rect, x, y));
                let app = st.apps.iter().position(|a| in_rect(&a.rect, x, y));
                if pin.is_some() || app.is_some() {
                    let mut pt = POINT { x, y };
                    windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut pt);
                    hide_tip(st);
                    if let Some(i) = pin {
                        pin_menu(st, i, pt.x, pt.y);
                    } else if let Some(i) = app {
                        app_menu(st, i, pt.x, pt.y);
                    }
                    return 0;
                }
                // Botón derecho sobre la bandeja: la página de ajustes que toque.
                if let Some(i) = st.tray.hit(x, y) {
                    match st.tray.items[i].kind {
                        tray::Kind::App(j) => {
                            if let Some(a) = st.tray.apps.get(j) {
                                let mut pt = POINT { x, y };
                                windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut pt);
                                crate::traynotify::forward_click(a, WM_RBUTTONUP, pt.x, pt.y);
                            }
                        }
                        tray::Kind::Volume => launch_own("--settings sonido"),
                        tray::Kind::Net => launch_own("--settings red"),
                        tray::Kind::Bluetooth => launch_own("--settings bluetooth"),
                        tray::Kind::Battery => launch_own("--settings energia"),
                        tray::Kind::Bell => launch_own("--settings notificaciones"),
                    }
                    return 0;
                }
            }
            launch_own("--settings barra");
            0
        }
        WM_DESTROY => {
            unregister_hotkeys(hwnd);
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

        let class_name = ui::wide(CLASS);
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

        let tip_class = ui::wide(TIP_CLASS);
        let wtip = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(tip_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: tip_class.as_ptr(),
        };
        RegisterClassW(&wtip);

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
        let quick = tray::create_quick(hinst);
        let notif = notify::create(hinst);
        // El escritorio se crea antes que nada para que quede por debajo.
        let desktop = crate::desktop::create(hinst);
        ui::menu::register(hinst);
        // Ventana Shell_TrayWnd: recoge los iconos de las aplicaciones.
        crate::traynotify::create(hinst);
        let tip = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            tip_class.as_ptr(),
            ui::wide("").as_ptr(),
            WS_POPUP,
            0, 0, 10, 10,
            std::ptr::null_mut(), std::ptr::null_mut(), hinst, std::ptr::null(),
        );
        if !tip.is_null() {
            window::apply_frame_opaque(tip, config::current().dark());
        }

        let st = Box::new(State {
            hwnd,
            apps: Vec::new(),
            pins: Vec::new(),
            start_rc: er(),
            clock_rc: er(),
            hover: -1,
            menu,
            quick,
            notif,
            tip,
            tray: Tray::new(),
            desktop,
            tracking: false,
            last_net_up: None,
            last_charging: None,
            last_battery_level: 100,
        });
        let raw = Box::into_raw(st);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
        (*raw).tray.sample(true);
        report_changes(&mut *raw);
        apply_placement(&mut *raw, true);
        refresh_apps(&mut *raw);
        ShowWindow(hwnd, SW_SHOW);
        register_hotkeys(hwnd);
        ui::apply_perf();
        // Busca versiones nuevas en GitHub (en un hilo aparte, si el ajuste
        // está activado y toca comprobar) y limpia el .exe de la anterior.
        crate::update::check_on_start();

        let code = window::message_loop();
        ui::shutdown();
        code
    }
}
