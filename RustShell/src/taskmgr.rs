// Administrador de tareas (--taskmgr). Port de CppShell/src/taskmgr.cpp.
//
// Marco propio con barra de título (icono + minimizar/maximizar/cerrar),
// pestañas Procesos · Rendimiento · Inicio · Servicios, barra de herramientas
// por pestaña con buscador, y tabla con columnas ordenables. La pestaña
// Procesos usa un único barrido de NtQuerySystemInformation (sin abrir un handle
// por proceso) para nombre, PID, %CPU (por delta), memoria, E/S y hilos.
//
// Las gráficas de Rendimiento y las pestañas Inicio/Servicios comparten la
// estructura pero su contenido completo se añadirá en iteraciones siguientes.
#![allow(non_snake_case, non_upper_case_globals)]

use std::collections::HashMap;
use std::ffi::c_void;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, ScreenToClient};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    OpenProcess, TerminateProcess, PROCESS_TERMINATE,
};
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_BACK, VK_DELETE, VK_ESCAPE,
};
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::ui::gdip::{Align, Chrome, Gfx};
use crate::ui::config::{self, Id};
use crate::ui::{self, theme, window};
use crate::startup;

// ---------------------------------------------------------------- Nt API
#[link(name = "ntdll")]
extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
}
const SYSTEM_PROCESS_INFORMATION_CLASS: u32 = 5;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}
#[repr(C)]
struct Spi {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: UnicodeString,
    base_priority: i32,
    unique_process_id: isize,
    inherited_from_unique_process_id: isize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
}

// ---------------------------------------------------------------- modelo
#[derive(Clone)]
struct Proc {
    pid: u32,
    name: String,
    ws: u64,
    threads: u32,
    cpu: f32,
}

// ---- pestañas y botones ----
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Proc = 0,
    Perf = 1,
    Startup = 2,
    Services = 3,
}
const TAB_NAMES: [&str; 4] = ["Procesos", "Rendimiento", "Inicio", "Servicios"];

// Identificadores de zonas interactivas (hover/click).
const B_NONE: i32 = 0;
const B_MIN: i32 = 1;
const B_MAX: i32 = 2;
const B_CLOSE: i32 = 3;
const B_TAB0: i32 = 10; // + índice de pestaña
const B_END: i32 = 20; // Finalizar tarea
const B_NEWTASK: i32 = 21; // Nueva tarea
const B_TOGGLE: i32 = 22; // Habilitar/Deshabilitar (Inicio)

const STARTUP_COLS: [Col; 4] = [
    Col { title: "Nombre", width: 240, right: false },
    Col { title: "Estado", width: 120, right: false },
    Col { title: "Origen", width: 200, right: false },
    Col { title: "Comando", width: 0, right: false },
];

fn columns(tab: Tab) -> &'static [Col] {
    match tab {
        Tab::Startup => &STARTUP_COLS,
        _ => &PROC_COLS,
    }
}

// ---- columnas de la tabla ----
struct Col {
    title: &'static str,
    width: i32, // 0 = ocupa el resto
    right: bool,
}
const PROC_COLS: [Col; 6] = [
    Col { title: "Nombre", width: 0, right: false },
    Col { title: "PID", width: 76, right: true },
    Col { title: "CPU", width: 76, right: true },
    Col { title: "Memoria", width: 104, right: true },
    Col { title: "E/S", width: 96, right: true },
    Col { title: "Hilos", width: 64, right: true },
];

// ---------------------------------------------------------------- estado
struct State {
    hwnd: HWND,
    tab: Tab,
    procs: Vec<Proc>,
    view: Vec<usize>, // índices filtrados+ordenados
    prev_cpu: HashMap<u32, i64>, // user+kernel previos por pid
    prev_tick: u64,
    cpus: f64,
    // tabla
    sort_col: usize,
    sort_desc: bool,
    scroll: i32,
    hover_row: i32,
    sel_pid: u32,
    // interacción
    hover_btn: i32,
    filter: String,
    // layout (en píxeles de cliente)
    tab_rc: [RECT; 4],
    search_rc: RECT,
    btn_end: RECT,
    btn_newtask: RECT,
    btn_toggle: RECT,
    content: RECT,
    col_x: Vec<i32>,  // borde izquierdo de cada columna + fin
    // Inicio
    startup_items: Vec<startup::Entry>,
    startup_sel: isize,
    // resumen
    mem_used: u64,
    mem_total: u64,
    mem_load: u32,
    cpu_total: f64,
}


