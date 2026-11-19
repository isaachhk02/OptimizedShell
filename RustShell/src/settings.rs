// Configuración (--settings). Port de CppShell/src/settings.cpp.
//
// Primera iteración: marco propio + barra de navegación (Personalización · Fondo
// de escritorio · Pantalla · Bluetooth) y la página de Personalización completa
// (modo claro/oscuro, color de acento, desenfoque y opacidades), que guarda en
// %APPDATA%\OptimizedShell\theme.ini y lo aplica al momento, avisando al resto de
// ventanas de OptimizedShell. Las páginas Fondo/Pantalla/Bluetooth (miniaturas,
// DisplayConfig/WMI y WinRT) se añadirán en iteraciones siguientes.
#![allow(non_snake_case, non_upper_case_globals)]

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, ScreenToClient};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::args;
use crate::ui::gdip::{Align, Gfx};
use crate::ui::theme::{self, ACCENT_PRESETS};
use crate::ui::{self, window};

const CAPTION: i32 = 42;
const PAGES: [&str; 4] = ["Personalización", "Fondo de escritorio", "Pantalla", "Bluetooth"];
const PAGE_GLYPHS: [&str; 4] = ["\u{E771}", "\u{E91B}", "\u{E7F4}", "\u{E702}"];

// Zonas interactivas.
const H_NONE: i32 = 0;
const H_MIN: i32 = 1;
const H_CLOSE: i32 = 2;
const H_NAV0: i32 = 20; // + página
const H_DARK: i32 = 30;
const H_LIGHT: i32 = 31;
const H_WINACCENT: i32 = 32;
const H_BLUR: i32 = 33;
const H_RESET: i32 = 36;
const H_SWATCH0: i32 = 100; // + índice de acento

