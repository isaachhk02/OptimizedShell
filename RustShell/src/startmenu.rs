// Menú Inicio estilo Windows 7.
//
// Dos columnas: a la izquierda la lista de programas (los más usados primero y
// un botón «Todos los programas» que enseña la lista completa) con un cuadro de
// búsqueda abajo que filtra mientras se escribe; a la derecha el usuario, las
// carpetas personales, Equipo, Panel de control, las herramientas propias y los
// botones de bloquear/reiniciar/apagar.
//
// El contenido sale de `apps` (accesos directos reales del menú Inicio de
// Windows) y las medidas de `ui::config`, así que el menú se puede estrechar,
// alargar, quitarle la columna derecha o la búsqueda desde Configuración.
#![allow(non_snake_case, non_upper_case_globals)]

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::System::Shutdown::{ExitWindowsEx, EWX_REBOOT, EWX_SHUTDOWN, SHTDN_REASON_FLAG_PLANNED};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SetFocus, VK_BACK, VK_DOWN, VK_ESCAPE, VK_NEXT, VK_PRIOR, VK_RETURN, VK_UP,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::apps;
use crate::ui::config::{self, Id};
use crate::ui::gdip::{Align, Gfx};
use crate::ui::{self, theme, window};

pub const CLASS: &str = "OShellRustStartMenu";

// ---------------------------------------------------------------- acciones
#[derive(Clone, Copy, PartialEq)]
enum Act {
    /// Abre una carpeta del perfil del usuario con el explorador propio.
    Folder(&'static str),
    /// Lanza OptimizedShell con estos argumentos.
    Own(&'static str),
    Control,
    Run,
    Lock,
    Reboot,
    Shutdown,
}

struct Entry {
    glyph: &'static str,
    label: &'static str,
    act: Act,
}

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    None,
    Row(usize),
    All,
    Right(usize),
    Power(usize),
}

struct Menu {
    apps: Vec<apps::App>,
    /// Índices de `apps` que se están mostrando.
    view: Vec<usize>,
    /// Lista completa (true) o solo los frecuentes (false).
    all_mode: bool,
    /// Cuántas de las primeras filas de `view` son programas anclados.
    pinned_count: usize,
    filter: String,
    sel: isize,
    scroll: i32,
    hover: Hit,
    user: String,
    // layout
    list: RECT,
    allbtn: RECT,
    search: RECT,
    right_rc: RECT,
    rows: Vec<RECT>,
    right_items: Vec<Entry>,
    right_rects: Vec<RECT>,
    power: Vec<Entry>,
    power_rects: Vec<RECT>,
}

fn er() -> RECT {
    RECT { left: 0, top: 0, right: 0, bottom: 0 }
}
fn in_rect(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}
fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}
fn rw(r: &RECT) -> i32 {
    r.right - r.left
}
fn rh(r: &RECT) -> i32 {
    r.bottom - r.top
}

fn item_h() -> i32 {
    ui::s(config::current().get(Id::MenuAltoElemento))
}

/// Ancho de la lista, ancho de la columna derecha y tamaño total del menú.
pub fn metrics() -> (i32, i32, i32, i32) {
    let c = config::current();
    let left_w = ui::s(c.get(Id::MenuAncho));
    let right_w = if c.b(Id::MenuColumnaDerecha) { ui::s(208) } else { 0 };
    let h = ui::s(c.get(Id::MenuAlto));
    (left_w, right_w, left_w + right_w, h)
}

// ---------------------------------------------------------------- contenido
fn right_entries() -> Vec<Entry> {
    let c = config::current();
    let mut v = vec![
        Entry { glyph: "\u{E8B7}", label: "Documentos", act: Act::Folder("Documents") },
        Entry { glyph: "\u{EB9F}", label: "Imágenes", act: Act::Folder("Pictures") },
        Entry { glyph: "\u{E8D6}", label: "Música", act: Act::Folder("Music") },
        Entry { glyph: "\u{E896}", label: "Descargas", act: Act::Folder("Downloads") },
        Entry { glyph: "\u{E977}", label: "Equipo", act: Act::Own("--explorer C:\\") },
        Entry { glyph: "\u{E713}", label: "Panel de control", act: Act::Control },
        Entry { glyph: "\u{E115}", label: "Configuración", act: Act::Own("--settings") },
        Entry { glyph: "\u{E9D9}", label: "Administrador de tareas", act: Act::Own("--taskmgr") },
    ];
    if c.b(Id::MenuEjecutar) {
        v.push(Entry { glyph: "\u{E756}", label: "Ejecutar...", act: Act::Run });
    }
    v
}