fn empty_rect() -> RECT {
    RECT { left: 0, top: 0, right: 0, bottom: 0 }
}
fn in_rect(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn format_bytes(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{} {}", bytes, U[0]) } else { format!("{:.1} {}", v, U[i]) }
}

// ---------------------------------------------------------------- muestreo
fn snapshot(st: &mut State) {
    let mut buf: Vec<u8> = vec![0u8; 512 * 1024];
    loop {
        let mut needed: u32 = 0;
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_PROCESS_INFORMATION_CLASS,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
                &mut needed,
            )
        };
        if status == 0xC000_0004u32 as i32 {
            buf.resize((needed as usize).max(buf.len() * 2), 0);
            continue;
        }
        if status < 0 {
            return;
        }
        break;
    }

    let now = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
    let dwall = (now.saturating_sub(st.prev_tick)) as f64; // ms
    let mut new_prev: HashMap<u32, i64> = HashMap::new();
    let mut procs = Vec::with_capacity(st.procs.len().max(64));
    let mut total_cpu = 0.0f64;

    let mut offset = 0usize;
    loop {
        let p = unsafe { &*(buf.as_ptr().add(offset) as *const Spi) };
        let pid = p.unique_process_id as u32;
        let name = if p.image_name.buffer.is_null() || p.image_name.length == 0 {
            if p.unique_process_id == 0 { "Proceso inactivo del sistema".to_string() } else { String::new() }
        } else {
            let len = (p.image_name.length / 2) as usize;
            let slice = unsafe { std::slice::from_raw_parts(p.image_name.buffer, len) };
            String::from_utf16_lossy(slice)
        };
        let times = p.user_time + p.kernel_time; // 100 ns
        let mut cpu = 0.0f32;
        if st.prev_tick != 0 && dwall > 0.0 {
            if let Some(&prev) = st.prev_cpu.get(&pid) {
                let dcpu = (times - prev).max(0) as f64 / 1e4; // ms de CPU
                cpu = (dcpu / dwall / st.cpus * 100.0) as f32;
                if pid != 0 {
                    total_cpu += dcpu / dwall / st.cpus * 100.0;
                }
            }
        }
        new_prev.insert(pid, times);
        procs.push(Proc { pid, name, ws: p.working_set_size as u64, threads: p.number_of_threads, cpu });
        if p.next_entry_offset == 0 {
            break;
        }
        offset += p.next_entry_offset as usize;
    }

    st.procs = procs;
    st.prev_cpu = new_prev;
    st.prev_tick = now;
    st.cpu_total = total_cpu.min(100.0);

    // memoria
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut m) != 0 {
            st.mem_total = m.ullTotalPhys;
            st.mem_used = m.ullTotalPhys - m.ullAvailPhys;
            st.mem_load = m.dwMemoryLoad;
        }
    }

    rebuild_view(st);
}

fn cell(p: &Proc, c: usize) -> String {
    match c {
        0 => if p.name.is_empty() { "(sin nombre)".to_string() } else { p.name.clone() },
        1 => p.pid.to_string(),
        2 => if p.cpu >= 0.05 { format!("{:.1} %", p.cpu) } else { "0 %".to_string() },
        3 => format_bytes(p.ws),
        4 => "0 KB/s".to_string(),
        5 => p.threads.to_string(),
        _ => String::new(),
    }
}

fn rebuild_view(st: &mut State) {
    let f = st.filter.to_lowercase();
    st.view = st
        .procs
        .iter()
        .enumerate()
        .filter(|(_, p)| f.is_empty() || p.name.to_lowercase().contains(&f))
        .map(|(i, _)| i)
        .collect();
    let col = st.sort_col;
    let desc = st.sort_desc;
    st.view.sort_by(|&a, &b| {
        let pa = &st.procs[a];
        let pb = &st.procs[b];
        let ord = match col {
            1 => pa.pid.cmp(&pb.pid),
            2 => pa.cpu.partial_cmp(&pb.cpu).unwrap_or(std::cmp::Ordering::Equal),
            3 => pa.ws.cmp(&pb.ws),
            5 => pa.threads.cmp(&pb.threads),
            _ => pa.name.to_lowercase().cmp(&pb.name.to_lowercase()),
        };
        if desc { ord.reverse() } else { ord }
    });
    let max = (st.view.len() as i32 - visible_rows(st)).max(0);
    st.scroll = st.scroll.clamp(0, max);
}

fn visible_rows(st: &State) -> i32 {
    (((st.content.bottom - st.content.top) - ui::s(30)) / row_h()).max(1)
}