struct State {
    hwnd: HWND,
    page: usize,
    hover: i32,
    drag_opacity: i32, // 0 ninguno, 1 barra, 2 ventanas
    live_opacity: i32,
    // layout
    x0: i32,
    cw: i32,
    nav: [RECT; 4],
    dark_rc: RECT,
    light_rc: RECT,
    swatch: Vec<RECT>,
    winaccent_rc: RECT,
    blur_rc: RECT,
    task_track: RECT,
    win_track: RECT,
    reset_rc: RECT,
    min_rc: RECT,
    close_rc: RECT,
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

fn layout(st: &mut State) {
    let mut rc = er();
    unsafe { GetClientRect(st.hwnd, &mut rc) };
    let w = rc.right;
    let h = rc.bottom;
    let ch = ui::gdip::chrome_rects(w);
    st.min_rc = rect(ch.min.0, ch.min.1, ch.min.2, ch.min.3);
    st.close_rc = rect(ch.close.0, ch.close.1, ch.close.2, ch.close.3);
    for i in 0..4 {
        st.nav[i] = rect(ui::s(10), ui::s(58) + i as i32 * ui::s(42), ui::s(206), ui::s(38));
    }
    st.x0 = ui::s(250);
    st.cw = w - st.x0 - ui::s(28);
    let card_w = (st.cw - ui::s(16)) / 2;
    st.dark_rc = rect(st.x0, ui::s(126), card_w, ui::s(156));
    st.light_rc = rect(st.x0 + card_w + ui::s(16), ui::s(126), card_w, ui::s(156));
    let (sw, gap) = (ui::s(34), ui::s(10));
    st.swatch = (0..ACCENT_PRESETS.len() as i32)
        .map(|i| rect(st.x0 + i * (sw + gap), ui::s(330), sw, sw))
        .collect();
    st.winaccent_rc = rect(st.x0, ui::s(380), ui::s(236), ui::s(34));
    st.blur_rc = rect(st.x0, ui::s(446), st.cw, ui::s(56));
    let task_rc = rect(st.x0, ui::s(510), st.cw, ui::s(46));
    let win_rc = rect(st.x0, ui::s(562), st.cw, ui::s(46));
    st.task_track = rect(task_rc.left + ui::s(16), task_rc.top + ui::s(30), st.cw - ui::s(32), ui::s(6));
    st.win_track = rect(win_rc.left + ui::s(16), win_rc.top + ui::s(30), st.cw - ui::s(32), ui::s(6));
    st.reset_rc = rect(st.x0 + st.cw - ui::s(150), h - ui::s(50), ui::s(150), ui::s(34));
}

// ---------------------------------------------------------------- tema
fn apply(t: theme::ThemeSettings) {
    theme::save(&t);
    theme::apply(t);
    unsafe {
        let msg = RegisterWindowMessageW(ui::wide("OptimizedShell.ThemeChanged").as_ptr());
        SendNotifyMessageW(HWND_BROADCAST, msg, 0, 0);
    }
}

// ---------------------------------------------------------------- pintado
fn paint(st: &State) {
    window::paint_buffered(st.hwnd, |g: &Gfx, w, h| {
        let s = theme::style();
        // marco
        g.clear(theme::argb(0, 0, 0, 0));
        g.rounded(0.0, 0.0, w as f32, h as f32, ui::sf(10.0), Some(s.window_bg), Some((s.card_border, 1.0)));
        g.fill_rect(0, 0, w, ui::s(CAPTION), s.title_bg);
        g.hline(0, w, ui::s(CAPTION) - 1, s.separator);
        g.rounded(ui::s(14) as f32, ui::s(11) as f32, ui::s(20) as f32, ui::s(20) as f32, ui::sf(4.0), Some(s.accent_solid), None);
        g.text_in("\u{E713}", ui::s(14) as f32, ui::s(11) as f32, ui::s(20) as f32, ui::s(20) as f32, 12.0, false, s.on_accent, Align::Center, Align::Center, true);
        g.text_in("Configuración", ui::s(42) as f32, 0.0, (w - ui::s(200)) as f32, ui::s(CAPTION) as f32, 14.0, true, s.text, Align::Near, Align::Center, false);
        // botones min/cerrar
        chrome_btn(g, &s, st.min_rc, "\u{E921}", st.hover == H_MIN, false);
        chrome_btn(g, &s, st.close_rc, "\u{E8BB}", st.hover == H_CLOSE, true);

        // navegación
        g.fill_rect(0, ui::s(CAPTION), ui::s(226), h - ui::s(CAPTION), s.nav_bg);
        for i in 0..4 {
            let r = st.nav[i];
            if i == st.page {
                g.rounded(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32, ui::sf(6.0), Some(s.row_selected), None);
                g.rounded(r.left as f32, (r.top + ui::s(10)) as f32, ui::s(3) as f32, (r.bottom - r.top - ui::s(20)) as f32, ui::sf(1.5), Some(s.accent), None);
            } else if st.hover == H_NAV0 + i as i32 {
                g.rounded(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32, ui::sf(6.0), Some(s.row_hover), None);
            }
            let col = if i == st.page { s.accent } else { s.text_dim };
            g.text_in(PAGE_GLYPHS[i], (r.left + ui::s(10)) as f32, r.top as f32, ui::s(24) as f32, (r.bottom - r.top) as f32, 12.0, false, col, Align::Center, Align::Center, true);
            g.text_in(PAGES[i], (r.left + ui::s(44)) as f32, r.top as f32, (r.right - r.left - ui::s(48)) as f32, (r.bottom - r.top) as f32, 13.0, false, s.text, Align::Near, Align::Center, false);
        }

        // título de página
        g.text_in(PAGES[st.page], st.x0 as f32, ui::s(60) as f32, st.cw as f32, ui::s(34) as f32, 19.0, true, s.text, Align::Near, Align::Center, false);

        match st.page {
            0 => paint_theme(g, &s, st, h),
            _ => {
                g.text_in("Esta sección se añadirá en una próxima iteración del port a Rust.",
                          st.x0 as f32, ui::s(140) as f32, st.cw as f32, ui::s(28) as f32, 13.0, false, s.text_faint, Align::Near, Align::Center, false);
            }
        }
    });
}

fn section_title(g: &Gfx, s: &theme::Style, st: &State, t: &str, y: i32) {
    g.text_in(t, st.x0 as f32, y as f32, st.cw as f32, ui::s(24) as f32, 12.0, true, s.text_dim, Align::Near, Align::Center, false);
}

fn mode_card(g: &Gfx, s: &theme::Style, r: RECT, dark_mode: bool, label: &str, hover: bool) {
    let cur = theme::current();
    let selected = cur.dark == dark_mode;
    g.rounded(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32, ui::sf(10.0),
              Some(if hover { s.control_hover } else { s.card_bg }),
              Some((if selected { s.accent_solid } else { s.control_border }, if selected { 2.0 } else { 1.0 })));
    // vista previa
    let (bg, title) = if dark_mode { (theme::rgbc(17, 20, 27), theme::rgbc(23, 27, 36)) } else { (theme::rgbc(247, 248, 250), theme::rgbc(237, 239, 243)) };
    let px = r.left + ui::s(12);
    let py = r.top + ui::s(12);
    let pw = (r.right - r.left) - ui::s(24);
    let ph = (r.bottom - r.top) - ui::s(52);
    g.rounded(px as f32, py as f32, pw as f32, ph as f32, ui::sf(6.0), Some(bg), None);
    g.rounded(px as f32, py as f32, pw as f32, ui::s(18) as f32, ui::sf(6.0), Some(title), None);
    g.rounded((px + ui::s(8)) as f32, (py + ui::s(5)) as f32, ui::s(8) as f32, ui::s(8) as f32, ui::sf(2.0), Some(s.accent_solid), None);
    g.text_in(label, (r.left + ui::s(14)) as f32, (r.bottom - ui::s(36)) as f32, (r.right - r.left - ui::s(50)) as f32, ui::s(28) as f32, 13.0, false, s.text, Align::Near, Align::Center, false);
    if selected {
        let bx = (r.right - ui::s(34)) as f32;
        let by = (r.bottom - ui::s(32)) as f32;
        g.fill_circle(bx + ui::s(10) as f32, by + ui::s(10) as f32, ui::s(10) as f32, s.accent_solid);
        g.check(bx + ui::s(10) as f32, by + ui::s(10) as f32, ui::sf(11.0), s.on_accent, ui::sf(1.8));
    }
}

fn paint_theme(g: &Gfx, s: &theme::Style, st: &State, h: i32) {
    let cur = theme::current();
    section_title(g, s, st, "Modo", ui::s(96));
    mode_card(g, s, st.dark_rc, true, "Oscuro", st.hover == H_DARK);
    mode_card(g, s, st.light_rc, false, "Claro", st.hover == H_LIGHT);

    section_title(g, s, st, "Color de acento", ui::s(298));
    for (i, r) in st.swatch.iter().enumerate() {
        let sel = cur.accent == ACCENT_PRESETS[i].color;
        let cx = (r.left + r.right) as f32 / 2.0;
        let cy = (r.top + r.bottom) as f32 / 2.0;
        let rad = (r.right - r.left) as f32 / 2.0;
        if sel || st.hover == H_SWATCH0 + i as i32 {
            g.stroke_circle(cx, cy, rad + ui::sf(3.0), if sel { s.text } else { s.text_faint }, if sel { ui::sf(2.0) } else { 1.0 });
        }
        let c = theme::from_colorref(ACCENT_PRESETS[i].color);
        g.fill_circle(cx, cy, rad - ui::sf(2.0), c);
        if sel {
            let lum = (0.299 * theme::r_of(c) as f32 + 0.587 * theme::g_of(c) as f32 + 0.114 * theme::b_of(c) as f32) / 255.0;
            g.check(cx, cy, rad * 0.8, if lum > 0.68 { theme::rgbc(20, 20, 24) } else { theme::rgbc(255, 255, 255) }, ui::sf(2.0));
        }
    }
    draw_button(g, s, st.winaccent_rc, "Usar el color de Windows", st.hover == H_WINACCENT, false);

    section_title(g, s, st, "Efectos", ui::s(418));
    let b = st.blur_rc;
    g.rounded(b.left as f32, b.top as f32, (b.right - b.left) as f32, (b.bottom - b.top) as f32, ui::sf(8.0), Some(s.card_bg), Some((s.card_border, 1.0)));
    g.text_in("Desenfoque (efecto cristal)", (b.left + ui::s(16)) as f32, (b.top + ui::s(8)) as f32, (b.right - b.left - ui::s(100)) as f32, ui::s(22) as f32, 13.0, false, s.text, Align::Near, Align::Center, false);
    g.text_in("Fondo acrílico en la barra de tareas, los paneles y las ventanas. Desactivarlo ahorra GPU.",
              (b.left + ui::s(16)) as f32, (b.top + ui::s(30)) as f32, (b.right - b.left - ui::s(100)) as f32, ui::s(20) as f32, 11.5, false, s.text_dim, Align::Near, Align::Center, false);
    g.switch((b.right - ui::s(62)) as f32, (b.top + (b.bottom - b.top - ui::s(22)) / 2) as f32, ui::s(44) as f32, ui::s(22) as f32,
             cur.blur, s.accent_solid, s.control_bg, if cur.blur { s.on_accent } else { s.text_dim });

    let task_op = if st.drag_opacity == 1 { st.live_opacity } else { cur.taskbar_opacity };
    let win_op = if st.drag_opacity == 2 { st.live_opacity } else { cur.window_opacity };
    opacity_slider(g, s, st.task_track, "Opacidad de la barra de tareas", task_op);
    opacity_slider(g, s, st.win_track, "Opacidad de las ventanas", win_op);

    draw_button(g, s, st.reset_rc, "Restablecer", st.hover == H_RESET, false);
    let _ = h;
}

fn opacity_slider(g: &Gfx, s: &theme::Style, track: RECT, label: &str, value: i32) {
    g.text_in(label, track.left as f32, (track.top - ui::s(24)) as f32, (track.right - track.left - ui::s(60)) as f32, ui::s(20) as f32, 12.0, false, s.text, Align::Near, Align::Center, false);
    g.text_in(&format!("{} %", value), (track.right - ui::s(50)) as f32, (track.top - ui::s(24)) as f32, ui::s(50) as f32, ui::s(20) as f32, 12.0, false, s.text_dim, Align::Far, Align::Center, false);
    let tw = (track.right - track.left) as f32;
    let th = (track.bottom - track.top) as f32;
    g.rounded(track.left as f32, track.top as f32, tw, th, th / 2.0, Some(s.track), None);
    let fillw = tw * value as f32 / 100.0;
    g.rounded(track.left as f32, track.top as f32, fillw, th, th / 2.0, Some(s.accent_solid), None);
    let kx = track.left as f32 + fillw;
    g.fill_circle(kx, track.top as f32 + th / 2.0, ui::sf(8.0), s.accent_solid);
    g.fill_circle(kx, track.top as f32 + th / 2.0, ui::sf(4.0), s.on_accent);
}

fn chrome_btn(g: &Gfx, s: &theme::Style, r: RECT, glyph: &str, hov: bool, close: bool) {
    if hov {
        g.fill_rect(r.left, r.top, r.right - r.left, r.bottom - r.top, if close { s.close_hover } else { s.control_hover });
    }
    let col = if hov && close { theme::rgbc(255, 255, 255) } else { s.text_dim };
    g.text_in(glyph, r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32, 10.0, false, col, Align::Center, Align::Center, true);
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
    g.rounded(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32, ui::sf(6.0), Some(fill), Some((border, 1.0)));
    g.text_in(label, r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32, 13.0, false, fg, Align::Center, Align::Center, false);
}

// ---------------------------------------------------------------- interacción
fn hit(st: &State, x: i32, y: i32) -> i32 {
    if in_rect(&st.close_rc, x, y) {
        return H_CLOSE;
    }
    if in_rect(&st.min_rc, x, y) {
        return H_MIN;
    }
    for i in 0..4 {
        if in_rect(&st.nav[i], x, y) {
            return H_NAV0 + i as i32;
        }
    }
    if st.page == 0 {
        if in_rect(&st.dark_rc, x, y) {
            return H_DARK;
        }
        if in_rect(&st.light_rc, x, y) {
            return H_LIGHT;
        }
        for (i, r) in st.swatch.iter().enumerate() {
            if in_rect(r, x, y) {
                return H_SWATCH0 + i as i32;
            }
        }
        if in_rect(&st.winaccent_rc, x, y) {
            return H_WINACCENT;
        }
        if in_rect(&st.blur_rc, x, y) {
            return H_BLUR;
        }
        if in_rect(&st.reset_rc, x, y) {
            return H_RESET;
        }
    }
    H_NONE
}

fn near_track(track: &RECT, x: i32, y: i32) -> bool {
    x >= track.left - ui::s(8) && x <= track.right + ui::s(8) && (y - (track.top + track.bottom) / 2).abs() <= ui::s(14)
}

fn value_at(track: &RECT, x: i32) -> i32 {
    let w = (track.right - track.left).max(1);
    (((x - track.left) * 100 / w)).clamp(0, 100)
}

fn click(st: &mut State, x: i32, y: i32) {
    let hwnd = st.hwnd;
    let h = hit(st, x, y);
    match h {
        H_CLOSE => unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0); },
        H_MIN => unsafe { ShowWindow(hwnd, SW_MINIMIZE); },
        _ if h >= H_NAV0 && h < H_NAV0 + 4 => {
            st.page = (h - H_NAV0) as usize;
            st.hover = H_NONE;
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
        }
        H_DARK | H_LIGHT => {
            let mut t = theme::current();
            t.dark = h == H_DARK;
            apply(t);
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
        }
        H_WINACCENT => {
            let mut t = theme::current();
            t.accent = windows_accent();
            apply(t);
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
        }
        H_BLUR => {
            let mut t = theme::current();
            t.blur = !t.blur;
            apply(t);
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
        }
        H_RESET => {
            apply(theme::ThemeSettings::default());
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
        }
        _ if h >= H_SWATCH0 => {
            let i = (h - H_SWATCH0) as usize;
            let mut t = theme::current();
            t.accent = ACCENT_PRESETS[i].color;
            apply(t);
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
        }
        _ => {}
    }
}

