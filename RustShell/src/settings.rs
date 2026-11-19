// Configuración (--settings).
//
// La ventana no tiene los controles escritos a mano: recorre la tabla de
// ajustes de `ui::config` y dibuja un control por entrada según su tipo
// (interruptor, deslizador con pasos, control segmentado, tarjetas de modo o
// selector de color). Añadir un ajuste nuevo a `config::ITEMS` lo hace aparecer
// aquí sin tocar este archivo.
//
// Cada cambio se aplica al momento, se guarda en %APPDATA%\OptimizedShell\
// config.ini y se difunde al resto de ventanas de OptimizedShell.
#![allow(non_snake_case, non_upper_case_globals)]

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, ScreenToClient};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RIGHT, VK_UP};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::args;
use crate::ui::config::{self, Config, Id, Item, Kind, Page};
use crate::ui::gdip::{Align, Gfx};
use crate::ui::theme::{self, ACCENT_PRESETS};
use crate::ui::{self, window};

// ------------------------------------------------------------ navegación
struct NavEntry {
    glyph: &'static str,
    title: &'static str,
    page: Option<Page>,
    /// Nombre admitido como argumento posicional (`--settings <arg>`).
    arg: &'static str,
}

const NAV: &[NavEntry] = &[
    NavEntry { glyph: "\u{E771}", title: "Personalización", page: Some(Page::Apariencia), arg: "personalizacion" },
    NavEntry { glyph: "\u{E7F4}", title: "Barra de tareas", page: Some(Page::Barra), arg: "barra" },
    NavEntry { glyph: "\u{E700}", title: "Menú Inicio", page: Some(Page::Menu), arg: "menu" },
    NavEntry { glyph: "\u{E8B7}", title: "Listas y explorador", page: Some(Page::Listas), arg: "listas" },
    NavEntry { glyph: "\u{E9D9}", title: "Rendimiento", page: Some(Page::Rendimiento), arg: "rendimiento" },
    NavEntry { glyph: "\u{E91B}", title: "Fondo de escritorio", page: None, arg: "fondo" },
    NavEntry { glyph: "\u{E7F8}", title: "Pantalla", page: None, arg: "pantalla" },
    NavEntry { glyph: "\u{E702}", title: "Bluetooth", page: None, arg: "bluetooth" },
];

// ------------------------------------------------------------ zonas sensibles
#[derive(Clone, Copy, PartialEq)]
enum Hit {
    None,
    Min,
    Close,
    Nav(usize),
    /// Interruptor de un ajuste booleano.
    Toggle(Id),
    /// Opción de un control segmentado o de unas tarjetas de modo.
    Choice(Id, i32),
    /// Deslizador: canal 0 = valor; 1/2/3 = R/G/B de un color.
    Slider(Id, u8),
    /// Botones − / + de un entero.
    Step(Id, i32),
    /// Muestra de color predefinida.
    Swatch(usize),
    WinAccent,
    ResetPage,
    ResetAll,
    OpenIni,
}

struct Block {
    item: &'static Item,
    rect: RECT,
}