// ---------------------------------------------------------------- layout
fn layout(st: &mut State) {
    let mut rc = empty_rect();
    unsafe { GetClientRect(st.hwnd, &mut rc) };
    let w = rc.right;
    let h = rc.bottom;

    // pestañas
    let mut tx = ui::s(12);
    for i in 0..4 {
        let tw = ui::s(24) + TAB_NAMES[i].chars().count() as i32 * ui::s(8);
        st.tab_rc[i] = RECT { left: tx, top: CAPTION_S() + ui::s(4), right: tx + tw, bottom: CAPTION_S() + ui::s(4) + ui::s(36) };
        tx += tw + ui::s(4);
    }
    // barra de herramientas
    let y = CAPTION_S() + ui::s(48);
    let bh = ui::s(32);
    st.search_rc = RECT { left: ui::s(14), top: y, right: ui::s(14) + ui::s(260), bottom: y + bh };
    st.btn_end = empty_rect();
    st.btn_newtask = empty_rect();
    st.btn_toggle = empty_rect();
    let mut rx = w - ui::s(14);
    let mut right = |width: i32, rx: &mut i32| -> RECT {
        *rx -= width;
        let r = RECT { left: *rx, top: y, right: *rx + width, bottom: y + bh };
        *rx -= ui::s(8);
        r
    };
    match st.tab {
        Tab::Proc => {
            st.btn_end = right(ui::s(132), &mut rx);
            st.btn_newtask = right(ui::s(124), &mut rx);
        }
        Tab::Startup => {
            st.btn_toggle = right(ui::s(150), &mut rx);
        }
        _ => {}
    }
    // contenido
    st.content = RECT { left: ui::s(8), top: y + bh + ui::s(10), right: w - ui::s(8), bottom: h - ui::s(8) };

    // columnas de la tabla (según la pestaña)
    let cols = columns(st.tab);
    let body_left = st.content.left + ui::s(8);
    let body_right = st.content.right - ui::s(10);
    let mut fixed = 0;
    for c in cols {
        fixed += ui::s(c.width);
    }
    let flex = (body_right - body_left - fixed - ui::s(8)).max(ui::s(120));
    st.col_x = vec![0; cols.len() + 1];
    let mut cx = body_left;
    for (i, c) in cols.iter().enumerate() {
        st.col_x[i] = cx;
        cx += if c.width == 0 { flex } else { ui::s(c.width) };
    }
    st.col_x[cols.len()] = cx;
}

#[allow(non_snake_case)]
fn CAPTION_S() -> i32 {
    ui::caption_h()
}

/// Alto de fila de las tablas (configurable en Configuración · Listas).
fn row_h() -> i32 {
    ui::s(config::current().get(Id::ListaAltoFila))
}

