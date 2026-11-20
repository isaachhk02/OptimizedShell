// Explorador de archivos (--explorer [ruta]).
//
// Marco propio, barra de herramientas (atrás/adelante/arriba/actualizar), ruta
// actual, panel lateral de accesos (unidades y carpetas del usuario) y lista
// con columnas Nombre/Fecha/Tipo/Tamaño ordenables.
//
// Las operaciones sobre archivos (copiar, mover, eliminar, renombrar, carpeta
// nueva) se hacen con `IFileOperation` a través de `fileops`, igual que el
// Explorador de Windows: papelera, diálogo de progreso, confirmaciones y
// deshacer. El portapapeles usa CF_HDROP, así que se copia y se pega con el
// Explorador de Windows indistintamente, y la carpeta se revisa sola con
// `ReadDirectoryChangesW` (módulo `watcher`).
#![allow(non_snake_case, non_upper_case_globals)]

use std::collections::HashSet;
use std::ffi::c_void;

use windows_sys::Win32::Foundation::{FILETIME, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, POINT, RECT, SYSTEMTIME, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, InvalidateRect, ScreenToClient};
use windows_sys::Win32::Storage::FileSystem::{
    FindClose, FindFirstFileW, FindNextFileW, GetLogicalDrives, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_HIDDEN, WIN32_FIND_DATAW,
};
use windows_sys::Win32::System::Time::FileTimeToSystemTime;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_F2, VK_F5, VK_RETURN,
    VK_SHIFT, VK_UP,
};
use windows_sys::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, DragQueryPoint, ShellExecuteW, HDROP};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::args;
use crate::fileops::{self, Op};
use crate::ui::clipboard::{self, Effect};
use crate::ui::gdip::{Align, Chrome, Gfx};
use crate::ui::config::{self, Id};
use crate::ui::menu;
use crate::ui::{self, theme, window};
use crate::watcher::{self, Watcher};


/// Alto de fila (configurable en Configuración · Listas).
fn row_h() -> i32 {
    ui::s(config::current().get(Id::ListaAltoFila))
}

// Botones.
const B_NONE: i32 = 0;
const B_MIN: i32 = 1;
const B_MAX: i32 = 2;
const B_CLOSE: i32 = 3;
const B_BACK: i32 = 10;
const B_FWD: i32 = 11;
const B_UP: i32 = 12;
const B_REFRESH: i32 = 13;
const B_NAV0: i32 = 100; // + índice de acceso

struct FileItem {
    name: String,
    is_dir: bool,
    size: u64,
    hidden: bool,
    date: String,
    kind: String,
}

struct Place {
    name: String,
    path: String,
    glyph: &'static str,
}