struct State {
    hwnd: HWND,
    page: usize,
    hover: Hit,
    scroll: i32,
    content_h: i32,
    /// Ajuste que se está arrastrando (deslizador) y su canal.
    drag: Option<(Id, u8)>,
    // layout
    x0: i32,
    cw: i32,
    content_rc: RECT,
    nav: Vec<RECT>,
    blocks: Vec<Block>,
    hits: Vec<(RECT, Hit)>,
    min_rc: RECT,
    close_rc: RECT,
    reset_page_rc: RECT,
    reset_all_rc: RECT,
    open_ini_rc: RECT,
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

// ------------------------------------------------- geometría de los controles
// Las mismas funciones se usan al medir (layout) y al dibujar (paint), para que
// lo que se ve y lo que se puede pulsar no se separen nunca.

fn has_help(it: &Item) -> bool {
    !it.help.is_empty()
}

/// Primera línea libre dentro de la tarjeta de un ajuste.
fn body_top(r: &RECT, it: &Item) -> i32 {
    r.top + if has_help(it) { ui::s(48) } else { ui::s(34) }
}

fn switch_rect(r: &RECT) -> RECT {
    let w = ui::s(44);
    let h = ui::s(22);
    rect(r.right - ui::s(16) - w, r.top + (rh(r) - h) / 2, w, h)
}

fn track_rect(r: &RECT) -> RECT {
    rect(r.left + ui::s(16), r.bottom - ui::s(20), rw(r) - ui::s(32), ui::s(6))
}

fn stepper_rects(r: &RECT) -> (RECT, RECT) {
    let b = ui::s(24);
    let y = r.top + ui::s(10);
    (rect(r.right - ui::s(16) - b * 2 - ui::s(6), y, b, b), rect(r.right - ui::s(16) - b, y, b, b))
}

fn chip_rects(r: &RECT, opts: &[&str], it: &Item) -> Vec<RECT> {
    let x0 = r.left + ui::s(16);
    let max_x = r.right - ui::s(16);
    let h = ui::s(28);
    let gap = ui::s(8);
    let mut x = x0;
    let mut y = body_top(r, it);
    let mut out = Vec::with_capacity(opts.len());
    for o in opts {
        let w = ui::s(22) + o.chars().count() as i32 * ui::s(7);
        if x + w > max_x && x > x0 {
            x = x0;
            y += h + gap;
        }
        out.push(rect(x, y, w, h));
        x += w + gap;
    }
    out
}

fn card_rects(r: &RECT) -> (RECT, RECT) {
    let gap = ui::s(16);
    let w = (rw(r) - gap) / 2;
    let h = ui::s(150);
    let y = r.top + ui::s(10);
    (rect(r.left, y, w, h), rect(r.left + w + gap, y, w, h))
}

fn swatch_rects(r: &RECT) -> Vec<RECT> {
    let sw = ui::s(34);
    let gap = ui::s(10);
    let x0 = r.left + ui::s(16);
    let max_x = r.right - ui::s(16);
    let mut x = x0;
    let mut y = r.top + ui::s(38);
    let mut out = Vec::new();
    for _ in 0..ACCENT_PRESETS.len() {
        if x + sw > max_x && x > x0 {
            x = x0;
            y += sw + gap;
        }
        out.push(rect(x, y, sw, sw));
        x += sw + gap;
    }
    out
}

/// Pistas R/G/B del selector de color, bajo las muestras.
fn color_tracks(r: &RECT) -> [RECT; 3] {
    let y0 = swatch_rects(r).last().map(|s| s.bottom).unwrap_or(r.top) + ui::s(26);
    let w = rw(r) - ui::s(32) - ui::s(60);
    let step = ui::s(34);
    [
        rect(r.left + ui::s(16) + ui::s(28), y0, w, ui::s(6)),
        rect(r.left + ui::s(16) + ui::s(28), y0 + step, w, ui::s(6)),
        rect(r.left + ui::s(16) + ui::s(28), y0 + step * 2, w, ui::s(6)),
    ]
}

fn winaccent_rect(r: &RECT) -> RECT {
    let t = color_tracks(r)[2];
    rect(r.left + ui::s(16), t.bottom + ui::s(18), ui::s(236), ui::s(32))
}

/// Alto que ocupa el bloque de un ajuste (el ancho se toma de `probe`).
fn block_height(it: &Item, cw: i32) -> i32 {
    let probe = rect(0, 0, cw, 0);
    match it.kind {
        Kind::Bool => ui::s(if has_help(it) { 62 } else { 50 }),
        Kind::Int { .. } => ui::s(if has_help(it) { 92 } else { 76 }),
        Kind::Choice(opts) => {
            let rs = chip_rects(&probe, opts, it);
            rs.last().map(|r| r.bottom).unwrap_or(0) + ui::s(14)
        }
        Kind::Cards(_) => ui::s(172),
        Kind::Color => winaccent_rect(&probe).bottom + ui::s(14),
    }
}

// ---------------------------------------------------------------- layout
fn layout(st: &mut State) {
    let mut rc = er();
    unsafe { GetClientRect(st.hwnd, &mut rc) };
    let w = rc.right;
    let h = rc.bottom;
    let cap = ui::caption_h();
    let ch = ui::gdip::chrome_rects(w);
    st.min_rc = rect(ch.min.0, ch.min.1, ch.min.2, ch.min.3);
    st.close_rc = rect(ch.close.0, ch.close.1, ch.close.2, ch.close.3);

    let nav_w = ui::s(226);
    st.nav = (0..NAV.len())
        .map(|i| rect(ui::s(10), cap + ui::s(16) + i as i32 * ui::s(42), nav_w - ui::s(20), ui::s(38)))
        .collect();

    st.x0 = nav_w + ui::s(24);
    st.cw = (w - st.x0 - ui::s(24)).max(ui::s(240));
    let footer_h = ui::s(58);
    let top = cap + ui::s(56);
    st.content_rc = rect(st.x0, top, st.cw, (h - top - footer_h).max(ui::s(80)));

    // Botones del pie.
    let by = h - ui::s(46);
    let bw = ui::s(170);
    st.reset_page_rc = rect(st.x0, by, bw, ui::s(32));
    st.reset_all_rc = rect(st.x0 + bw + ui::s(10), by, ui::s(140), ui::s(32));
    st.open_ini_rc = rect(st.x0 + bw + ui::s(160), by, ui::s(150), ui::s(32));

    // Bloques de la página activa.
    st.blocks.clear();
    st.hits.clear();
    st.hits.push((st.close_rc, Hit::Close));
    st.hits.push((st.min_rc, Hit::Min));
    for (i, r) in st.nav.iter().enumerate() {
        st.hits.push((*r, Hit::Nav(i)));
    }
    st.hits.push((st.reset_page_rc, Hit::ResetPage));
    st.hits.push((st.reset_all_rc, Hit::ResetAll));
    st.hits.push((st.open_ini_rc, Hit::OpenIni));

    let page = match NAV[st.page].page {
        Some(p) => p,
        None => {
            st.content_h = 0;
            st.scroll = 0;
            return;
        }
    };

    let mut y = 0;
    let gap = ui::s(10);
    for it in config::items_of(page) {
        let hgt = block_height(it, st.cw);
        st.blocks.push(Block { item: it, rect: rect(st.x0, y, st.cw, hgt) });
        y += hgt + gap;
    }
    st.content_h = y;
    let max_scroll = (st.content_h - rh(&st.content_rc)).max(0);
    st.scroll = st.scroll.clamp(0, max_scroll);

    // Zonas sensibles, ya desplazadas.
    let dy = st.content_rc.top - st.scroll;
    for b in st.blocks.iter() {
        let r = rect(b.rect.left, b.rect.top + dy, rw(&b.rect), rh(&b.rect));
        let it = b.item;
        match it.kind {
            Kind::Bool => st.hits.push((r, Hit::Toggle(it.id))),
            Kind::Int { .. } => {
                let (m, p) = stepper_rects(&r);
                st.hits.push((m, Hit::Step(it.id, -1)));
                st.hits.push((p, Hit::Step(it.id, 1)));
                let t = track_rect(&r);
                st.hits.push((rect(t.left - ui::s(8), t.top - ui::s(12), rw(&t) + ui::s(16), ui::s(28)), Hit::Slider(it.id, 0)));
            }
            Kind::Choice(opts) => {
                for (i, cr) in chip_rects(&r, opts, it).into_iter().enumerate() {
                    st.hits.push((cr, Hit::Choice(it.id, i as i32)));
                }
            }
            Kind::Cards(_) => {
                let (a, b2) = card_rects(&r);
                st.hits.push((a, Hit::Choice(it.id, 0)));
                st.hits.push((b2, Hit::Choice(it.id, 1)));
            }
            Kind::Color => {
                for (i, sr) in swatch_rects(&r).into_iter().enumerate() {
                    st.hits.push((sr, Hit::Swatch(i)));
                }
                for (i, t) in color_tracks(&r).iter().enumerate() {
                    st.hits.push((rect(t.left - ui::s(8), t.top - ui::s(12), rw(t) + ui::s(16), ui::s(28)),
                                  Hit::Slider(it.id, i as u8 + 1)));
                }
                st.hits.push((winaccent_rect(&r), Hit::WinAccent));
            }
        }
    }
}

// ---------------------------------------------------------------- aplicar
/// Guarda (si toca), recalcula el tema y avisa al resto de ventanas.
fn commit(st: &mut State, c: &Config, persist: bool) {
    ui::apply_config(c, persist);
    ui::broadcast_config_changed();
    layout(st);
    unsafe { InvalidateRect(st.hwnd, std::ptr::null(), 0) };
}

fn windows_accent() -> u32 {
    unsafe {
        let mut argb = 0u32;
        let mut opaque = 0i32;
        if windows_sys::Win32::Graphics::Dwm::DwmGetColorizationColor(&mut argb, &mut opaque) == 0 {
            let r = ((argb >> 16) & 0xFF) as u8;
            let g = ((argb >> 8) & 0xFF) as u8;
            let b = (argb & 0xFF) as u8;
            return theme::colorref(r, g, b);
        }
    }
    theme::colorref(59, 130, 246)
}

// ---------------------------------------------------------------- pintado
fn paint(st: &State) {
    let cfg = config::current();
    window::paint_buffered(st.hwnd, |g: &Gfx, w, h| {
        let s = theme::style();
        let cap = ui::caption_h();
        // marco
        g.clear(theme::argb(0, 0, 0, 0));
        g.rounded(0.0, 0.0, w as f32, h as f32, ui::sf(10.0), Some(s.window_bg), Some((s.card_border, 1.0)));
        g.fill_rect(0, 0, w, cap, s.title_bg);
        g.hline(0, w, cap - 1, s.separator);
        let ic = ui::s(20);
        g.rounded(ui::s(14) as f32, ((cap - ic) / 2) as f32, ic as f32, ic as f32, ui::sf(4.0), Some(s.accent_solid), None);
        g.text_in("\u{E713}", ui::s(14) as f32, ((cap - ic) / 2) as f32, ic as f32, ic as f32, 12.0, false, s.on_accent, Align::Center, Align::Center, true);
        g.text_in("Configuración", ui::s(42) as f32, 0.0, (w - ui::s(200)) as f32, cap as f32, 14.0, true, s.text, Align::Near, Align::Center, false);
        chrome_btn(g, &s, st.min_rc, "\u{E921}", st.hover == Hit::Min, false);
        chrome_btn(g, &s, st.close_rc, "\u{E8BB}", st.hover == Hit::Close, true);

        // navegación
        g.fill_rect(0, cap, ui::s(226), h - cap, s.nav_bg);
        for (i, r) in st.nav.iter().enumerate() {
            let active = i == st.page;
            if active {
                g.rounded(r.left as f32, r.top as f32, rw(r) as f32, rh(r) as f32, ui::sf(6.0), Some(s.row_selected), None);
                g.rounded(r.left as f32, (r.top + ui::s(10)) as f32, ui::s(3) as f32, (rh(r) - ui::s(20)) as f32, ui::sf(1.5), Some(s.accent), None);
            } else if st.hover == Hit::Nav(i) {
                g.rounded(r.left as f32, r.top as f32, rw(r) as f32, rh(r) as f32, ui::sf(6.0), Some(s.row_hover), None);
            }
            let col = if active { s.accent } else { s.text_dim };
            g.text_in(NAV[i].glyph, (r.left + ui::s(10)) as f32, r.top as f32, ui::s(24) as f32, rh(r) as f32, 12.0, false, col, Align::Center, Align::Center, true);
            g.text_in(NAV[i].title, (r.left + ui::s(44)) as f32, r.top as f32, (rw(r) - ui::s(48)) as f32, rh(r) as f32, 13.0, false, s.text, Align::Near, Align::Center, false);
        }

        // título de la página
        g.text_in(NAV[st.page].title, st.x0 as f32, (cap + ui::s(10)) as f32, st.cw as f32, ui::s(34) as f32, 19.0, true, s.text, Align::Near, Align::Center, false);

        if NAV[st.page].page.is_none() {
            g.text_in("Esta sección se añadirá en una próxima iteración del port a Rust.",
                      st.x0 as f32, st.content_rc.top as f32, st.cw as f32, ui::s(28) as f32,
                      13.0, false, s.text_faint, Align::Near, Align::Center, false);
        } else {
            // contenido desplazable
            let cr = st.content_rc;
            g.set_clip(cr.left, cr.top, rw(&cr), rh(&cr));
            let dy = cr.top - st.scroll;
            for b in st.blocks.iter() {
                let r = rect(b.rect.left, b.rect.top + dy, rw(&b.rect), rh(&b.rect));
                if r.bottom < cr.top || r.top > cr.bottom {
                    continue; // fuera de la vista
                }
                paint_block(g, &s, st, &cfg, b.item, &r);
            }
            g.reset_clip();
            // indicador de desplazamiento
            let max_scroll = (st.content_h - rh(&cr)).max(0);
            if max_scroll > 0 {
                let bar_h = (rh(&cr) * rh(&cr) / st.content_h.max(1)).max(ui::s(30));
                let by = cr.top + (rh(&cr) - bar_h) * st.scroll / max_scroll;
                g.rounded((cr.right + ui::s(6)) as f32, by as f32, ui::s(4) as f32, bar_h as f32,
                          ui::sf(2.0), Some(s.track), None);
            }
        }

        // pie
        g.hline(ui::s(226), w, h - ui::s(58), s.separator);
        draw_button(g, &s, st.reset_page_rc, "Restablecer esta página", st.hover == Hit::ResetPage, false);
        draw_button(g, &s, st.reset_all_rc, "Restablecer todo", st.hover == Hit::ResetAll, false);
        draw_button(g, &s, st.open_ini_rc, "Abrir config.ini", st.hover == Hit::OpenIni, false);
    });
}

fn paint_block(g: &Gfx, s: &theme::Style, st: &State, cfg: &Config, it: &'static Item, r: &RECT) {
    let value = cfg.get(it.id);
    let card = !matches!(it.kind, Kind::Cards(_));
    if card {
        g.rounded(r.left as f32, r.top as f32, rw(r) as f32, rh(r) as f32, ui::sf(8.0),
                  Some(s.card_bg), Some((s.card_border, 1.0)));
    }
    // etiqueta y ayuda
    if card {
        let label_w = rw(r) - ui::s(150);
        g.text_in(it.label, (r.left + ui::s(16)) as f32, (r.top + ui::s(8)) as f32, label_w as f32,
                  ui::s(22) as f32, 13.0, false, s.text, Align::Near, Align::Center, false);
        if has_help(it) {
            g.text_in(it.help, (r.left + ui::s(16)) as f32, (r.top + ui::s(28)) as f32, (rw(r) - ui::s(120)) as f32,
                      ui::s(18) as f32, 11.5, false, s.text_dim, Align::Near, Align::Center, false);
        }
    } else {
        g.text_in(it.label, r.left as f32, (r.top - ui::s(18)) as f32, rw(r) as f32, ui::s(20) as f32,
                  12.0, true, s.text_dim, Align::Near, Align::Center, false);
    }

    match it.kind {
        Kind::Bool => {
            let sw = switch_rect(r);
            g.switch(sw.left as f32, sw.top as f32, rw(&sw) as f32, rh(&sw) as f32, value != 0,
                     s.accent_solid, s.control_bg, if value != 0 { s.on_accent } else { s.text_dim });
        }
        Kind::Int { min, max, unit, .. } => {
            let (mr, pr) = stepper_rects(r);
            g.text_in(&format!("{} {}", value, unit), (mr.left - ui::s(90)) as f32, mr.top as f32,
                      ui::s(84) as f32, rh(&mr) as f32, 12.0, false, s.text_dim, Align::Far, Align::Center, false);
            small_btn(g, s, mr, "\u{E738}", st.hover == Hit::Step(it.id, -1));
            small_btn(g, s, pr, "\u{E710}", st.hover == Hit::Step(it.id, 1));
            let t = track_rect(r);
            slider(g, s, &t, value, min, max);
        }
        Kind::Choice(opts) => {
            for (i, cr) in chip_rects(r, opts, it).into_iter().enumerate() {
                let sel = value == i as i32;
                let hov = st.hover == Hit::Choice(it.id, i as i32);
                g.rounded(cr.left as f32, cr.top as f32, rw(&cr) as f32, rh(&cr) as f32, ui::sf(6.0),
                          Some(if sel { s.accent_solid } else if hov { s.control_hover } else { s.control_bg }),
                          Some((if sel { s.accent_solid } else { s.control_border }, 1.0)));
                g.text_in(opts[i], cr.left as f32, cr.top as f32, rw(&cr) as f32, rh(&cr) as f32, 12.0, false,
                          if sel { s.on_accent } else { s.text }, Align::Center, Align::Center, false);
            }
        }
        Kind::Cards(opts) => {
            let (a, b) = card_rects(r);
            mode_card(g, s, &a, true, opts[0], value == 0, st.hover == Hit::Choice(it.id, 0));
            mode_card(g, s, &b, false, opts.get(1).copied().unwrap_or("Claro"), value == 1,
                      st.hover == Hit::Choice(it.id, 1));
        }
        Kind::Color => {
            let cur = cfg.accent_colorref();
            for (i, sr) in swatch_rects(r).into_iter().enumerate() {
                let preset = &ACCENT_PRESETS[i];
                let sel = preset.color == cur;
                let cx = (sr.left + sr.right) as f32 / 2.0;
                let cy = (sr.top + sr.bottom) as f32 / 2.0;
                let rad = rw(&sr) as f32 / 2.0;
                if sel || st.hover == Hit::Swatch(i) {
                    g.stroke_circle(cx, cy, rad + ui::sf(3.0), if sel { s.text } else { s.text_faint },
                                    if sel { ui::sf(2.0) } else { 1.0 });
                }
                let c = theme::from_colorref(preset.color);
                g.fill_circle(cx, cy, rad - ui::sf(2.0), c);
                if sel {
                    let lum = (0.299 * theme::r_of(c) as f32 + 0.587 * theme::g_of(c) as f32 + 0.114 * theme::b_of(c) as f32) / 255.0;
                    g.check(cx, cy, rad * 0.8, if lum > 0.68 { theme::rgbc(20, 20, 24) } else { theme::rgbc(255, 255, 255) }, ui::sf(2.0));
                }
            }
            let names = ["R", "G", "B"];
            let packed = value as u32;
            let chans = [(packed >> 16) & 0xFF, (packed >> 8) & 0xFF, packed & 0xFF];
            for (i, t) in color_tracks(r).iter().enumerate() {
                g.text_in(names[i], (t.left - ui::s(26)) as f32, (t.top - ui::s(10)) as f32, ui::s(20) as f32,
                          ui::s(26) as f32, 11.5, false, s.text_dim, Align::Near, Align::Center, false);
                slider(g, s, t, chans[i] as i32, 0, 255);
                g.text_in(&format!("{}", chans[i]), (t.right + ui::s(10)) as f32, (t.top - ui::s(10)) as f32,
                          ui::s(40) as f32, ui::s(26) as f32, 11.5, false, s.text_dim, Align::Near, Align::Center, false);
            }
            // vista previa
            let pv = rect(r.right - ui::s(60), r.top + ui::s(10), ui::s(44), ui::s(24));
            g.rounded(pv.left as f32, pv.top as f32, rw(&pv) as f32, rh(&pv) as f32, ui::sf(5.0),
                      Some(theme::from_colorref(cfg.accent_colorref())), Some((s.card_border, 1.0)));
            draw_button(g, s, winaccent_rect(r), "Usar el color de Windows", st.hover == Hit::WinAccent, false);
        }
    }
}

fn slider(g: &Gfx, s: &theme::Style, track: &RECT, value: i32, min: i32, max: i32) {
    let tw = rw(track) as f32;
    let th = rh(track) as f32;
    g.rounded(track.left as f32, track.top as f32, tw, th, th / 2.0, Some(s.track), None);
    let frac = if max > min { (value - min) as f32 / (max - min) as f32 } else { 0.0 };
    let fillw = tw * frac.clamp(0.0, 1.0);
    g.rounded(track.left as f32, track.top as f32, fillw, th, th / 2.0, Some(s.accent_solid), None);
    let kx = track.left as f32 + fillw;
    g.fill_circle(kx, track.top as f32 + th / 2.0, ui::sf(8.0), s.accent_solid);
    g.fill_circle(kx, track.top as f32 + th / 2.0, ui::sf(4.0), s.on_accent);
}

fn mode_card(g: &Gfx, s: &theme::Style, r: &RECT, dark_mode: bool, label: &str, selected: bool, hover: bool) {
    g.rounded(r.left as f32, r.top as f32, rw(r) as f32, rh(r) as f32, ui::sf(10.0),
              Some(if hover { s.control_hover } else { s.card_bg }),
              Some((if selected { s.accent_solid } else { s.control_border }, if selected { 2.0 } else { 1.0 })));
    let (bg, title) = if dark_mode {
        (theme::rgbc(17, 20, 27), theme::rgbc(23, 27, 36))
    } else {
        (theme::rgbc(247, 248, 250), theme::rgbc(237, 239, 243))
    };
    let px = r.left + ui::s(12);
    let py = r.top + ui::s(12);
    let pw = rw(r) - ui::s(24);
    let ph = rh(r) - ui::s(52);
    g.rounded(px as f32, py as f32, pw as f32, ph as f32, ui::sf(6.0), Some(bg), None);
    g.rounded(px as f32, py as f32, pw as f32, ui::s(18) as f32, ui::sf(6.0), Some(title), None);
    g.rounded((px + ui::s(8)) as f32, (py + ui::s(5)) as f32, ui::s(8) as f32, ui::s(8) as f32, ui::sf(2.0), Some(s.accent_solid), None);
    g.text_in(label, (r.left + ui::s(14)) as f32, (r.bottom - ui::s(36)) as f32, (rw(r) - ui::s(50)) as f32,
              ui::s(28) as f32, 13.0, false, s.text, Align::Near, Align::Center, false);
    if selected {
        let bx = (r.right - ui::s(34)) as f32;
        let by = (r.bottom - ui::s(32)) as f32;
        g.fill_circle(bx + ui::s(10) as f32, by + ui::s(10) as f32, ui::s(10) as f32, s.accent_solid);
        g.check(bx + ui::s(10) as f32, by + ui::s(10) as f32, ui::sf(11.0), s.on_accent, ui::sf(1.8));
    }
}

fn chrome_btn(g: &Gfx, s: &theme::Style, r: RECT, glyph: &str, hov: bool, close: bool) {
    if hov {
        g.fill_rect(r.left, r.top, rw(&r), rh(&r), if close { s.close_hover } else { s.control_hover });
    }
    let col = if hov && close { theme::rgbc(255, 255, 255) } else { s.text_dim };
    g.text_in(glyph, r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32, 10.0, false, col, Align::Center, Align::Center, true);
}

fn small_btn(g: &Gfx, s: &theme::Style, r: RECT, glyph: &str, hov: bool) {
    g.rounded(r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32, ui::sf(5.0),
              Some(if hov { s.control_hover } else { s.control_bg }), Some((s.control_border, 1.0)));
    g.text_in(glyph, r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32, 8.0, false, s.text, Align::Center, Align::Center, true);
}

fn draw_button(g: &Gfx, s: &theme::Style, r: RECT, label: &str, hov: bool, primary: bool) {
    if r.right <= r.left {
        return;
    }
    let (fill, border, fg) = if primary {
        (if hov { s.accent_hover } else { s.accent_solid }, theme::argb(0, 0, 0, 0), s.on_accent)
    } else {
        (if hov { s.control_hover } else { s.control_bg }, s.control_border, s.text)
    };
    g.rounded(r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32, ui::sf(6.0), Some(fill), Some((border, 1.0)));
    g.text_in(label, r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32, 12.5, false, fg, Align::Center, Align::Center, false);
}

// ---------------------------------------------------------------- interacción
fn hit(st: &State, x: i32, y: i32) -> Hit {
    // Las zonas del contenido solo valen dentro del área visible.
    for (r, h) in st.hits.iter() {
        if !in_rect(r, x, y) {
            continue;
        }
        let inside_content = r.top >= st.content_rc.top - ui::s(30) && r.bottom <= st.content_rc.bottom + ui::s(30);
        let is_content = !matches!(h, Hit::Nav(_) | Hit::Min | Hit::Close | Hit::ResetPage | Hit::ResetAll | Hit::OpenIni);
        if is_content && !(inside_content && y >= st.content_rc.top && y <= st.content_rc.bottom) {
            continue;
        }
        return *h;
    }
    Hit::None
}

/// Valor correspondiente a la posición x dentro de una pista.
fn value_at(track: &RECT, x: i32, min: i32, max: i32, step: i32) -> i32 {
    let w = rw(track).max(1);
    let raw = min + ((x - track.left) as i64 * (max - min) as i64 / w as i64) as i32;
    let step = step.max(1);
    let snapped = ((raw - min + step / 2) / step) * step + min;
    snapped.clamp(min, max)
}

fn slider_track(st: &State, id: Id, channel: u8) -> Option<RECT> {
    let dy = st.content_rc.top - st.scroll;
    for b in st.blocks.iter() {
        if b.item.id != id {
            continue;
        }
        let r = rect(b.rect.left, b.rect.top + dy, rw(&b.rect), rh(&b.rect));
        return Some(if channel == 0 {
            track_rect(&r)
        } else {
            color_tracks(&r)[(channel - 1) as usize]
        });
    }
    None
}

fn apply_slider(st: &mut State, id: Id, channel: u8, x: i32, persist: bool) {
    let Some(t) = slider_track(st, id, channel) else { return };
    let mut c = config::current();
    if channel == 0 {
        if let Kind::Int { min, max, step, .. } = config::item(id).kind {
            c.set(id, value_at(&t, x, min, max, step));
        }
    } else {
        let v = value_at(&t, x, 0, 255, 1) as u32;
        let packed = c.get(id) as u32;
        let shift = 16 - (channel as u32 - 1) * 8;
        let cleared = packed & !(0xFF << shift);
        c.set(id, (cleared | (v << shift)) as i32);
    }
    commit(st, &c, persist);
}

fn nudge(st: &mut State, id: Id, dir: i32) {
    let mut c = config::current();
    match config::item(id).kind {
        Kind::Int { step, .. } => {
            let v = c.get(id) + dir * step.max(1);
            c.set(id, v);
        }
        Kind::Bool => c.toggle(id),
        Kind::Choice(opts) | Kind::Cards(opts) => {
            let n = opts.len() as i32;
            c.set(id, (c.get(id) + dir).rem_euclid(n));
        }
        Kind::Color => {}
    }
    commit(st, &c, true);
}

fn click(st: &mut State, x: i32, y: i32) {
    let hwnd = st.hwnd;
    match hit(st, x, y) {
        Hit::Close => unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0); },
        Hit::Min => unsafe { ShowWindow(hwnd, SW_MINIMIZE); },
        Hit::Nav(i) => {
            st.page = i;
            st.scroll = 0;
            st.hover = Hit::None;
            layout(st);
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
        }
        Hit::Toggle(id) => {
            let mut c = config::current();
            c.toggle(id);
            commit(st, &c, true);
        }
        Hit::Choice(id, v) => {
            let mut c = config::current();
            c.set(id, v);
            commit(st, &c, true);
        }
        Hit::Step(id, d) => nudge(st, id, d),
        Hit::Swatch(i) => {
            let mut c = config::current();
            c.set_accent_colorref(ACCENT_PRESETS[i].color);
            commit(st, &c, true);
        }
        Hit::WinAccent => {
            let mut c = config::current();
            c.set_accent_colorref(windows_accent());
            commit(st, &c, true);
        }
        Hit::Slider(id, ch) => {
            st.drag = Some((id, ch));
            unsafe { SetCapture(hwnd) };
            apply_slider(st, id, ch, x, false);
        }
        Hit::ResetPage => {
            if let Some(p) = NAV[st.page].page {
                let mut c = config::current();
                c.reset_page(p);
                commit(st, &c, true);
            }
        }
        Hit::ResetAll => {
            let c = Config::default();
            commit(st, &c, true);
        }
        Hit::OpenIni => unsafe {
            let path = config::config_file();
            config::save(&config::current());
            ShellExecuteW(std::ptr::null_mut(), ui::wide("open").as_ptr(),
                          ui::wide(&path.to_string_lossy()).as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
        },
        Hit::None => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let st_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    // El mensaje de difusión lo origina esta ventana: la configuración ya está
    // aplicada en memoria, solo hay que volver a medir.
    if msg == ui::config_changed_msg() {
        if !st_ptr.is_null() {
            window::refresh_frame(hwnd);
            layout(&mut *st_ptr);
            InvalidateRect(hwnd, std::ptr::null(), 0);
        }
        return 0;
    }
    match msg {
        WM_NCCALCSIZE if wp != 0 => 0,
        WM_NCHITTEST => {
            let mut pt = POINT { x: (lp & 0xFFFF) as i16 as i32, y: ((lp >> 16) & 0xFFFF) as i16 as i32 };
            ScreenToClient(hwnd, &mut pt);
            let mut rc = er();
            GetClientRect(hwnd, &mut rc);
            let b = ui::s(6);
            if IsZoomed(hwnd) == 0 {
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
                if !st_ptr.is_null() {
                    let st = &*st_ptr;
                    if in_rect(&st.close_rc, pt.x, pt.y) || in_rect(&st.min_rc, pt.x, pt.y) {
                        return HTCLIENT as LRESULT;
                    }
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
                if let Some((id, ch)) = st.drag {
                    apply_slider(st, id, ch, x, false);
                } else {
                    let h = hit(st, x, y);
                    if h != st.hover {
                        st.hover = h;
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                click(st, x, y);
            }
            0
        }
        WM_LBUTTONUP => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                if st.drag.take().is_some() {
                    ReleaseCapture();
                    // Al soltar se escribe en disco.
                    let c = config::current();
                    commit(st, &c, true);
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let delta = ((wp >> 16) & 0xFFFF) as i16 as i32;
                let max_scroll = (st.content_h - rh(&st.content_rc)).max(0);
                st.scroll = (st.scroll - (delta / 120) * ui::s(48)).clamp(0, max_scroll);
                layout(st);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_KEYDOWN => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let vk = wp as u32;
                let max_scroll = (st.content_h - rh(&st.content_rc)).max(0);
                if vk == VK_ESCAPE as u32 {
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                } else if vk == VK_DOWN as u32 || vk == VK_UP as u32 {
                    let d = if vk == VK_DOWN as u32 { 1 } else { -1 };
                    st.scroll = (st.scroll + d * ui::s(40)).clamp(0, max_scroll);
                    layout(st);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                } else if vk == VK_LEFT as u32 || vk == VK_RIGHT as u32 {
                    // Ajusta el control bajo el puntero.
                    let d = if vk == VK_RIGHT as u32 { 1 } else { -1 };
                    let mut pt = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut pt);
                    ScreenToClient(hwnd, &mut pt);
                    match hit(st, pt.x, pt.y) {
                        Hit::Slider(id, 0) | Hit::Step(id, _) | Hit::Toggle(id) | Hit::Choice(id, _) => nudge(st, id, d),
                        _ => {}
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

        // página inicial según el argumento posicional
        let arg = args::positional().unwrap_or_default().to_lowercase();
        let page = NAV.iter().position(|n| n.arg == arg).unwrap_or(0);

        let class_name = ui::wide("OShellRustSettings");
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

        let mut wa = er();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut _ as *mut c_void, 0);
        let w = ui::s(980).min(wa.right - wa.left);
        let h = ui::s(700).min(wa.bottom - wa.top);
        let x = wa.left + (wa.right - wa.left - w) / 2;
        let y = wa.top + (wa.bottom - wa.top - h) / 2;

        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            class_name.as_ptr(),
            ui::wide("Configuración").as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_THICKFRAME,
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
            page,
            hover: Hit::None,
            scroll: 0,
            content_h: 0,
            drag: None,
            x0: 0,
            cw: 0,
            content_rc: er(),
            nav: Vec::new(),
            blocks: Vec::new(),
            hits: Vec::new(),
            min_rc: er(),
            close_rc: er(),
            reset_page_rc: er(),
            reset_all_rc: er(),
            open_ini_rc: er(),
        });
        let raw = Box::into_raw(st);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
        window::refresh_frame(hwnd);
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
        layout(&mut *raw);
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetForegroundWindow(hwnd);
        ui::apply_perf();

        let code = window::message_loop();
        ui::shutdown();
        code
    }
}