// ---------------------------------------------------------------- pintado
fn paint(st: &State) {
    window::paint_buffered(st.hwnd, |g: &Gfx, w, h| {
        let s = theme::style();
        let maximized = unsafe { IsZoomed(st.hwnd) != 0 };

        // marco + barra de título (DrawChrome)
        g.clear(theme::argb(0, 0, 0, 0));
        g.rounded(0.0, 0.0, w as f32, h as f32, if maximized { 0.0 } else { ui::sf(10.0) },
                  Some(s.window_bg), Some((s.card_border, 1.0)));
        let ch: Chrome = ui::gdip::chrome_rects(w);
        g.fill_rect(0, 0, w, ch.title_h, s.title_bg);
        g.hline(0, w, ch.title_h - 1, s.separator);
        // icono (recuadro con glifo)
        g.rounded(ui::s(14) as f32, ui::s(11) as f32, ui::s(20) as f32, ui::s(20) as f32, ui::sf(4.0),
                  Some(s.accent_solid), None);
        g.text_in("\u{E950}", ui::s(14) as f32, ui::s(11) as f32, ui::s(20) as f32, ui::s(20) as f32,
                  12.0, false, s.on_accent, Align::Center, Align::Center, true);
        g.text_in("Administrador de tareas", ui::s(42) as f32, 0.0, (w - ui::s(200)) as f32, ch.title_h as f32,
                  14.0, true, s.text, Align::Near, Align::Center, false);
        // botones min/max/cerrar
        chrome_button(g, &s, ch.min, "\u{E921}", st.hover_btn == B_MIN, false);
        chrome_button(g, &s, ch.max, if maximized { "\u{E923}" } else { "\u{E922}" }, st.hover_btn == B_MAX, false);
        chrome_button(g, &s, ch.close, "\u{E8BB}", st.hover_btn == B_CLOSE, true);

        // pestañas
        for i in 0..4 {
            let r = st.tab_rc[i];
            let act = i == st.tab as usize;
            let hov = st.hover_btn == B_TAB0 + i as i32;
            if hov && !act {
                g.rounded(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32,
                          ui::sf(6.0), Some(s.row_hover), None);
            }
            g.text_in(TAB_NAMES[i], r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32,
                      13.0, act, if act { s.text } else { s.text_dim }, Align::Center, Align::Center, false);
            if act {
                g.rounded((r.left + ui::s(12)) as f32, (r.bottom - ui::s(3)) as f32,
                          (r.right - r.left - ui::s(24)) as f32, ui::s(3) as f32, ui::sf(1.5),
                          Some(s.accent), None);
            }
        }
        // resumen a la derecha
        let summary = format!(
            "CPU {:.0} %   ·   Memoria {} / {} ({} %)   ·   {} procesos",
            st.cpu_total, format_bytes(st.mem_used), format_bytes(st.mem_total), st.mem_load, st.procs.len()
        );
        let last = st.tab_rc[3];
        g.text_in(&summary, (last.right + ui::s(16)) as f32, st.tab_rc[0].top as f32,
                  (w - last.right - ui::s(32)) as f32, (st.tab_rc[0].bottom - st.tab_rc[0].top) as f32,
                  11.5, false, s.text_dim, Align::Far, Align::Center, false);
        g.hline(0, w, st.tab_rc[0].bottom + ui::s(2), s.separator);

        // barra de herramientas
        if st.tab != Tab::Perf {
            draw_field(g, &s, st.search_rc);
            g.text_in("\u{E721}", (st.search_rc.left + ui::s(8)) as f32, st.search_rc.top as f32,
                      ui::s(20) as f32, (st.search_rc.bottom - st.search_rc.top) as f32,
                      12.0, false, s.text_dim, Align::Center, Align::Center, true);
            let (txt, col) = if st.filter.is_empty() {
                ("Buscar".to_string(), s.text_faint)
            } else {
                (st.filter.clone(), s.text)
            };
            g.text_in(&txt, (st.search_rc.left + ui::s(34)) as f32, st.search_rc.top as f32,
                      (st.search_rc.right - st.search_rc.left - ui::s(42)) as f32,
                      (st.search_rc.bottom - st.search_rc.top) as f32, 12.0, false, col,
                      Align::Near, Align::Center, false);
        }
        match st.tab {
            Tab::Proc => {
                draw_button(g, &s, st.btn_end, "Finalizar tarea", st.hover_btn == B_END, false, true);
                draw_button(g, &s, st.btn_newtask, "Nueva tarea", st.hover_btn == B_NEWTASK, true, false);
            }
            Tab::Startup => {
                let label = match st.startup_items.get(st.startup_sel.max(0) as usize) {
                    Some(e) if e.enabled => "Deshabilitar",
                    Some(_) => "Habilitar",
                    None => "Habilitar",
                };
                draw_button(g, &s, st.btn_toggle, label, st.hover_btn == B_TOGGLE, true, false);
            }
            _ => {}
        }

        // cuerpo según la pestaña
        match st.tab {
            Tab::Proc => paint_table(g, &s, st),
            Tab::Startup => paint_startup(g, &s, st),
            _ => {
                g.text_in(
                    "Esta pestaña se añadirá en una próxima iteración del port a Rust.",
                    st.content.left as f32, (st.content.top + ui::s(20)) as f32,
                    (st.content.right - st.content.left) as f32, ui::s(28) as f32,
                    13.0, false, s.text_faint, Align::Center, Align::Center, false);
            }
        }
    });
}

fn chrome_button(g: &Gfx, s: &theme::Style, rc: (i32, i32, i32, i32), glyph: &str, hov: bool, close: bool) {
    let (x, y, w, h) = rc;
    if hov {
        g.fill_rect(x, y, w, h, if close { s.close_hover } else { s.control_hover });
    }
    let col = if hov && close { theme::rgbc(255, 255, 255) } else { s.text_dim };
    g.text_in(glyph, x as f32, y as f32, w as f32, h as f32, 10.0, false, col, Align::Center, Align::Center, true);
}

fn draw_field(g: &Gfx, s: &theme::Style, rc: RECT) {
    g.rounded(rc.left as f32, rc.top as f32, (rc.right - rc.left) as f32, (rc.bottom - rc.top) as f32,
              ui::sf(6.0), Some(s.control_bg), Some((s.control_border, 1.0)));
}