fn windows_accent() -> u32 {
    unsafe {
        let mut argb = 0u32;
        let mut opaque = 0i32;
        if windows_sys::Win32::Graphics::Dwm::DwmGetColorizationColor(&mut argb, &mut opaque) == 0 {
            let r = ((argb >> 16) & 0xFF) as u8;
            let g = ((argb >> 8) & 0xFF) as u8;
            let b = (argb & 0xFF) as u8;
            return (r as u32) | ((g as u32) << 8) | ((b as u32) << 16);
        }
    }
    theme::ThemeSettings::default().accent
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let st_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    match msg {
        WM_NCCALCSIZE if wp != 0 => 0,
        WM_NCHITTEST => {
            let mut pt = POINT { x: (lp & 0xFFFF) as i16 as i32, y: ((lp >> 16) & 0xFFFF) as i16 as i32 };
            ScreenToClient(hwnd, &mut pt);
            if pt.y < ui::s(CAPTION) {
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
                if st.drag_opacity != 0 {
                    let track = if st.drag_opacity == 1 { st.task_track } else { st.win_track };
                    st.live_opacity = value_at(&track, x);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                } else {
                    let hb = hit(st, x, y);
                    if hb != st.hover {
                        st.hover = hb;
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
                if st.page == 0 && near_track(&st.task_track, x, y) {
                    st.drag_opacity = 1;
                    st.live_opacity = value_at(&st.task_track, x);
                    SetCapture(hwnd);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                    return 0;
                }
                if st.page == 0 && near_track(&st.win_track, x, y) {
                    st.drag_opacity = 2;
                    st.live_opacity = value_at(&st.win_track, x);
                    SetCapture(hwnd);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                    return 0;
                }
                click(st, x, y);
            }
            0
        }
        WM_LBUTTONUP => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                if st.drag_opacity != 0 {
                    let mut t = theme::current();
                    if st.drag_opacity == 1 {
                        t.taskbar_opacity = st.live_opacity;
                    } else {
                        t.window_opacity = st.live_opacity;
                    }
                    st.drag_opacity = 0;
                    ReleaseCapture();
                    apply(t);
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

        // página inicial según argumento posicional (fondo|pantalla|bluetooth)
        let page = match args::positional().as_deref() {
            Some("fondo") => 1,
            Some("pantalla") => 2,
            Some("bluetooth") => 3,
            _ => 0,
        };

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
        let w = ui::s(940).min(wa.right - wa.left);
        let h = ui::s(680).min(wa.bottom - wa.top);
        let x = wa.left + (wa.right - wa.left - w) / 2;
        let y = wa.top + (wa.bottom - wa.top - h) / 2;

        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            class_name.as_ptr(),
            ui::wide("Configuración").as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
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
            hover: H_NONE,
            drag_opacity: 0,
            live_opacity: 0,
            x0: 0,
            cw: 0,
            nav: [er(); 4],
            dark_rc: er(),
            light_rc: er(),
            swatch: Vec::new(),
            winaccent_rc: er(),
            blur_rc: er(),
            task_track: er(),
            win_track: er(),
            reset_rc: er(),
            min_rc: er(),
            close_rc: er(),
        });
        let raw = Box::into_raw(st);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
        window::apply_frame_opaque(hwnd, theme::current().dark);
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
        layout(&mut *raw);
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetForegroundWindow(hwnd);
        ui::set_process_eco(true);
        ui::trim_working_set();

        let code = window::message_loop();
        ui::shutdown();
        code
    }
}