fn power_entries() -> Vec<Entry> {
    let c = config::current();
    let mut v = Vec::new();
    if c.b(Id::MenuBloquear) {
        v.push(Entry { glyph: "\u{E72E}", label: "Bloquear", act: Act::Lock });
    }
    if c.b(Id::MenuApagado) {
        v.push(Entry { glyph: "\u{E777}", label: "Reiniciar", act: Act::Reboot });
        v.push(Entry { glyph: "\u{E7E8}", label: "Apagar", act: Act::Shutdown });
    }
    v
}

fn run_act(act: Act) {
    match act {
        Act::Folder(sub) => {
            let base = std::env::var("USERPROFILE").unwrap_or_default();
            crate::shell::launch_own(&format!("--explorer {}\\{}", base, sub));
        }
        Act::Own(args) => crate::shell::launch_own(args),
        Act::Control => unsafe {
            ShellExecuteW(std::ptr::null_mut(), ui::wide("open").as_ptr(), ui::wide("control.exe").as_ptr(),
                          std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
        },
        Act::Run => unsafe {
            // Cuadro «Ejecutar» del shell de Windows.
            ShellExecuteW(std::ptr::null_mut(), ui::wide("open").as_ptr(), ui::wide("explorer.exe").as_ptr(),
                          ui::wide("shell:::{2559a1f3-21d7-11d4-bdaf-00c04f60b9f0}").as_ptr(),
                          std::ptr::null(), SW_SHOWNORMAL);
        },
        Act::Lock => crate::shell::launch_own("--lock"),
        Act::Reboot => unsafe { ExitWindowsEx(EWX_REBOOT, SHTDN_REASON_FLAG_PLANNED); },
        Act::Shutdown => unsafe { ExitWindowsEx(EWX_SHUTDOWN, SHTDN_REASON_FLAG_PLANNED); },
    }
}

/// Recalcula qué programas se ven: filtro de búsqueda, lista completa o
/// solo los más usados.
fn rebuild_view(m: &mut Menu) {
    let f = m.filter.trim().to_lowercase();
    if !f.is_empty() {
        m.view = m
            .apps
            .iter()
            .enumerate()
            .filter(|(_, a)| a.name.to_lowercase().contains(&f))
            .map(|(i, _)| i)
            .collect();
    } else if m.all_mode {
        m.view = (0..m.apps.len()).collect();
    } else {
        // Primera pantalla: anclados arriba y, debajo, los más usados que no
        // estén ya anclados.
        let pinned = apps::pinned_indices(&m.apps);
        let n = config::current().get(Id::MenuFrecuentes).max(0) as usize;
        let mut v = pinned.clone();
        for i in apps::frequent(&m.apps, n + pinned.len()) {
            if v.len() >= n + pinned.len() {
                break;
            }
            if !pinned.contains(&i) {
                v.push(i);
            }
        }
        m.pinned_count = pinned.len();
        m.view = v;
    }
    if m.all_mode || !m.filter.trim().is_empty() {
        m.pinned_count = 0;
    }
    m.scroll = 0;
    m.sel = if m.view.is_empty() { -1 } else { 0 };
}

// ---------------------------------------------------------------- layout
fn layout(hwnd: HWND, m: &mut Menu) {
    let mut rc = er();
    unsafe { GetClientRect(hwnd, &mut rc) };
    let (left_w, right_w, _, _) = metrics();
    let pad = ui::s(10);
    let ih = item_h();
    let show_search = config::current().b(Id::MenuBusqueda);

    let left = rect(0, 0, if right_w > 0 { left_w } else { rc.right }, rc.bottom);
    m.right_rc = rect(left.right, 0, right_w, rc.bottom);

    let search_h = if show_search { ui::s(34) } else { 0 };
    let all_h = ui::s(34);
    let bottom = left.bottom - pad - search_h - if show_search { ui::s(8) } else { 0 };
    m.allbtn = rect(pad, bottom - all_h, rw(&left) - pad * 2, all_h);
    m.search = if show_search {
        rect(pad, m.allbtn.bottom + ui::s(8), rw(&left) - pad * 2, ui::s(34))
    } else {
        er()
    };
    m.list = RECT { left: pad, top: pad + ui::s(32), right: left.right - pad, bottom: m.allbtn.top - ui::s(6) };

    let vis = (rh(&m.list) / ih.max(1)).max(0);
    m.rows = (0..vis)
        .map(|i| rect(m.list.left, m.list.top + i * ih, rw(&m.list), ih))
        .collect();

    // columna derecha
    m.right_items = right_entries();
    m.power = power_entries();
    let rpad = ui::s(8);
    let rih = ui::s(30).max(ih * 3 / 4);
    let top = ui::s(78);
    m.right_rects = (0..m.right_items.len() as i32)
        .map(|i| rect(m.right_rc.left + rpad, top + i * rih, right_w - rpad * 2, rih))
        .collect();
    // Botonera inferior al estilo de Windows 7: los secundarios como iconos
    // cuadrados y el último (Apagar) ancho y con etiqueta.
    let py = rc.bottom - ui::s(44);
    let bh = ui::s(34);
    let small = ui::s(36);
    let n = m.power.len() as i32;
    m.power_rects = Vec::new();
    if n > 0 {
        let avail = right_w - rpad * 2;
        let wide = avail - (n - 1) * (small + ui::s(6));
        let mut x = m.right_rc.left + rpad;
        for i in 0..n {
            let w = if i == n - 1 { wide.max(small) } else { small };
            m.power_rects.push(rect(x, py, w, bh));
            x += w + ui::s(6);
        }
    }
}

// ---------------------------------------------------------------- pintado
fn paint(hwnd: HWND, m: &Menu) {
    let c = config::current();
    let icons = c.b(Id::MenuIconosApps);
    window::paint_buffered(hwnd, |g: &Gfx, w, h| {
        let s = theme::style();
        g.clear(theme::argb(0, 0, 0, 0));
        g.rounded(0.0, 0.0, w as f32, h as f32, ui::sf(10.0), Some(s.menu_bg), Some((s.menu_border, 1.0)));

        // ---- columna derecha (fondo)
        if rw(&m.right_rc) > 0 {
            let r = m.right_rc;
            g.fill_rect(r.left, ui::s(8), rw(&r) - ui::s(8), h - ui::s(16), s.nav_bg);
            g.line(r.left as f32, ui::s(12) as f32, r.left as f32, (h - ui::s(12)) as f32, s.separator, 1.0);
            // usuario
            let av = ui::s(44);
            let cx = r.left + rw(&r) / 2;
            g.fill_circle(cx as f32, (ui::s(14) + av / 2) as f32, (av / 2) as f32, s.accent_solid);
            let initial = m.user.chars().next().unwrap_or('U').to_uppercase().to_string();
            g.text_in(&initial, (cx - av / 2) as f32, ui::s(14) as f32, av as f32, av as f32, 18.0, true,
                      s.on_accent, Align::Center, Align::Center, false);
            g.text_in(&m.user, r.left as f32, (ui::s(16) + av) as f32, rw(&r) as f32, ui::s(20) as f32,
                      12.0, true, s.text, Align::Center, Align::Center, false);

            for (i, (e, rr)) in m.right_items.iter().zip(m.right_rects.iter()).enumerate() {
                if m.hover == Hit::Right(i) {
                    g.rounded(rr.left as f32, rr.top as f32, rw(rr) as f32, rh(rr) as f32, ui::sf(5.0),
                              Some(s.menu_hover), None);
                }
                g.text_in(e.glyph, (rr.left + ui::s(6)) as f32, rr.top as f32, ui::s(22) as f32, rh(rr) as f32,
                          11.0, false, s.accent, Align::Center, Align::Center, true);
                g.text_in(e.label, (rr.left + ui::s(32)) as f32, rr.top as f32, (rw(rr) - ui::s(36)) as f32,
                          rh(rr) as f32, 12.0, false, s.text, Align::Near, Align::Center, false);
            }
            // apagado: los secundarios con icono, el último con etiqueta
            let last = m.power.len().saturating_sub(1);
            for (i, (e, rr)) in m.power.iter().zip(m.power_rects.iter()).enumerate() {
                let hov = m.hover == Hit::Power(i);
                let danger = e.act == Act::Shutdown;
                g.rounded(rr.left as f32, rr.top as f32, rw(rr) as f32, rh(rr) as f32, ui::sf(6.0),
                          Some(if hov { if danger { s.close_hover } else { s.control_hover } } else { s.control_bg }),
                          Some((s.control_border, 1.0)));
                let fg = if hov && danger { theme::rgbc(255, 255, 255) } else { s.text };
                let wide = i == last && rw(rr) > ui::s(70);
                if wide {
                    g.text_in(e.glyph, (rr.left + ui::s(8)) as f32, rr.top as f32, ui::s(20) as f32,
                              rh(rr) as f32, 10.5, false, fg, Align::Center, Align::Center, true);
                    g.text_in(e.label, (rr.left + ui::s(30)) as f32, rr.top as f32, (rw(rr) - ui::s(36)) as f32,
                              rh(rr) as f32, 12.0, false, fg, Align::Near, Align::Center, false);
                } else {
                    g.text_in(e.glyph, rr.left as f32, rr.top as f32, rw(rr) as f32, rh(rr) as f32, 11.0, false,
                              fg, Align::Center, Align::Center, true);
                }
            }
        }

        // ---- cabecera de la lista
        let title = if !m.filter.is_empty() {
            format!("Resultados ({})", m.view.len())
        } else if m.all_mode {
            format!("Todos los programas ({})", m.view.len())
        } else if m.pinned_count > 0 {
            "Anclados y frecuentes".to_string()
        } else {
            "Programas frecuentes".to_string()
        };
        g.text_in(&title, m.list.left as f32, ui::s(8) as f32, rw(&m.list) as f32, ui::s(22) as f32,
                  11.0, true, s.text_dim, Align::Near, Align::Center, false);

        // ---- lista de programas
        let ih = item_h();
        for (row, rr) in m.rows.iter().enumerate() {
            let idx = m.scroll as usize + row;
            if idx >= m.view.len() {
                break;
            }
            let a = &m.apps[m.view[idx]];
            let sel = idx as isize == m.sel;
            if sel {
                g.rounded(rr.left as f32, rr.top as f32, rw(rr) as f32, (rh(rr) - ui::s(2)) as f32,
                          ui::sf(6.0), Some(s.row_selected), None);
            } else if m.hover == Hit::Row(row) {
                g.rounded(rr.left as f32, rr.top as f32, rw(rr) as f32, (rh(rr) - ui::s(2)) as f32,
                          ui::sf(6.0), Some(s.menu_hover), None);
            }
            let mut tx = rr.left + ui::s(10);
            if icons {
                let size = ui::s(20);
                let hicon = apps::icon(&a.path);
                if !hicon.is_null() {
                    g.draw_hicon(hicon, rr.left + ui::s(8), rr.top + (ih - size) / 2, size);
                } else {
                    g.text_in("\u{E7C3}", (rr.left + ui::s(8)) as f32, rr.top as f32, size as f32, ih as f32,
                              11.0, false, s.text_faint, Align::Center, Align::Center, true);
                }
                tx = rr.left + ui::s(8) + size + ui::s(10);
            }
            // Chincheta en los anclados (y en cualquier fila que lo esté).
            let is_pinned = if idx < m.pinned_count {
                true
            } else {
                apps::is_pinned(&a.path)
            };
            let name_w = rr.right - tx - ui::s(8) - if is_pinned { ui::s(18) } else { 0 };
            g.text_in(&a.name, tx as f32, rr.top as f32, name_w as f32, ih as f32,
                      12.0, false, s.text, Align::Near, Align::Center, false);
            if is_pinned {
                g.text_in("\u{E718}", (rr.right - ui::s(22)) as f32, rr.top as f32, ui::s(16) as f32,
                          ih as f32, 8.5, false, s.accent, Align::Center, Align::Center, true);
            }
        }
        if m.view.is_empty() {
            let msg = if m.filter.is_empty() {
                "No se han encontrado accesos directos en el menú Inicio."
            } else {
                "Ningún programa coincide."
            };
            g.text_in(msg, m.list.left as f32, (m.list.top + ui::s(16)) as f32, rw(&m.list) as f32,
                      ui::s(24) as f32, 12.0, false, s.text_faint, Align::Center, Align::Center, false);
        }
        // barra de desplazamiento
        let vis = m.rows.len() as i32;
        let total = m.view.len() as i32;
        if total > vis && vis > 0 {
            let track_h = rh(&m.list);
            let bar = (track_h * vis / total).max(ui::s(24));
            let y = m.list.top + (track_h - bar) * m.scroll / (total - vis).max(1);
            g.rounded((m.list.right - ui::s(4)) as f32, y as f32, ui::s(3) as f32, bar as f32,
                      ui::sf(2.0), Some(s.track), None);
        }

        // ---- «Todos los programas»
        let hov = m.hover == Hit::All;
        g.rounded(m.allbtn.left as f32, m.allbtn.top as f32, rw(&m.allbtn) as f32, rh(&m.allbtn) as f32,
                  ui::sf(6.0), Some(if hov { s.menu_hover } else { theme::argb(0, 0, 0, 0) }), None);
        g.text_in(if m.all_mode { "\u{E72B}" } else { "\u{E72A}" }, (m.allbtn.left + ui::s(8)) as f32,
                  m.allbtn.top as f32, ui::s(20) as f32, rh(&m.allbtn) as f32, 10.0, false, s.accent,
                  Align::Center, Align::Center, true);
        g.text_in(if m.all_mode { "Atrás" } else { "Todos los programas" },
                  (m.allbtn.left + ui::s(34)) as f32, m.allbtn.top as f32, (rw(&m.allbtn) - ui::s(40)) as f32,
                  rh(&m.allbtn) as f32, 12.5, true, s.text, Align::Near, Align::Center, false);

        // ---- búsqueda
        if rw(&m.search) > 0 {
            g.rounded(m.search.left as f32, m.search.top as f32, rw(&m.search) as f32, rh(&m.search) as f32,
                      ui::sf(6.0), Some(s.control_bg), Some((s.control_border, 1.0)));
            g.text_in("\u{E721}", (m.search.left + ui::s(8)) as f32, m.search.top as f32, ui::s(20) as f32,
                      rh(&m.search) as f32, 11.0, false, s.text_dim, Align::Center, Align::Center, true);
            let (txt, col) = if m.filter.is_empty() {
                ("Buscar programas".to_string(), s.text_faint)
            } else {
                (m.filter.clone(), s.text)
            };
            g.text_in(&txt, (m.search.left + ui::s(34)) as f32, m.search.top as f32,
                      (rw(&m.search) - ui::s(42)) as f32, rh(&m.search) as f32, 12.0, false, col,
                      Align::Near, Align::Center, false);
        }
    });
}

// ---------------------------------------------------------------- interacción
fn hit(m: &Menu, x: i32, y: i32) -> Hit {
    for (i, r) in m.rows.iter().enumerate() {
        if in_rect(r, x, y) {
            return if m.scroll as usize + i < m.view.len() { Hit::Row(i) } else { Hit::None };
        }
    }
    if in_rect(&m.allbtn, x, y) {
        return Hit::All;
    }
    for (i, r) in m.right_rects.iter().enumerate() {
        if in_rect(r, x, y) {
            return Hit::Right(i);
        }
    }
    for (i, r) in m.power_rects.iter().enumerate() {
        if in_rect(r, x, y) {
            return Hit::Power(i);
        }
    }
    Hit::None
}

fn launch_sel(hwnd: HWND, m: &mut Menu) {
    let Some(&idx) = m.view.get(m.sel.max(0) as usize) else { return };
    let path = m.apps[idx].path.clone();
    unsafe { ShowWindow(hwnd, SW_HIDE) };
    apps::launch(&path);
}

fn click(hwnd: HWND, m: &mut Menu, x: i32, y: i32) {
    match hit(m, x, y) {
        Hit::Row(row) => {
            m.sel = (m.scroll as usize + row) as isize;
            launch_sel(hwnd, m);
        }
        Hit::All => {
            m.all_mode = !m.all_mode;
            m.filter.clear();
            rebuild_view(m);
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
        }
        Hit::Right(i) => {
            let act = m.right_items[i].act;
            unsafe { ShowWindow(hwnd, SW_HIDE) };
            run_act(act);
        }
        Hit::Power(i) => {
            let act = m.power[i].act;
            unsafe { ShowWindow(hwnd, SW_HIDE) };
            run_act(act);
        }
        Hit::None => {}
    }
}

/// Menú contextual de un programa de la lista.
fn app_menu(hwnd: HWND, m: &mut Menu, app: &apps::App, x: i32, y: i32) {
    let pinned = apps::is_pinned(&app.path);
    let items = vec![
        ui::menu::Item::header(&app.name),
        ui::menu::Item::new(1, "Abrir").glyph("\u{E8E5}"),
        ui::menu::Item::new(2, "Ejecutar como administrador").glyph("\u{E7EF}"),
        ui::menu::Item::sep(),
        ui::menu::Item::new(3, if pinned { "Desanclar de Inicio" } else { "Anclar a Inicio" })
            .glyph("\u{E718}")
            .checked(pinned),
        ui::menu::Item::new(4, "Abrir ubicación del archivo").glyph("\u{E8DA}"),
        ui::menu::Item::new(5, "Copiar ruta").glyph("\u{E8C8}"),
    ];
    match ui::menu::popup(hwnd, x, y, items) {
        1 => {
            unsafe { ShowWindow(hwnd, SW_HIDE) };
            apps::launch(&app.path);
        }
        2 => {
            unsafe { ShowWindow(hwnd, SW_HIDE) };
            apps::launch_elevated(&app.path);
        }
        3 => {
            apps::toggle_pin(&app.path);
            rebuild_view(m);
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
        }
        4 => {
            if let Some(dir) = apps::parent_dir(&app.path) {
                unsafe { ShowWindow(hwnd, SW_HIDE) };
                crate::shell::launch_own(&format!("--explorer {}", dir));
            }
        }
        5 => {
            ui::set_clipboard_text(&app.path);
        }
        _ => {}
    }
}

fn scroll_by(m: &mut Menu, delta: i32) {
    let vis = m.rows.len() as i32;
    let max = (m.view.len() as i32 - vis).max(0);
    m.scroll = (m.scroll + delta).clamp(0, max);
}

/// Mantiene la selección a la vista al moverse con las flechas.
fn ensure_visible(m: &mut Menu) {
    let vis = m.rows.len() as i32;
    if vis <= 0 || m.sel < 0 {
        return;
    }
    if m.sel < m.scroll as isize {
        m.scroll = m.sel as i32;
    } else if m.sel >= (m.scroll + vis) as isize {
        m.scroll = m.sel as i32 - vis + 1;
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let m_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Menu;
    if msg == ui::config_changed_msg() {
        ui::reload_config();
        if !m_ptr.is_null() {
            let m = &mut *m_ptr;
            resize(hwnd);
            layout(hwnd, m);
            rebuild_view(m);
        }
        InvalidateRect(hwnd, std::ptr::null(), 0);
        return 0;
    }
    match msg {
        WM_ERASEBKGND => 1,
        WM_SIZE => {
            if !m_ptr.is_null() {
                layout(hwnd, &mut *m_ptr);
            }
            0
        }
        WM_PAINT => {
            if !m_ptr.is_null() {
                paint(hwnd, &*m_ptr);
            }
            0
        }
        WM_MOUSEMOVE => {
            if !m_ptr.is_null() {
                let m = &mut *m_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                let h = hit(m, x, y);
                if h != m.hover {
                    m.hover = h;
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !m_ptr.is_null() {
                let m = &mut *m_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                click(hwnd, m, x, y);
            }
            0
        }
        WM_RBUTTONUP => {
            // Menú del programa: anclar, abrir ubicación, ejecutar como admin.
            if !m_ptr.is_null() {
                let m = &mut *m_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                if let Hit::Row(row) = hit(m, x, y) {
                    let idx = m.scroll as usize + row;
                    if let Some(&ai) = m.view.get(idx) {
                        m.sel = idx as isize;
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                        let app = m.apps[ai].clone();
                        let mut pt = windows_sys::Win32::Foundation::POINT { x, y };
                        windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut pt);
                        app_menu(hwnd, m, &app, pt.x, pt.y);
                    }
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            if !m_ptr.is_null() {
                let m = &mut *m_ptr;
                let delta = ((wp >> 16) & 0xFFFF) as i16 as i32;
                scroll_by(m, -(delta / 120) * 3);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_KEYDOWN => {
            if !m_ptr.is_null() {
                let m = &mut *m_ptr;
                let vk = wp as u32;
                let vis = m.rows.len() as isize;
                if vk == VK_ESCAPE as u32 {
                    ShowWindow(hwnd, SW_HIDE);
                } else if vk == VK_RETURN as u32 {
                    launch_sel(hwnd, m);
                } else if vk == VK_DOWN as u32 || vk == VK_UP as u32 {
                    let d = if vk == VK_DOWN as u32 { 1 } else { -1 };
                    let n = m.view.len() as isize;
                    if n > 0 {
                        m.sel = (m.sel + d).clamp(0, n - 1);
                        ensure_visible(m);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                } else if vk == VK_NEXT as u32 || vk == VK_PRIOR as u32 {
                    let d = if vk == VK_NEXT as u32 { vis } else { -vis };
                    let n = m.view.len() as isize;
                    if n > 0 {
                        m.sel = (m.sel + d).clamp(0, n - 1);
                        ensure_visible(m);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                }
            }
            0
        }
        WM_CHAR => {
            if !m_ptr.is_null() {
                let m = &mut *m_ptr;
                let ch = wp as u32;
                if !config::current().b(Id::MenuBusqueda) {
                    return 0;
                }
                if ch == VK_BACK as u32 {
                    m.filter.pop();
                    rebuild_view(m);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                } else if ch >= 0x20 {
                    if let Some(c) = char::from_u32(ch) {
                        m.filter.push(c);
                        rebuild_view(m);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                }
            }
            0
        }
        WM_ACTIVATE => {
            if (wp & 0xFFFF) == WA_INACTIVE as usize {
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_DESTROY => {
            if !m_ptr.is_null() {
                drop(Box::from_raw(m_ptr));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Ajusta el tamaño de la ventana del menú a la configuración actual.
fn resize(hwnd: HWND) {
    let (_, _, w, h) = metrics();
    unsafe {
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

// ---------------------------------------------------------------- creación
pub fn register(hinst: windows_sys::Win32::Foundation::HINSTANCE) {
    unsafe {
        let class = ui::wide(CLASS);
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
    }
}

pub fn create(hinst: windows_sys::Win32::Foundation::HINSTANCE) -> HWND {
    unsafe {
        let (_, _, w, h) = metrics();
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            ui::wide(CLASS).as_ptr(),
            ui::wide("Inicio").as_ptr(),
            WS_POPUP,
            0, 0, w, h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return hwnd;
        }
        let m = Box::new(Menu {
            apps: Vec::new(),
            view: Vec::new(),
            all_mode: false,
            pinned_count: 0,
            filter: String::new(),
            sel: -1,
            scroll: 0,
            hover: Hit::None,
            user: std::env::var("USERNAME").unwrap_or_else(|_| "Usuario".into()),
            list: er(),
            allbtn: er(),
            search: er(),
            right_rc: er(),
            rows: Vec::new(),
            right_items: Vec::new(),
            right_rects: Vec::new(),
            power: Vec::new(),
            power_rects: Vec::new(),
        });
        let raw = Box::into_raw(m);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
        window::apply_frame_opaque(hwnd, config::current().dark());
        layout(hwnd, &mut *raw);
        hwnd
    }
}

/// Muestra u oculta el menú, colocándolo junto a la barra de tareas.
pub fn toggle(menu: HWND, bar: HWND) {
    unsafe {
        if menu.is_null() {
            return;
        }
        if IsWindowVisible(menu) != 0 {
            ShowWindow(menu, SW_HIDE);
            return;
        }
        let m_ptr = GetWindowLongPtrW(menu, GWLP_USERDATA) as *mut Menu;
        if !m_ptr.is_null() {
            let m = &mut *m_ptr;
            // Se reexplora al abrir: así aparecen los programas recién
            // instalados y se actualizan los contadores de uso.
            m.apps = apps::scan();
            m.all_mode = false;
            m.filter.clear();
            m.hover = Hit::None;
            rebuild_view(m);
        }
        let (_, _, w, h) = metrics();
        let mut br = er();
        GetWindowRect(bar, &mut br);
        let x = br.left + ui::s(8);
        let y = if config::current().get(Id::BarraPosicion) == 1 {
            br.bottom + ui::s(8)
        } else {
            br.top - h - ui::s(8)
        };
        SetWindowPos(menu, HWND_TOPMOST, x, y.max(0), w, h, SWP_SHOWWINDOW);
        if !m_ptr.is_null() {
            layout(menu, &mut *m_ptr);
        }
        SetForegroundWindow(menu);
        SetFocus(menu);
        InvalidateRect(menu, std::ptr::null(), 0);
    }
}