fn draw_button(g: &Gfx, s: &theme::Style, rc: RECT, label: &str, hov: bool, primary: bool, danger: bool) {
    if rc.right <= rc.left {
        return;
    }
    let (mut fill, mut border, mut fg) = (if hov { s.control_hover } else { s.control_bg }, s.control_border, s.text);
    if primary {
        fill = if hov { s.accent_hover } else { s.accent_solid };
        border = theme::argb(0, 0, 0, 0);
        fg = s.on_accent;
    }
    if danger {
        fill = if hov { theme::mix(s.control_bg, s.danger, 0.28, 255) } else { theme::mix(s.control_bg, s.danger, 0.14, 255) };
        border = theme::with_alpha(s.danger, 130);
        fg = s.danger;
    }
    g.rounded(rc.left as f32, rc.top as f32, (rc.right - rc.left) as f32, (rc.bottom - rc.top) as f32,
              ui::sf(6.0), Some(fill), Some((border, 1.0)));
    g.text_in(label, rc.left as f32, rc.top as f32, (rc.right - rc.left) as f32, (rc.bottom - rc.top) as f32,
              13.0, false, fg, Align::Center, Align::Center, false);
}

fn paint_table(g: &Gfx, s: &theme::Style, st: &State) {
    let hx = &st.col_x;
    let header_top = st.content.top;
    let header_h = ui::s(30);
    // cabecera
    for (i, c) in PROC_COLS.iter().enumerate() {
        let mut rx = hx[i] + ui::s(2);
        let mut rw = hx[i + 1] - hx[i] - ui::s(10);
        if i == st.sort_col && c.right {
            rw -= ui::s(18);
        }
        let col = if i == st.sort_col { s.text } else { s.text_dim };
        g.text_in(c.title, rx as f32, header_top as f32, rw as f32, header_h as f32, 11.5, true, col,
                  if c.right { Align::Far } else { Align::Near }, Align::Center, false);
        let _ = &mut rx;
        if i == st.sort_col {
            let ax = hx[i + 1] - ui::s(24);
            g.text_in(if st.sort_desc { "\u{E74B}" } else { "\u{E74A}" }, ax as f32, header_top as f32,
                      ui::s(14) as f32, header_h as f32, 9.0, false, s.accent, Align::Center, Align::Center, true);
        }
    }
    g.hline(st.content.left, st.content.right, header_top + header_h - 1, s.separator);

    // filas
    let body_top = header_top + header_h;
    let body_right = st.content.right - ui::s(10);
    let vis = visible_rows(st);
    for row in 0..vis {
        let idx = (st.scroll + row) as usize;
        if idx >= st.view.len() {
            break;
        }
        let p = &st.procs[st.view[idx]];
        let ry = body_top + row * row_h();
        let rr_x = st.content.left + ui::s(2);
        let rr_w = body_right - rr_x;
        let rr_h = row_h() - ui::s(2);
        if p.pid == st.sel_pid {
            g.rounded(rr_x as f32, ry as f32, rr_w as f32, rr_h as f32, ui::sf(5.0), Some(s.row_selected), None);
        } else if row == st.hover_row {
            g.rounded(rr_x as f32, ry as f32, rr_w as f32, rr_h as f32, ui::sf(5.0), Some(s.row_hover), None);
        }
        for (c, col) in PROC_COLS.iter().enumerate() {
            let cx = hx[c] + ui::s(2);
            let cw = hx[c + 1] - hx[c] - ui::s(10);
            let color = match c {
                0 => s.text,
                2 => if p.cpu >= 50.0 { s.danger } else if p.cpu >= 15.0 { s.warning } else { s.text_dim },
                3 => if p.ws >= 1024 * 1024 * 1024 { s.warning } else { s.text_dim },
                _ => s.text_dim,
            };
            g.text_in(&cell(p, c), cx as f32, ry as f32, cw as f32, rr_h as f32, 11.5, false, color,
                      if col.right { Align::Far } else { Align::Near }, Align::Center, false);
        }
    }
    if st.view.is_empty() {
        let msg = if st.filter.is_empty() { "Cargando..." } else { "Ningún proceso coincide." };
        g.text_in(msg, st.content.left as f32, (body_top + ui::s(20)) as f32,
                  (st.content.right - st.content.left) as f32, ui::s(28) as f32, 13.0, false, s.text_faint,
                  Align::Center, Align::Center, false);
    }
}