struct State {
    hwnd: HWND,
    path: String,
    items: Vec<usize>, // vista ordenada -> índice en all
    all: Vec<FileItem>,
    sel: isize,
    /// Filas seleccionadas (índices de `items`), para operar con varias a la vez.
    marked: HashSet<usize>,
    /// Ancla de la selección con Mayús.
    anchor: isize,
    /// Nombre que se está editando en la fila, si se está renombrando.
    editing: Option<(usize, String)>,
    /// Vigilante de la carpeta actual.
    watch: Option<Watcher>,
    /// Último mensaje de error de una operación.
    msg: Option<String>,
    scroll: i32,
    hover: i32,
    hover_row: i32,
    sort_col: usize,
    sort_desc: bool,
    back: Vec<String>,
    fwd: Vec<String>,
    places: Vec<Place>,
    // layout
    btn: [RECT; 4], // back/fwd/up/refresh
    chrome: Chrome,
    addr_rc: RECT,
    nav_rc: RECT,
    header_rc: RECT,
    list_rc: RECT,
    status_rc: RECT,
    col_x: [i32; 5],
    nav_rects: Vec<RECT>,
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

fn from_utf16(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
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

fn format_time(ft: FILETIME) -> String {
    unsafe {
        let mut stm: SYSTEMTIME = std::mem::zeroed();
        if FileTimeToSystemTime(&ft, &mut stm) != 0 {
            format!("{:02}/{:02}/{:04} {:02}:{:02}", stm.wDay, stm.wMonth, stm.wYear, stm.wHour, stm.wMinute)
        } else {
            String::new()
        }
    }
}

fn kind_of(name: &str, is_dir: bool) -> String {
    if is_dir {
        return "Carpeta de archivos".to_string();
    }
    match name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() => format!("Archivo {}", ext.to_uppercase()),
        _ => "Archivo".to_string(),
    }
}

fn read_dir(path: &str) -> Vec<FileItem> {
    let mut out = Vec::new();
    unsafe {
        let pat = ui::wide(&format!("{}\\*", path.trim_end_matches('\\')));
        let mut fd: WIN32_FIND_DATAW = std::mem::zeroed();
        let h = FindFirstFileW(pat.as_ptr(), &mut fd);
        if h == INVALID_HANDLE_VALUE {
            return out;
        }
        loop {
            let name = from_utf16(&fd.cFileName);
            if name != "." && name != ".." {
                let is_dir = fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
                let hidden = fd.dwFileAttributes & FILE_ATTRIBUTE_HIDDEN != 0;
                let size = ((fd.nFileSizeHigh as u64) << 32) | fd.nFileSizeLow as u64;
                out.push(FileItem {
                    kind: kind_of(&name, is_dir),
                    date: format_time(fd.ftLastWriteTime),
                    name,
                    is_dir,
                    size: if is_dir { 0 } else { size },
                    hidden,
                });
            }
            if FindNextFileW(h, &mut fd) == 0 {
                break;
            }
        }
        FindClose(h);
    }
    out
}

fn rebuild(st: &mut State) {
    let cfg = config::current();
    let col = st.sort_col;
    let desc = st.sort_desc;
    let show_hidden = cfg.b(Id::ExplOcultos);
    let dirs_first = cfg.b(Id::ExplCarpetasPrimero);
    let mut v: Vec<usize> = (0..st.all.len()).filter(|&i| show_hidden || !st.all[i].hidden).collect();
    v.sort_by(|&a, &b| {
        let (ia, ib) = (&st.all[a], &st.all[b]);
        // Carpetas primero (configurable).
        if dirs_first && ia.is_dir != ib.is_dir {
            return if ia.is_dir { std::cmp::Ordering::Less } else { std::cmp::Ordering::Greater };
        }
        let ord = match col {
            1 => ia.date.cmp(&ib.date),
            2 => ia.kind.cmp(&ib.kind),
            3 => ia.size.cmp(&ib.size),
            _ => ia.name.to_lowercase().cmp(&ib.name.to_lowercase()),
        };
        if desc { ord.reverse() } else { ord }
    });
    st.items = v;
    let max = (st.items.len() as i32 - rows_visible(st)).max(0);
    st.scroll = st.scroll.clamp(0, max);
}

fn rows_visible(st: &State) -> i32 {
    ((st.list_rc.bottom - st.list_rc.top) / row_h()).max(1)
}

fn navigate(st: &mut State, path: &str, push_history: bool) {
    let p = path.trim_end_matches('\\');
    let p = if p.len() == 2 && p.as_bytes()[1] == b':' { format!("{}\\", p) } else { p.to_string() };
    if push_history && !st.path.is_empty() {
        st.back.push(st.path.clone());
        st.fwd.clear();
    }
    st.path = p;
    st.all = read_dir(&st.path);
    st.sel = -1;
    st.marked.clear();
    st.anchor = -1;
    st.editing = None;
    st.msg = None;
    st.scroll = 0;
    rebuild(st);
    // Se vigila la carpeta nueva y se deja de vigilar la anterior.
    st.watch = if config::current().b(Id::ExplAutoRefresco) {
        Watcher::start(&st.path, st.hwnd)
    } else {
        None
    };
    unsafe {
        let title = ui::wide(&format!("{} — Explorador", st.path));
        SetWindowTextW(st.hwnd, title.as_ptr());
    }
}

/// Relee la carpeta conservando selección y desplazamiento (para el refresco
/// automático: la lista no debe saltar mientras se mira).
fn reload_keep_view(st: &mut State) {
    let selected_names: Vec<String> = selected_items(st)
        .iter()
        .map(|i| st.all[*i].name.clone())
        .collect();
    let scroll = st.scroll;
    st.all = read_dir(&st.path);
    rebuild(st);
    st.marked.clear();
    st.sel = -1;
    for (row, &idx) in st.items.iter().enumerate() {
        if selected_names.iter().any(|n| *n == st.all[idx].name) {
            st.marked.insert(row);
            if st.sel < 0 {
                st.sel = row as isize;
            }
        }
    }
    let max = (st.items.len() as i32 - rows_visible(st)).max(0);
    st.scroll = scroll.clamp(0, max);
}

/// Índices en `all` de lo que está seleccionado.
fn selected_items(st: &State) -> Vec<usize> {
    let mut v: Vec<usize> = st.marked.iter().filter_map(|&r| st.items.get(r).copied()).collect();
    if v.is_empty() {
        if let Some(&i) = st.items.get(st.sel.max(0) as usize) {
            if st.sel >= 0 {
                v.push(i);
            }
        }
    }
    v
}

/// Rutas completas de lo seleccionado.
fn selected_paths(st: &State) -> Vec<String> {
    let base = st.path.trim_end_matches('\\');
    selected_items(st)
        .iter()
        .map(|&i| format!("{}\\{}", base, st.all[i].name))
        .collect()
}

fn full_path(st: &State, idx_in_all: usize) -> String {
    format!("{}\\{}", st.path.trim_end_matches('\\'), st.all[idx_in_all].name)
}

fn parent(path: &str) -> Option<String> {
    let p = path.trim_end_matches('\\');
    match p.rfind('\\') {
        Some(i) if i >= 2 => Some(p[..i].to_string()),
        Some(_) | None if p.len() > 3 => None,
        _ => None,
    }
}

fn user_folder(sub: &str) -> Option<Place> {
    let up = std::env::var("USERPROFILE").ok()?;
    let path = format!("{}\\{}", up, sub);
    Some(Place { name: sub.to_string(), path, glyph: "\u{E8B7}" })
}

fn build_places() -> Vec<Place> {
    let mut v = Vec::new();
    for (sub, name) in [("Desktop", "Escritorio"), ("Documents", "Documentos"), ("Downloads", "Descargas"),
                        ("Pictures", "Imágenes"), ("Music", "Música"), ("Videos", "Vídeos")] {
        if let Some(mut p) = user_folder(sub) {
            p.name = name.to_string();
            v.push(p);
        }
    }
    // unidades
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26 {
        if mask & (1 << i) != 0 {
            let letter = (b'A' + i as u8) as char;
            v.push(Place { name: format!("{}:\\", letter), path: format!("{}:\\", letter), glyph: "\u{EDA2}" });
        }
    }
    v
}

fn layout(st: &mut State) {
    let mut rc = er();
    unsafe { GetClientRect(st.hwnd, &mut rc) };
    let (w, h) = (rc.right, rc.bottom);
    st.chrome = ui::gdip::chrome_rects(w);
    let y0 = ui::caption_h() + ui::s(6);
    let bs = ui::s(34);
    for i in 0..4 {
        st.btn[i] = rect(ui::s(8) + i as i32 * (bs + ui::s(4)), y0, bs, bs);
    }
    let ax = st.btn[3].right + ui::s(8);
    st.addr_rc = rect(ax, y0, w - ui::s(10) - ax, bs);

    let cfg = config::current();
    let top = y0 + bs + ui::s(8);
    let status_h = ui::s(28);
    let nav_w = if cfg.b(Id::ExplPanelLateral) { ui::s(220) } else { 0 };
    st.nav_rc = rect(0, top, nav_w, h - top - status_h);
    let lx = st.nav_rc.right + if nav_w > 0 { 1 } else { ui::s(4) };
    st.header_rc = rect(lx, top, w - lx, ui::s(30));
    st.list_rc = rect(lx, top + ui::s(30), w - lx - ui::s(10), h - top - ui::s(30) - status_h);
    st.status_rc = rect(0, h - status_h, w, status_h);

    // Columnas: cada una puede ocultarse desde Configuración. Una columna
    // oculta se colapsa (col_x[i] == col_x[i+1]) y el pintado la salta.
    let size_w = if cfg.b(Id::ExplColumnaTamano) { ui::s(100) } else { 0 };
    let type_w = if cfg.b(Id::ExplColumnaTipo) { ui::s(160) } else { 0 };
    let date_w = if cfg.b(Id::ExplColumnaFecha) { ui::s(150) } else { 0 };
    st.col_x[0] = lx + ui::s(8);
    st.col_x[4] = st.list_rc.right;
    st.col_x[3] = st.col_x[4] - size_w;
    st.col_x[2] = st.col_x[3] - type_w;
    st.col_x[1] = st.col_x[2] - date_w;
    // Si no cabe el nombre, se van quitando columnas de derecha a izquierda.
    if st.col_x[1] - st.col_x[0] < ui::s(160) {
        st.col_x[1] = st.col_x[3];
        st.col_x[2] = st.col_x[3];
    }
    if st.col_x[1] - st.col_x[0] < ui::s(120) {
        st.col_x[1] = st.col_x[4];
        st.col_x[2] = st.col_x[4];
        st.col_x[3] = st.col_x[4];
    }

    st.nav_rects = if nav_w == 0 {
        Vec::new()
    } else {
        st.places.iter().enumerate()
            .map(|(i, _)| rect(ui::s(6), st.nav_rc.top + ui::s(8) + i as i32 * ui::s(30), st.nav_rc.right - ui::s(12), ui::s(30)))
            .collect()
    };
}

// ---------------------------------------------------------------- pintado
fn paint(st: &State) {
    window::paint_buffered(st.hwnd, |g: &Gfx, w, h| {
        let s = theme::style();
        let maximized = unsafe { IsZoomed(st.hwnd) != 0 };
        g.clear(theme::argb(0, 0, 0, 0));
        g.rounded(0.0, 0.0, w as f32, h as f32, if maximized { 0.0 } else { ui::sf(10.0) }, Some(s.window_bg), Some((s.card_border, 1.0)));
        g.fill_rect(0, 0, w, st.chrome.title_h, s.title_bg);
        g.rounded(ui::s(14) as f32, ui::s(11) as f32, ui::s(20) as f32, ui::s(20) as f32, ui::sf(4.0), Some(s.accent_solid), None);
        g.text_in("\u{E8B7}", ui::s(14) as f32, ui::s(11) as f32, ui::s(20) as f32, ui::s(20) as f32, 12.0, false, s.on_accent, Align::Center, Align::Center, true);
        g.text_in("Explorador de archivos", ui::s(42) as f32, 0.0, (w - ui::s(220)) as f32, st.chrome.title_h as f32, 14.0, true, s.text, Align::Near, Align::Center, false);
        chrome_btn(g, &s, st.chrome.min, "\u{E921}", st.hover == B_MIN, false);
        chrome_btn(g, &s, st.chrome.max, if maximized { "\u{E923}" } else { "\u{E922}" }, st.hover == B_MAX, false);
        chrome_btn(g, &s, st.chrome.close, "\u{E8BB}", st.hover == B_CLOSE, true);

        // barra de herramientas
        let glyphs = ["\u{E72B}", "\u{E72A}", "\u{E70E}", "\u{E72C}"];
        let enabled = [!st.back.is_empty(), !st.fwd.is_empty(), parent(&st.path).is_some(), true];
        for i in 0..4 {
            tool_btn(g, &s, st.btn[i], glyphs[i], st.hover == B_BACK + i as i32, enabled[i]);
        }
        // barra de direcciones (ruta)
        g.rounded(st.addr_rc.left as f32, st.addr_rc.top as f32, (st.addr_rc.right - st.addr_rc.left) as f32, (st.addr_rc.bottom - st.addr_rc.top) as f32, ui::sf(6.0), Some(s.control_bg), Some((s.control_border, 1.0)));
        g.text_in(&st.path, (st.addr_rc.left + ui::s(12)) as f32, st.addr_rc.top as f32, (st.addr_rc.right - st.addr_rc.left - ui::s(20)) as f32, (st.addr_rc.bottom - st.addr_rc.top) as f32, 12.5, false, s.text, Align::Near, Align::Center, false);

        // panel lateral
        g.fill_rect(st.nav_rc.left, st.nav_rc.top, st.nav_rc.right - st.nav_rc.left, st.nav_rc.bottom - st.nav_rc.top, s.nav_bg);
        for (i, r) in st.nav_rects.iter().enumerate() {
            let p = &st.places[i];
            let active = st.path.eq_ignore_ascii_case(p.path.trim_end_matches('\\')) || st.path.eq_ignore_ascii_case(&p.path);
            if active {
                g.rounded(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32, ui::sf(6.0), Some(s.row_selected), None);
            } else if st.hover == B_NAV0 + i as i32 {
                g.rounded(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32, ui::sf(6.0), Some(s.row_hover), None);
            }
            g.text_in(p.glyph, (r.left + ui::s(8)) as f32, r.top as f32, ui::s(22) as f32, (r.bottom - r.top) as f32, 12.0, false, s.accent, Align::Center, Align::Center, true);
            g.text_in(&p.name, (r.left + ui::s(34)) as f32, r.top as f32, (r.right - r.left - ui::s(38)) as f32, (r.bottom - r.top) as f32, 12.5, false, s.text, Align::Near, Align::Center, false);
        }

        // cabecera
        let titles = ["Nombre", "Fecha de modificación", "Tipo", "Tamaño"];
        for c in 0..4 {
            if st.col_x[c] == st.col_x[c + 1] {
                continue;
            }
            let col = if c == st.sort_col { s.text } else { s.text_dim };
            let right = c == 3;
            g.text_in(titles[c], (st.col_x[c] + ui::s(2)) as f32, st.header_rc.top as f32, (st.col_x[c + 1] - st.col_x[c] - ui::s(10)) as f32, (st.header_rc.bottom - st.header_rc.top) as f32, 11.5, true, col, if right { Align::Far } else { Align::Near }, Align::Center, false);
        }
        g.hline(st.header_rc.left, st.header_rc.right, st.header_rc.bottom - 1, s.separator);

        // lista
        let vis = rows_visible(st);
        for row in 0..vis {
            let idx = (st.scroll + row) as usize;
            if idx >= st.items.len() {
                break;
            }
            let it = &st.all[st.items[idx]];
            let ry = st.list_rc.top + row * row_h();
            let rr_x = st.list_rc.left + ui::s(2);
            let rr_w = st.list_rc.right - rr_x;
            let rr_h = row_h() - ui::s(2);
            let marked = st.marked.contains(&idx);
            if marked {
                g.rounded(rr_x as f32, ry as f32, rr_w as f32, rr_h as f32, ui::sf(5.0), Some(s.row_selected), None);
                if idx as isize == st.sel {
                    // La fila con el foco lleva además un borde fino.
                    g.rounded(rr_x as f32, ry as f32, rr_w as f32, rr_h as f32, ui::sf(5.0), None,
                              Some((s.focus_ring, 1.0)));
                }
            } else if row == st.hover_row {
                g.rounded(rr_x as f32, ry as f32, rr_w as f32, rr_h as f32, ui::sf(5.0), Some(s.row_hover), None);
            }
            let fg = if it.hidden { s.text_dim } else { s.text };
            // Icono real del shell si está activado; si no, un glifo.
            let isz = ui::s(18);
            let drew_icon = if config::current().b(Id::ExplIconos) {
                let hicon = crate::apps::icon(&format!("{}\\{}", st.path.trim_end_matches('\\'), it.name));
                if hicon.is_null() {
                    false
                } else {
                    g.draw_hicon(hicon, st.col_x[0], ry + (rr_h - isz) / 2, isz);
                    true
                }
            } else {
                false
            };
            if !drew_icon {
                g.text_in(if it.is_dir { "\u{E8B7}" } else { "\u{E7C3}" }, st.col_x[0] as f32, ry as f32, isz as f32, rr_h as f32, 12.0, false, if it.is_dir { s.accent } else { s.text_dim }, Align::Near, Align::Center, true);
            }
            // Nombre, o el cuadro de edición si se está renombrando esta fila.
            let name_x = st.col_x[0] + ui::s(24);
            let name_w = st.col_x[1] - st.col_x[0] - ui::s(28);
            match &st.editing {
                Some((erow, text)) if *erow == idx => {
                    g.rounded(name_x as f32, (ry + ui::s(1)) as f32, name_w as f32,
                              (rr_h - ui::s(2)) as f32, ui::sf(4.0), Some(s.control_bg),
                              Some((s.accent_solid, 1.6)));
                    g.text_in(text, (name_x + ui::s(6)) as f32, ry as f32, (name_w - ui::s(12)) as f32,
                              rr_h as f32, 12.0, false, s.text, Align::Near, Align::Center, false);
                }
                _ => {
                    g.text_in(&it.name, name_x as f32, ry as f32, name_w as f32, rr_h as f32, 12.0,
                              false, fg, Align::Near, Align::Center, false);
                }
            }
            if st.col_x[1] != st.col_x[2] {
                g.text_in(&it.date, (st.col_x[1] + ui::s(2)) as f32, ry as f32, (st.col_x[2] - st.col_x[1] - ui::s(8)) as f32, (row_h() - ui::s(2)) as f32, 11.5, false, s.text_dim, Align::Near, Align::Center, false);
            }
            if st.col_x[2] != st.col_x[3] {
                g.text_in(&it.kind, (st.col_x[2] + ui::s(2)) as f32, ry as f32, (st.col_x[3] - st.col_x[2] - ui::s(8)) as f32, (row_h() - ui::s(2)) as f32, 11.5, false, s.text_dim, Align::Near, Align::Center, false);
            }
            if st.col_x[3] != st.col_x[4] {
                let sz = if it.is_dir { String::new() } else { format_bytes(it.size) };
                g.text_in(&sz, (st.col_x[3] + ui::s(2)) as f32, ry as f32, (st.col_x[4] - st.col_x[3] - ui::s(10)) as f32, (row_h() - ui::s(2)) as f32, 11.5, false, s.text_dim, Align::Far, Align::Center, false);
            }
        }

        // barra de estado
        g.hline(0, w, st.status_rc.top, s.separator);
        let n = st.items.len();
        let sel = st.marked.len();
        let status = if sel > 0 {
            let bytes: u64 = st.marked.iter()
                .filter_map(|r| st.items.get(*r))
                .map(|&i| st.all[i].size)
                .sum();
            if bytes > 0 {
                format!("{} de {} seleccionados · {}", sel, n, format_bytes(bytes))
            } else {
                format!("{} de {} seleccionados", sel, n)
            }
        } else {
            format!("{} elemento{}", n, if n == 1 { "" } else { "s" })
        };
        let sh = st.status_rc.bottom - st.status_rc.top;
        g.text_in(&status, ui::s(12) as f32, st.status_rc.top as f32, (w / 2) as f32, sh as f32, 11.5, false, s.text_dim, Align::Near, Align::Center, false);
        // Último error de una operación, a la derecha.
        if let Some(m) = &st.msg {
            g.text_in(m, (w / 2) as f32, st.status_rc.top as f32, (w / 2 - ui::s(12)) as f32,
                      sh as f32, 11.0, false, s.danger, Align::Far, Align::Center, false);
        } else if clipboard::has_files() {
            g.text_in("Hay archivos en el portapapeles · Ctrl+V para pegar", (w / 2) as f32,
                      st.status_rc.top as f32, (w / 2 - ui::s(12)) as f32, sh as f32, 11.0, false,
                      s.text_faint, Align::Far, Align::Center, false);
        }
    });
}

fn chrome_btn(g: &Gfx, s: &theme::Style, rc: (i32, i32, i32, i32), glyph: &str, hov: bool, close: bool) {
    let (x, y, w, h) = rc;
    if hov {
        g.fill_rect(x, y, w, h, if close { s.close_hover } else { s.control_hover });
    }
    let col = if hov && close { theme::rgbc(255, 255, 255) } else { s.text_dim };
    g.text_in(glyph, x as f32, y as f32, w as f32, h as f32, 10.0, false, col, Align::Center, Align::Center, true);
}

fn tool_btn(g: &Gfx, s: &theme::Style, r: RECT, glyph: &str, hov: bool, enabled: bool) {
    if hov && enabled {
        g.rounded(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32, ui::sf(6.0), Some(s.control_hover), None);
    }
    let col = if !enabled { theme::with_alpha(s.text_faint, 140) } else if hov { s.text } else { s.text_dim };
    g.text_in(glyph, r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32, 12.0, false, col, Align::Center, Align::Center, true);
}

// ---------------------------------------------------------------- interacción
fn hit(st: &State, x: i32, y: i32) -> i32 {
    let c = &st.chrome;
    let h = |r: (i32, i32, i32, i32)| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3;
    if h(c.close) {
        return B_CLOSE;
    }
    if h(c.max) {
        return B_MAX;
    }
    if h(c.min) {
        return B_MIN;
    }
    for i in 0..4 {
        if in_rect(&st.btn[i], x, y) {
            return B_BACK + i as i32;
        }
    }
    for (i, r) in st.nav_rects.iter().enumerate() {
        if in_rect(r, x, y) {
            return B_NAV0 + i as i32;
        }
    }
    B_NONE
}

fn row_at(st: &State, x: i32, y: i32) -> i32 {
    if !in_rect(&st.list_rc, x, y) {
        return -1;
    }
    let r = st.scroll + (y - st.list_rc.top) / row_h();
    if (r as usize) < st.items.len() { r } else { -1 }
}

fn open_item(st: &mut State, idx: usize) {
    let it = &st.all[st.items[idx]];
    if it.is_dir {
        let p = format!("{}\\{}", st.path.trim_end_matches('\\'), it.name);
        navigate(st, &p, true);
        unsafe { InvalidateRect(st.hwnd, std::ptr::null(), 0) };
    } else {
        let full = format!("{}\\{}", st.path.trim_end_matches('\\'), it.name);
        unsafe {
            ShellExecuteW(std::ptr::null_mut(), ui::wide("open").as_ptr(), ui::wide(&full).as_ptr(),
                          std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
        }
    }
}

// ------------------------------------------------- operaciones con archivos
/// Aplica el resultado de una operación: deja el mensaje si falló y relee.
fn after_op(st: &mut State, r: fileops::OpResult) {
    st.msg = match r {
        Ok(_) => None,
        Err(e) => Some(e),
    };
    reload_keep_view(st);
    unsafe { InvalidateRect(st.hwnd, std::ptr::null(), 0) };
}

fn copy_selection(st: &mut State, cut: bool) {
    let paths = selected_paths(st);
    if paths.is_empty() {
        return;
    }
    clipboard::set_files(&paths, if cut { Effect::Move } else { Effect::Copy });
    st.msg = None;
    unsafe { InvalidateRect(st.hwnd, std::ptr::null(), 0) };
}

fn paste_here(st: &mut State) {
    let Some((paths, effect)) = clipboard::get_files() else { return };
    let dest = st.path.trim_end_matches('\\').to_string();
    let kind = if effect == Effect::Move { Op::Move } else { Op::Copy };

    // Lo que ya está en esta carpeta se duplica con otro nombre; cortar y
    // pegar en el mismo sitio no hace nada, como en Windows.
    let same_dir = |p: &String| {
        std::path::Path::new(p)
            .parent()
            .map(|d| d.to_string_lossy().trim_end_matches('\\').eq_ignore_ascii_case(&dest))
            .unwrap_or(false)
    };
    let (here, elsewhere): (Vec<String>, Vec<String>) = paths.into_iter().partition(same_dir);

    let mut result: fileops::OpResult = Ok(false);
    if !elsewhere.is_empty() {
        result = fileops::transfer(st.hwnd, &elsewhere, &dest, kind);
    }
    if kind == Op::Copy {
        for p in &here {
            let name = std::path::Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let new_name = fileops::free_copy_name(&dest, &name);
            let r = fileops::copy_as(st.hwnd, p, &dest, &new_name);
            if r.is_err() {
                result = r;
            }
        }
    }
    after_op(st, result);
}

fn delete_selection(st: &mut State, permanent: bool) {
    let paths = selected_paths(st);
    if paths.is_empty() {
        return;
    }
    // La confirmación la pone el ajuste «Confirmar al eliminar»; el shell
    // añade la suya cuando Windows la tiene configurada.
    if !fileops::confirm_delete(st.hwnd, &paths, permanent) {
        return;
    }
    let r = fileops::delete(st.hwnd, &paths, permanent);
    after_op(st, r);
}

fn begin_rename(st: &mut State) {
    let Some(&idx) = st.items.get(st.sel.max(0) as usize) else { return };
    if st.sel < 0 {
        return;
    }
    st.editing = Some((st.sel as usize, st.all[idx].name.clone()));
    unsafe { InvalidateRect(st.hwnd, std::ptr::null(), 0) };
}

fn commit_rename(st: &mut State) {
    let Some((row, name)) = st.editing.take() else { return };
    let Some(&idx) = st.items.get(row) else { return };
    let old = st.all[idx].name.clone();
    if name.trim().is_empty() || name == old {
        unsafe { InvalidateRect(st.hwnd, std::ptr::null(), 0) };
        return;
    }
    let path = full_path(st, idx);
    let r = fileops::rename(st.hwnd, &path, &name);
    after_op(st, r);
}

fn create_folder(st: &mut State) {
    // El shell elige un nombre libre («Nueva carpeta», «Nueva carpeta (2)»...).
    let parent = st.path.clone();
    let r = fileops::new_folder(st.hwnd, &parent, "Nueva carpeta");
    after_op(st, r);
}

/// Menú contextual de la lista. El del shell (con «Abrir con», «Enviar a» y
/// lo que añadan los programas) se ofrece como una entrada más: es un menú del
/// sistema y no sigue el tema, así que no se pone por delante del propio.
fn list_menu(st: &mut State, x: i32, y: i32) {
    let paths = selected_paths(st);
    let has_sel = !paths.is_empty();
    let one = paths.len() == 1;
    let is_dir = selected_items(st).first().map(|&i| st.all[i].is_dir).unwrap_or(false);
    let can_paste = clipboard::has_files();

    let items = vec![
        menu::Item::new(1, if is_dir { "Abrir" } else { "Abrir" })
            .glyph("\u{E8E5}").enabled(one),
        menu::Item::new(2, "Abrir en una ventana nueva").glyph("\u{E8A7}").enabled(one && is_dir),
        menu::Item::sep(),
        menu::Item::new(3, "Cortar").glyph("\u{E8C6}").enabled(has_sel),
        menu::Item::new(4, "Copiar").glyph("\u{E8C8}").enabled(has_sel),
        menu::Item::new(5, "Pegar").glyph("\u{E77F}").enabled(can_paste),
        menu::Item::sep(),
        menu::Item::new(6, "Cambiar nombre").glyph("\u{E8AC}").hint("F2").enabled(one),
        menu::Item::new(7, "Eliminar").glyph("\u{E74D}").hint("Supr").enabled(has_sel).danger(),
        menu::Item::sep(),
        menu::Item::new(8, "Carpeta nueva").glyph("\u{E8F4}"),
        menu::Item::new(9, "Copiar la ruta").glyph("\u{E8C8}").enabled(has_sel),
        menu::Item::sep(),
        menu::Item::new(10, "Más opciones de Windows").glyph("\u{E712}").enabled(has_sel),
        menu::Item::new(11, "Propiedades").glyph("\u{E946}").hint("Alt+Entrar").enabled(one),
    ];
    match menu::popup(st.hwnd, x, y, items) {
        1 => {
            if st.sel >= 0 {
                open_item(st, st.sel as usize);
            }
        }
        2 => {
            if let Some(p) = paths.first() {
                crate::shell::launch_own(&format!("--explorer \"{}\"", p));
            }
        }
        3 => copy_selection(st, true),
        4 => copy_selection(st, false),
        5 => paste_here(st),
        6 => begin_rename(st),
        7 => delete_selection(st, false),
        8 => create_folder(st),
        9 => {
            ui::set_clipboard_text(&paths.join("\r\n"));
        }
        10 => {
            // El menú del shell, con sus extensiones.
            fileops::shell_context_menu(st.hwnd, &paths, x, y);
            reload_keep_view(st);
            unsafe { InvalidateRect(st.hwnd, std::ptr::null(), 0) };
        }
        11 => {
            if let Some(p) = paths.first() {
                fileops::show_properties(st.hwnd, p);
            }
        }
        _ => {}
    }
}

fn go_back(st: &mut State) {
    if let Some(p) = st.back.pop() {
        st.fwd.push(st.path.clone());
        navigate(st, &p, false);
    }
}
fn go_fwd(st: &mut State) {
    if let Some(p) = st.fwd.pop() {
        st.back.push(st.path.clone());
        navigate(st, &p, false);
    }
}
fn go_up(st: &mut State) {
    if let Some(p) = parent(&st.path) {
        navigate(st, &p, true);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let st_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    macro_rules! st {
        () => { &mut *st_ptr };
    }
    // Cambio de configuración: relee, recalcula tema/fuentes y vuelve a medir.
    if msg == ui::config_changed_msg() {
        ui::reload_config();
        window::refresh_frame(hwnd);
        if !st_ptr.is_null() {
            let st = st!();
            rebuild(st);
            layout(st);
        }
        InvalidateRect(hwnd, std::ptr::null(), 0);
        return 0;
    }
    match msg {
        WM_NCCALCSIZE if wp != 0 => {
            if IsZoomed(hwnd) != 0 {
                let p = &mut *(lp as *mut NCCALCSIZE_PARAMS);
                let fx = GetSystemMetrics(SM_CXFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
                let fy = GetSystemMetrics(SM_CYFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
                p.rgrc[0].left += fx;
                p.rgrc[0].right -= fx;
                p.rgrc[0].top += fy;
                p.rgrc[0].bottom -= fy;
            }
            0
        }
        WM_NCHITTEST => {
            let mut pt = POINT { x: (lp & 0xFFFF) as i16 as i32, y: ((lp >> 16) & 0xFFFF) as i16 as i32 };
            ScreenToClient(hwnd, &mut pt);
            let mut rc = er();
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
            if pt.y < ui::caption_h() {
                if !st_ptr.is_null() && hit(&*st_ptr, pt.x, pt.y) != B_NONE {
                    return HTCLIENT as LRESULT;
                }
                return HTCAPTION as LRESULT;
            }
            HTCLIENT as LRESULT
        }
        WM_SIZE => {
            if !st_ptr.is_null() {
                layout(st!());
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
                let st = st!();
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                let hb = hit(st, x, y);
                let hr = row_at(st, x, y);
                if hb != st.hover || hr != st.hover_row {
                    st.hover = hb;
                    st.hover_row = hr;
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !st_ptr.is_null() {
                let st = st!();
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                let b = hit(st, x, y);
                match b {
                    B_CLOSE => { PostMessageW(hwnd, WM_CLOSE, 0, 0); return 0; }
                    B_MIN => { ShowWindow(hwnd, SW_MINIMIZE); return 0; }
                    B_MAX => { ShowWindow(hwnd, if IsZoomed(hwnd) != 0 { SW_RESTORE } else { SW_MAXIMIZE }); return 0; }
                    B_BACK => { go_back(st); InvalidateRect(hwnd, std::ptr::null(), 0); return 0; }
                    B_FWD => { go_fwd(st); InvalidateRect(hwnd, std::ptr::null(), 0); return 0; }
                    B_UP => { go_up(st); InvalidateRect(hwnd, std::ptr::null(), 0); return 0; }
                    B_REFRESH => { let p = st.path.clone(); navigate(st, &p, false); InvalidateRect(hwnd, std::ptr::null(), 0); return 0; }
                    bn if bn >= B_NAV0 => {
                        let p = st.places[(bn - B_NAV0) as usize].path.clone();
                        navigate(st, &p, true);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                        return 0;
                    }
                    _ => {}
                }
                // cabecera: ordenar
                if in_rect(&st.header_rc, x, y) {
                    for c in 0..4 {
                        if st.col_x[c] != st.col_x[c + 1] && x >= st.col_x[c] && x < st.col_x[c + 1] {
                            if st.sort_col == c {
                                st.sort_desc = !st.sort_desc;
                            } else {
                                st.sort_col = c;
                                st.sort_desc = c == 3;
                            }
                            rebuild(st);
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                            return 0;
                        }
                    }
                }
                let r = row_at(st, x, y);
                if r >= 0 {
                    let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
                    let shift = GetKeyState(VK_SHIFT as i32) < 0;
                    let row = r as usize;
                    if ctrl {
                        // Añade o quita esta fila de la selección.
                        if !st.marked.remove(&row) {
                            st.marked.insert(row);
                        }
                        st.anchor = r as isize;
                    } else if shift && st.anchor >= 0 {
                        // Rango desde el ancla.
                        st.marked.clear();
                        let (a, b) = (st.anchor.min(r as isize), st.anchor.max(r as isize));
                        for i in a..=b {
                            st.marked.insert(i as usize);
                        }
                    } else {
                        st.marked.clear();
                        st.marked.insert(row);
                        st.anchor = r as isize;
                    }
                    st.sel = r as isize;
                    st.editing = None;
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                } else if in_rect(&st.list_rc, x, y) {
                    // Pulsar el hueco vacío deselecciona.
                    st.marked.clear();
                    st.sel = -1;
                    st.editing = None;
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_RBUTTONUP => {
            if !st_ptr.is_null() {
                let st = st!();
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                if in_rect(&st.list_rc, x, y) {
                    let r = row_at(st, x, y);
                    if r >= 0 && !st.marked.contains(&(r as usize)) {
                        st.marked.clear();
                        st.marked.insert(r as usize);
                        st.sel = r as isize;
                        st.anchor = r as isize;
                    } else if r < 0 {
                        st.marked.clear();
                        st.sel = -1;
                    }
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                    let mut pt = POINT { x, y };
                    ClientToScreen(hwnd, &mut pt);
                    list_menu(st, pt.x, pt.y);
                }
            }
            0
        }
        WM_DROPFILES => {
            // Arrastrar archivos a la ventana: se copian aquí (o se mueven con Mayús).
            if !st_ptr.is_null() {
                let st = st!();
                let drop = wp as HDROP;
                let n = DragQueryFileW(drop, 0xFFFF_FFFF, std::ptr::null_mut(), 0);
                let mut paths = Vec::with_capacity(n as usize);
                for i in 0..n {
                    let len = DragQueryFileW(drop, i, std::ptr::null_mut(), 0);
                    if len == 0 {
                        continue;
                    }
                    let mut buf = vec![0u16; len as usize + 1];
                    DragQueryFileW(drop, i, buf.as_mut_ptr(), buf.len() as u32);
                    paths.push(from_utf16(&buf));
                }
                let mut pt = POINT { x: 0, y: 0 };
                DragQueryPoint(drop, &mut pt);
                DragFinish(drop);
                if !paths.is_empty() {
                    // Si se suelta sobre una carpeta de la lista, va dentro.
                    let r = row_at(st, pt.x, pt.y);
                    let dest = match r {
                        r if r >= 0 => {
                            let idx = st.items[r as usize];
                            if st.all[idx].is_dir { full_path(st, idx) } else { st.path.clone() }
                        }
                        _ => st.path.clone(),
                    };
                    let shift = GetKeyState(VK_SHIFT as i32) < 0;
                    let kind = if shift { Op::Move } else { Op::Copy };
                    let res = fileops::transfer(hwnd, &paths, &dest, kind);
                    after_op(st, res);
                }
            }
            0
        }
        watcher::WM_DIR_CHANGED => {
            // La carpeta ha cambiado: se relee sin perder la selección.
            if !st_ptr.is_null() {
                let st = st!();
                if st.editing.is_none() {
                    reload_keep_view(st);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_LBUTTONDBLCLK => {
            if !st_ptr.is_null() {
                let st = st!();
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                let r = row_at(st, x, y);
                if r >= 0 {
                    open_item(st, r as usize);
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            if !st_ptr.is_null() {
                let st = st!();
                let delta = ((wp >> 16) & 0xFFFF) as i16 as i32;
                let max = (st.items.len() as i32 - rows_visible(st)).max(0);
                st.scroll = (st.scroll - (delta / 120) * 3).clamp(0, max);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_CHAR => {
            // Escribir mientras se renombra.
            if !st_ptr.is_null() {
                let st = st!();
                if let Some((_, name)) = st.editing.as_mut() {
                    let ch = wp as u32;
                    if ch == VK_BACK as u32 {
                        name.pop();
                    } else if ch >= 0x20 {
                        if let Some(c) = char::from_u32(ch) {
                            // Caracteres que Windows no admite en un nombre.
                            if !r#"\/:*?"<>|"#.contains(c) {
                                name.push(c);
                            }
                        }
                    }
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_KEYDOWN => {
            if !st_ptr.is_null() {
                let st = st!();
                let vk = wp as u32;
                let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
                let shift = GetKeyState(VK_SHIFT as i32) < 0;

                // Mientras se renombra, el teclado es para el nombre.
                if st.editing.is_some() {
                    if vk == VK_RETURN as u32 {
                        commit_rename(st);
                    } else if vk == VK_ESCAPE as u32 {
                        st.editing = None;
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                    return 0;
                }
                if ctrl {
                    match vk as u8 as char {
                        'C' => copy_selection(st, false),
                        'X' => copy_selection(st, true),
                        'V' => paste_here(st),
                        'A' => {
                            st.marked = (0..st.items.len()).collect();
                            st.sel = 0;
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        }
                        'N' => create_folder(st),
                        _ => {}
                    }
                    return 0;
                }
                if vk == VK_BACK as u32 {
                    go_up(st);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                } else if vk == VK_F5 as u32 {
                    let p = st.path.clone();
                    navigate(st, &p, false);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                } else if vk == VK_F2 as u32 {
                    begin_rename(st);
                } else if vk == VK_DELETE as u32 {
                    // Mayús+Supr elimina sin pasar por la papelera.
                    delete_selection(st, shift);
                } else if vk == VK_RETURN as u32 && st.sel >= 0 {
                    open_item(st, st.sel as usize);
                } else if vk == VK_UP as u32 || vk == VK_DOWN as u32 {
                    let d = if vk == VK_DOWN as u32 { 1 } else { -1 };
                    let n = st.items.len() as isize;
                    if n > 0 {
                        st.sel = (st.sel + d).clamp(0, n - 1);
                        if !shift {
                            st.marked.clear();
                            st.anchor = st.sel;
                        }
                        st.marked.insert(st.sel as usize);
                        // Mantiene la fila a la vista.
                        let vis = rows_visible(st);
                        if st.sel < st.scroll as isize {
                            st.scroll = st.sel as i32;
                        } else if st.sel >= (st.scroll + vis) as isize {
                            st.scroll = st.sel as i32 - vis + 1;
                        }
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
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

        let start = args::positional()
            .filter(|p| std::path::Path::new(p).exists())
            .unwrap_or_else(|| std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".to_string()));

        let class_name = ui::wide("OShellRustExplorer");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
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

        let mut wa = er();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut _ as *mut c_void, 0);
        let w = ui::s(1040).min(wa.right - wa.left - ui::s(40));
        let h = ui::s(660).min(wa.bottom - wa.top - ui::s(40));
        let x = wa.left + (wa.right - wa.left - w) / 2;
        let y = wa.top + (wa.bottom - wa.top - h) / 2;

        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            class_name.as_ptr(),
            ui::wide("Explorador de archivos").as_ptr(),
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
        let st = Box::new(State {
            hwnd,
            path: String::new(),
            items: Vec::new(),
            all: Vec::new(),
            sel: -1,
            marked: HashSet::new(),
            anchor: -1,
            editing: None,
            watch: None,
            msg: None,
            scroll: 0,
            hover: B_NONE,
            hover_row: -1,
            sort_col: 0,
            sort_desc: false,
            back: Vec::new(),
            fwd: Vec::new(),
            places: build_places(),
            btn: [er(); 4],
            chrome: ui::gdip::chrome_rects(w),
            addr_rc: er(),
            nav_rc: er(),
            header_rc: er(),
            list_rc: er(),
            status_rc: er(),
            col_x: [0; 5],
            nav_rects: Vec::new(),
        });
        let raw = Box::into_raw(st);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
        // COM para IFileOperation y el menú del shell, y arrastrar y soltar.
        fileops::init_com();
        ui::menu::register(hinst);
        DragAcceptFiles(hwnd, 1);
        window::refresh_frame(hwnd);
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
        layout(&mut *raw);
        navigate(&mut *raw, &start, false);
        layout(&mut *raw);
        ShowWindow(hwnd, SW_SHOWNORMAL);
        // El explorador es interactivo: no se marca como EcoQoS, pero sí
        // respeta el ajuste de liberación de memoria.
        ui::set_process_eco(false);
        if config::current().b(Id::LiberarMemoria) {
            ui::trim_working_set();
        }

        let code = window::message_loop();
        ui::shutdown();
        code
    }
}