fn paint_startup(g: &Gfx, s: &theme::Style, st: &State) {
    let cols = &STARTUP_COLS;
    let hx = &st.col_x;
    let header_top = st.content.top;
    let header_h = ui::s(30);
    for (i, c) in cols.iter().enumerate() {
        g.text_in(c.title, (hx[i] + ui::s(2)) as f32, header_top as f32,
                  (hx[i + 1] - hx[i] - ui::s(10)) as f32, header_h as f32, 11.5, true, s.text_dim,
                  Align::Near, Align::Center, false);
    }
    g.hline(st.content.left, st.content.right, header_top + header_h - 1, s.separator);
    let body_top = header_top + header_h;
    let vis = visible_rows(st);
    for row in 0..vis {
        let idx = (st.scroll + row) as usize;
        if idx >= st.startup_items.len() {
            break;
        }
        let e = &st.startup_items[idx];
        let ry = body_top + row * row_h();
        let rr_x = st.content.left + ui::s(2);
        let rr_w = st.content.right - ui::s(10) - rr_x;
        if idx as isize == st.startup_sel {
            g.rounded(rr_x as f32, ry as f32, rr_w as f32, (row_h() - ui::s(2)) as f32, ui::sf(5.0), Some(s.row_selected), None);
        }
        let vals = [
            e.name.clone(),
            if e.enabled { "Habilitado".to_string() } else { "Deshabilitado".to_string() },
            e.location.clone(),
            e.command.clone(),
        ];
        for (c, v) in vals.iter().enumerate() {
            let color = if c == 1 { if e.enabled { s.success } else { s.text_faint } } else if c == 0 { s.text } else { s.text_dim };
            g.text_in(v, (hx[c] + ui::s(2)) as f32, ry as f32, (hx[c + 1] - hx[c] - ui::s(10)) as f32,
                      (row_h() - ui::s(2)) as f32, 11.5, false, color, Align::Near, Align::Center, false);
        }
    }
    if st.startup_items.is_empty() {
        g.text_in("No hay programas de inicio.", st.content.left as f32, (body_top + ui::s(20)) as f32,
                  (st.content.right - st.content.left) as f32, ui::s(28) as f32, 13.0, false, s.text_faint,
                  Align::Center, Align::Center, false);
    }
}

fn load_startup(st: &mut State) {
    st.startup_items = startup::enumerate();
    if st.startup_sel >= st.startup_items.len() as isize {
        st.startup_sel = -1;
    }
}

fn toggle_startup(st: &mut State) {
    if let Some(e) = st.startup_items.get(st.startup_sel.max(0) as usize) {
        let want = !e.enabled;
        let e = e.clone();
        if startup::set_enabled(&e, want) == 0 {
            load_startup(st);
        }
    }
}

// ---------------------------------------------------------------- interacción
fn hit_test_btn(st: &State, x: i32, y: i32) -> i32 {
    let mut rc = empty_rect();
    unsafe { GetClientRect(st.hwnd, &mut rc) };
    let ch = ui::gdip::chrome_rects(rc.right);
    let hit = |r: (i32, i32, i32, i32)| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3;
    if hit(ch.close) {
        return B_CLOSE;
    }
    if hit(ch.max) {
        return B_MAX;
    }
    if hit(ch.min) {
        return B_MIN;
    }
    for i in 0..4 {
        if in_rect(&st.tab_rc[i], x, y) {
            return B_TAB0 + i as i32;
        }
    }
    match st.tab {
        Tab::Proc => {
            if in_rect(&st.btn_end, x, y) {
                return B_END;
            }
            if in_rect(&st.btn_newtask, x, y) {
                return B_NEWTASK;
            }
        }
        Tab::Startup => {
            if in_rect(&st.btn_toggle, x, y) {
                return B_TOGGLE;
            }
        }
        _ => {}
    }
    B_NONE
}

fn row_count(st: &State) -> i32 {
    match st.tab {
        Tab::Startup => st.startup_items.len() as i32,
        _ => st.view.len() as i32,
    }
}

fn row_at(st: &State, x: i32, y: i32) -> i32 {
    let body_top = st.content.top + ui::s(30);
    if x < st.content.left || x >= st.content.right || y < body_top || y >= st.content.bottom {
        return -1;
    }
    let r = st.scroll + (y - body_top) / row_h();
    if r < row_count(st) { r } else { -1 }
}

fn col_at(st: &State, x: i32, y: i32) -> i32 {
    let header_top = st.content.top;
    if y < header_top || y >= header_top + ui::s(30) {
        return -1;
    }
    for i in 0..PROC_COLS.len() {
        if x >= st.col_x[i] && x < st.col_x[i + 1] {
            return i as i32;
        }
    }
    -1
}

fn end_selected(st: &State) {
    if st.sel_pid == 0 {
        return;
    }
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, st.sel_pid);
        if !h.is_null() {
            TerminateProcess(h, 1);
            CloseHandle(h);
        }
    }
}

fn new_task() {
    // Lanza el diálogo de ejecución del sistema.
    let cmd = ui::wide("shell:::{2559a1f3-21d7-11d4-bdaf-00c04f60b9f0}");
    unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            ui::wide("explorer.exe").as_ptr(),
            cmd.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

fn switch_tab(st: &mut State, t: Tab) {
    if st.tab == t {
        return;
    }
    st.tab = t;
    st.filter.clear();
    st.scroll = 0;
    if t == Tab::Startup {
        load_startup(st);
    }
    layout(st);
}

// ---------------------------------------------------------------- wndproc
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let st_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    // Cambio de configuración: relee tema, fuentes, alto de fila e intervalo.
    if msg == ui::config_changed_msg() {
        let c = ui::reload_config();
        window::refresh_frame(hwnd);
        SetTimer(hwnd, 1, c.get(Id::TaskmgrRefresco) as u32, None);
        if !st_ptr.is_null() {
            layout(&mut *st_ptr);
        }
        InvalidateRect(hwnd, std::ptr::null(), 0);
        return 0;
    }
    match msg {
        WM_NCCALCSIZE => {
            if wp != 0 {
                // Marco propio: el área cliente ocupa toda la ventana.
                if IsZoomed(hwnd) != 0 {
                    let p = &mut *(lp as *mut NCCALCSIZE_PARAMS);
                    let fx = GetSystemMetrics(SM_CXFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
                    let fy = GetSystemMetrics(SM_CYFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
                    p.rgrc[0].left += fx;
                    p.rgrc[0].right -= fx;
                    p.rgrc[0].top += fy;
                    p.rgrc[0].bottom -= fy;
                }
                return 0;
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_NCHITTEST => {
            let mut pt = POINT { x: (lp & 0xFFFF) as i16 as i32, y: ((lp >> 16) & 0xFFFF) as i16 as i32 };
            ScreenToClient(hwnd, &mut pt);
            let mut rc = empty_rect();
            GetClientRect(hwnd, &mut rc);
            if IsZoomed(hwnd) == 0 {
                let b = ui::s(6);
                let (l, r, t, bo) = (pt.x < b, pt.x >= rc.right - b, pt.y < b, pt.y >= rc.bottom - b);
                if t && l { return HTTOPLEFT as LRESULT; }
                if t && r { return HTTOPRIGHT as LRESULT; }
                if bo && l { return HTBOTTOMLEFT as LRESULT; }
                if bo && r { return HTBOTTOMRIGHT as LRESULT; }
                if t { return HTTOP as LRESULT; }
                if bo { return HTBOTTOM as LRESULT; }
                if l { return HTLEFT as LRESULT; }
                if r { return HTRIGHT as LRESULT; }
            }
            if pt.y < CAPTION_S() {
                if !st_ptr.is_null() && hit_test_btn(&*st_ptr, pt.x, pt.y) != B_NONE {
                    return HTCLIENT as LRESULT;
                }
                return HTCAPTION as LRESULT;
            }
            HTCLIENT as LRESULT
        }
        WM_SIZE => {
            if !st_ptr.is_null() {
                layout(&mut *st_ptr);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_TIMER => {
            if !st_ptr.is_null() {
                snapshot(&mut *st_ptr);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            if !st_ptr.is_null() {
                paint(&*st_ptr);
            }
            0
        }
        WM_MOUSEMOVE => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                let hb = hit_test_btn(st, x, y);
                let hr = row_at(st, x, y);
                if hb != st.hover_btn || hr != st.hover_row {
                    st.hover_btn = hb;
                    st.hover_row = hr;
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                let mut tme: TRACKMOUSEEVENT = std::mem::zeroed();
                tme.cbSize = std::mem::size_of::<TRACKMOUSEEVENT>() as u32;
                tme.dwFlags = TME_LEAVE;
                tme.hwndTrack = hwnd;
                TrackMouseEvent(&mut tme);
            }
            0
        }
        WM_MOUSELEAVE => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                st.hover_btn = B_NONE;
                st.hover_row = -1;
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                let btn = hit_test_btn(st, x, y);
                match btn {
                    B_CLOSE => { PostMessageW(hwnd, WM_CLOSE, 0, 0); return 0; }
                    B_MIN => { ShowWindow(hwnd, SW_MINIMIZE); return 0; }
                    B_MAX => { ShowWindow(hwnd, if IsZoomed(hwnd) != 0 { SW_RESTORE } else { SW_MAXIMIZE }); return 0; }
                    B_END => { end_selected(st); return 0; }
                    B_NEWTASK => { new_task(); return 0; }
                    B_TOGGLE => { toggle_startup(st); InvalidateRect(hwnd, std::ptr::null(), 0); return 0; }
                    b if b >= B_TAB0 && b < B_TAB0 + 4 => {
                        let t = match b - B_TAB0 { 0 => Tab::Proc, 1 => Tab::Perf, 2 => Tab::Startup, _ => Tab::Services };
                        switch_tab(st, t);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                        return 0;
                    }
                    _ => {}
                }
                // cabecera de la tabla (ordenar)
                if st.tab == Tab::Proc {
                    let c = col_at(st, x, y);
                    if c >= 0 {
                        let c = c as usize;
                        if st.sort_col == c {
                            st.sort_desc = !st.sort_desc;
                        } else {
                            st.sort_col = c;
                            st.sort_desc = PROC_COLS[c].right;
                        }
                        rebuild_view(st);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                        return 0;
                    }
                    let r = row_at(st, x, y);
                    if r >= 0 {
                        st.sel_pid = st.procs[st.view[r as usize]].pid;
                        SetFocus(hwnd);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                } else if st.tab == Tab::Startup {
                    let r = row_at(st, x, y);
                    if r >= 0 {
                        st.startup_sel = r as isize;
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let delta = ((wp >> 16) & 0xFFFF) as i16 as i32;
                let max = (row_count(st) - visible_rows(st)).max(0);
                st.scroll = (st.scroll - (delta / 120) * 3).clamp(0, max);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_CHAR => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let ch = wp as u32;
                if ch == VK_BACK as u32 {
                    st.filter.pop();
                    rebuild_view(st);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                } else if ch >= 0x20 {
                    if let Some(c) = char::from_u32(ch) {
                        st.filter.push(c);
                        rebuild_view(st);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                }
            }
            0
        }
        WM_KEYDOWN => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let vk = wp as u32;
                if vk == VK_DELETE as u32 {
                    end_selected(st);
                } else if vk == VK_ESCAPE as u32 && !st.filter.is_empty() {
                    st.filter.clear();
                    rebuild_view(st);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            if !st_ptr.is_null() {
                drop(Box::from_raw(st_ptr));
            }
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub fn run() -> i32 {
    unsafe {
        let hinst = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null());
        ui::init(hinst);

        let class_name = ui::wide("OShellRustTaskMgr");
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

        // tamaño y posición centrada en el área de trabajo
        let mut wa = empty_rect();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut _ as *mut c_void, 0);
        let w = ui::s(1060).min(wa.right - wa.left - ui::s(40));
        let h = ui::s(700).min(wa.bottom - wa.top - ui::s(40));
        let x = wa.left + (wa.right - wa.left - w) / 2;
        let y = wa.top + (wa.bottom - wa.top - h) / 2;

        let title = ui::wide("Administrador de tareas");
        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            x, y, w, h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return 1;
        }

        let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
        let st = Box::new(State {
            hwnd,
            tab: Tab::Proc,
            procs: Vec::new(),
            view: Vec::new(),
            prev_cpu: HashMap::new(),
            prev_tick: 0,
            cpus,
            sort_col: 2,
            sort_desc: true,
            scroll: 0,
            hover_row: -1,
            sel_pid: 0,
            hover_btn: B_NONE,
            filter: String::new(),
            tab_rc: [empty_rect(); 4],
            search_rc: empty_rect(),
            btn_end: empty_rect(),
            btn_newtask: empty_rect(),
            btn_toggle: empty_rect(),
            content: empty_rect(),
            col_x: Vec::new(),
            startup_items: Vec::new(),
            startup_sel: -1,
            mem_used: 0,
            mem_total: 0,
            mem_load: 0,
            cpu_total: 0.0,
        });
        let st_raw = Box::into_raw(st);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, st_raw as isize);

        window::refresh_frame(hwnd);
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0,
                     SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
        layout(&mut *st_raw);
        snapshot(&mut *st_raw); // primer muestreo (sin %CPU)
        snapshot(&mut *st_raw); // segundo para tener %CPU inmediato no es posible aún; se llenará en el timer
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetTimer(hwnd, 1, config::current().get(Id::TaskmgrRefresco) as u32, None);
        ui::set_process_eco(false);
        if config::current().b(Id::LiberarMemoria) {
            ui::trim_working_set();
        }

        let code = window::message_loop();
        ui::shutdown();
        code
    }
}

