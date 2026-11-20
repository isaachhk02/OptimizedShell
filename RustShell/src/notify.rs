// Sistema de notificaciones.
//
// Tres piezas:
//   * un almacén de avisos en el proceso del shell (la barra de tareas),
//   * los avisos emergentes («toasts»), apilados en una esquina sobre la barra
//     y con cierre automático,
//   * el centro de notificaciones, un panel que se abre desde la campana del
//     área de notificación y guarda el historial.
//
// Cualquier parte de OptimizedShell —incluso otra instancia del ejecutable,
// porque cada modo es un proceso— puede lanzar un aviso con `post`, que lo
// envía a la ventana de la barra con WM_COPYDATA. `OptimizedShell --notify
// "Título|Mensaje"` hace lo mismo desde la línea de órdenes o un script.
#![allow(non_snake_case, non_upper_case_globals)]

use std::sync::Mutex;

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, SYSTEMTIME, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetTickCount};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::ui::config::{self, Id};
use crate::ui::gdip::{Align, Gfx};
use crate::ui::{self, theme, window};

pub const TOAST_CLASS: &str = "OShellRustToasts";
pub const CENTER_CLASS: &str = "OShellRustNotifCenter";
/// Identificador del bloque de WM_COPYDATA, para no confundirlo con otro.
pub const COPYDATA_ID: usize = 0x05_7A_11;

// ---------------------------------------------------------------- modelo
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Info,
    Success,
    Warning,
    Error,
}

impl Kind {
    fn from_code(c: u32) -> Kind {
        match c {
            1 => Kind::Success,
            2 => Kind::Warning,
            3 => Kind::Error,
            _ => Kind::Info,
        }
    }
    fn code(self) -> u32 {
        match self {
            Kind::Info => 0,
            Kind::Success => 1,
            Kind::Warning => 2,
            Kind::Error => 3,
        }
    }
    fn glyph(self) -> &'static str {
        match self {
            Kind::Info => "\u{E946}",
            Kind::Success => "\u{E73E}",
            Kind::Warning => "\u{E7BA}",
            Kind::Error => "\u{EA39}",
        }
    }
    fn color(self, s: &theme::Style) -> theme::Color {
        match self {
            Kind::Info => s.accent,
            Kind::Success => s.success,
            Kind::Warning => s.warning,
            Kind::Error => s.danger,
        }
    }
}

#[derive(Clone)]
pub struct Note {
    pub app: String,
    pub title: String,
    pub body: String,
    pub kind: Kind,
    /// Hora de llegada, ya formateada (el aviso no cambia de hora).
    pub time: String,
    pub read: bool,
}

/// Historial del proceso del shell. Las demás instancias no lo ven: le mandan
/// los avisos con `post`.
static NOTES: Mutex<Vec<Note>> = Mutex::new(Vec::new());

pub fn notes() -> Vec<Note> {
    NOTES.lock().unwrap().clone()
}

pub fn count() -> usize {
    NOTES.lock().unwrap().len()
}

pub fn unread() -> usize {
    NOTES.lock().unwrap().iter().filter(|n| !n.read).count()
}

pub fn mark_all_read() {
    for n in NOTES.lock().unwrap().iter_mut() {
        n.read = true;
    }
}

pub fn clear_all() {
    NOTES.lock().unwrap().clear();
}

pub fn remove(index: usize) {
    let mut v = NOTES.lock().unwrap();
    if index < v.len() {
        v.remove(index);
    }
}

fn now_text() -> String {
    let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!("{:02}:{:02}", t.wHour, t.wMinute)
}

// ---------------------------------------------------------------- envío
/// Manda un aviso a la barra de tareas de OptimizedShell. Devuelve false si la
/// barra no se está ejecutando.
pub fn post(app: &str, title: &str, body: &str, kind: Kind) -> bool {
    unsafe {
        let bar = FindWindowW(ui::wide(crate::shell::CLASS).as_ptr(), std::ptr::null());
        if bar.is_null() {
            return false;
        }
        // Campos separados por 0x1F (separador de unidad), que no aparece en texto.
        let payload = format!("{}\u{1F}{}\u{1F}{}\u{1F}{}", kind.code(), app, title, body);
        let utf16 = ui::wide(&payload);
        let cds = COPYDATASTRUCT {
            dwData: COPYDATA_ID,
            cbData: (utf16.len() * 2) as u32,
            lpData: utf16.as_ptr() as *mut _,
        };
        SendMessageW(bar, WM_COPYDATA, 0, &cds as *const _ as LPARAM);
        true
    }
}

/// Decodifica el bloque recibido por WM_COPYDATA.
pub fn decode(cds: &COPYDATASTRUCT) -> Option<Note> {
    if cds.dwData != COPYDATA_ID || cds.lpData.is_null() || cds.cbData < 2 {
        return None;
    }
    let len = (cds.cbData / 2) as usize;
    let slice = unsafe { std::slice::from_raw_parts(cds.lpData as *const u16, len) };
    let end = slice.iter().position(|&c| c == 0).unwrap_or(len);
    let text = String::from_utf16_lossy(&slice[..end]);
    let mut parts = text.splitn(4, '\u{1F}');
    let kind = Kind::from_code(parts.next()?.parse().unwrap_or(0));
    let app = parts.next().unwrap_or("OptimizedShell").to_string();
    let title = parts.next().unwrap_or_default().to_string();
    let body = parts.next().unwrap_or_default().to_string();
    if title.is_empty() && body.is_empty() {
        return None;
    }
    Some(Note { app, title, body, kind, time: now_text(), read: false })
}

// ---------------------------------------------------------------- recepción
/// Ventanas del subsistema de avisos, creadas por el shell.
#[derive(Clone, Copy)]
pub struct Windows {
    pub toasts: HWND,
    pub center: HWND,
}

/// Guarda el aviso en el historial y lo enseña si procede. Devuelve true si
/// hay que repintar la barra (el contador de la campana ha cambiado).
pub fn receive(w: &Windows, note: Note) -> bool {
    let c = config::current();
    if !c.b(Id::NotifActivadas) {
        return false;
    }
    let keep = c.get(Id::NotifHistorial).max(0) as usize;
    {
        let mut v = NOTES.lock().unwrap();
        v.insert(0, note.clone());
        if v.len() > keep {
            v.truncate(keep);
        }
    }
    if !c.b(Id::NotifNoMolestar) {
        push_toast(w.toasts, note);
        if c.b(Id::NotifSonido) {
            unsafe { MessageBeep(MB_ICONASTERISK) };
        }
    }
    unsafe {
        if !w.center.is_null() && IsWindowVisible(w.center) != 0 {
            InvalidateRect(w.center, std::ptr::null(), 0);
        }
    }
    true
}

// =============================================================== avisos emergentes
struct Shown {
    note: Note,
    /// Marca de tiempo (GetTickCount) en la que apareció.
    born: u32,
    close_rc: RECT,
    rect: RECT,
}

struct Toaster {
    shown: Vec<Shown>,
    /// Avisos que esperan hueco.
    queue: Vec<Note>,
    hover: isize,
    hover_close: isize,
}

fn er() -> RECT {
    RECT { left: 0, top: 0, right: 0, bottom: 0 }
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
fn in_rect(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn toast_w() -> i32 {
    ui::s(config::current().get(Id::NotifAncho))
}
fn toast_h() -> i32 {
    ui::s(84)
}
fn toast_gap() -> i32 {
    ui::s(10)
}
fn toast_margin() -> i32 {
    ui::s(12)
}

/// Encola un aviso en la pila de emergentes.
fn push_toast(hwnd: HWND, note: Note) {
    if hwnd.is_null() {
        return;
    }
    unsafe {
        let t = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Toaster;
        if t.is_null() {
            return;
        }
        let t = &mut *t;
        let max = config::current().get(Id::NotifMax).max(1) as usize;
        if t.shown.len() >= max {
            t.queue.push(note);
        } else {
            t.shown.push(Shown { note, born: GetTickCount(), close_rc: er(), rect: er() });
        }
        reflow(hwnd, t);
        SetTimer(hwnd, 1, 200, None);
    }
}

/// Recoloca la ventana de la pila y los rectángulos de cada aviso.
fn reflow(hwnd: HWND, t: &mut Toaster) {
    unsafe {
        if t.shown.is_empty() {
            ShowWindow(hwnd, SW_HIDE);
            return;
        }
        let c = config::current();
        let tw = toast_w();
        let th = toast_h();
        let gap = toast_gap();
        let margin = toast_margin();
        let w = tw + margin * 2;
        let h = t.shown.len() as i32 * (th + gap) - gap + margin * 2;

        let bar = crate::shell::bar_rect();
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        let top_bar = c.get(Id::BarraPosicion) == 1;
        let x = match c.get(Id::NotifPosicion) {
            1 => margin,
            2 => (sw - w) / 2,
            _ => sw - w - margin,
        };
        let y = if top_bar { bar.bottom + margin } else { sh - rh(&bar) - h - margin };

        for (i, s) in t.shown.iter_mut().enumerate() {
            let ty = margin + i as i32 * (th + gap);
            s.rect = rect(margin, ty, tw, th);
            s.close_rc = rect(s.rect.right - ui::s(30), ty + ui::s(8), ui::s(22), ui::s(22));
        }
        SetWindowPos(hwnd, HWND_TOPMOST, x, y.max(0), w, h,
                     SWP_NOACTIVATE | SWP_SHOWWINDOW);
        InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}

fn dismiss(hwnd: HWND, t: &mut Toaster, i: usize) {
    if i < t.shown.len() {
        t.shown.remove(i);
    }
    let max = config::current().get(Id::NotifMax).max(1) as usize;
    while t.shown.len() < max && !t.queue.is_empty() {
        let n = t.queue.remove(0);
        t.shown.push(Shown { note: n, born: unsafe { GetTickCount() }, close_rc: er(), rect: er() });
    }
    t.hover = -1;
    t.hover_close = -1;
    reflow(hwnd, t);
}

fn paint_toasts(hwnd: HWND, t: &Toaster) {
    window::paint_buffered(hwnd, |g: &Gfx, w, h| {
        let s = theme::style();
        g.clear(theme::argb(0, 0, 0, 0));
        let _ = (w, h);
        for (i, sh) in t.shown.iter().enumerate() {
            let r = &sh.rect;
            let hov = t.hover == i as isize;
            g.rounded(r.left as f32, r.top as f32, rw(r) as f32, rh(r) as f32, ui::sf(10.0),
                      Some(if hov { s.menu_hover } else { s.flyout_bg }), Some((s.flyout_border, 1.0)));
            // franja del color según el tipo
            let col = sh.note.kind.color(&s);
            g.rounded((r.left + ui::s(6)) as f32, (r.top + ui::s(10)) as f32, ui::s(3) as f32,
                      (rh(r) - ui::s(20)) as f32, ui::sf(2.0), Some(col), None);
            g.text_in(sh.note.kind.glyph(), (r.left + ui::s(16)) as f32, (r.top + ui::s(10)) as f32,
                      ui::s(26) as f32, ui::s(26) as f32, 13.0, false, col, Align::Center, Align::Center, true);
            let tx = r.left + ui::s(48);
            let tw = r.right - tx - ui::s(36);
            g.text_in(&sh.note.app, tx as f32, (r.top + ui::s(8)) as f32, tw as f32, ui::s(16) as f32,
                      10.0, false, s.text_faint, Align::Near, Align::Center, false);
            g.text_in(&sh.note.title, tx as f32, (r.top + ui::s(24)) as f32, tw as f32, ui::s(20) as f32,
                      13.0, true, s.text, Align::Near, Align::Center, false);
            g.text_in(&sh.note.body, tx as f32, (r.top + ui::s(44)) as f32, tw as f32, ui::s(32) as f32,
                      11.5, false, s.text_dim, Align::Near, Align::Near, false);
            // cerrar
            let cr = &sh.close_rc;
            if t.hover_close == i as isize {
                g.rounded(cr.left as f32, cr.top as f32, rw(cr) as f32, rh(cr) as f32, ui::sf(4.0),
                          Some(s.control_hover), None);
            }
            g.text_in("\u{E8BB}", cr.left as f32, cr.top as f32, rw(cr) as f32, rh(cr) as f32, 8.0, false,
                      s.text_dim, Align::Center, Align::Center, true);
            // barra de tiempo restante
            let dur = config::current().get(Id::NotifDuracion).max(1) as u32;
            let left = dur.saturating_sub(unsafe { GetTickCount() }.wrapping_sub(sh.born));
            let frac = left as f32 / dur as f32;
            if frac > 0.0 {
                g.rounded((r.left + ui::s(10)) as f32, (r.bottom - ui::s(5)) as f32,
                          ((rw(r) - ui::s(20)) as f32 * frac.min(1.0)).max(1.0), ui::s(2) as f32,
                          1.0, Some(theme::with_alpha(col, 140)), None);
            }
        }
    });
}

unsafe extern "system" fn toast_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let t_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Toaster;
    if msg == ui::config_changed_msg() {
        if !t_ptr.is_null() {
            reflow(hwnd, &mut *t_ptr);
        }
        return 0;
    }
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            if !t_ptr.is_null() {
                paint_toasts(hwnd, &*t_ptr);
            }
            0
        }
        WM_TIMER => {
            if !t_ptr.is_null() {
                let t = &mut *t_ptr;
                let dur = config::current().get(Id::NotifDuracion).max(500) as u32;
                let now = GetTickCount();
                // El aviso bajo el puntero no caduca: se da tiempo a leerlo.
                let expired = t
                    .shown
                    .iter()
                    .position(|s| now.wrapping_sub(s.born) > dur)
                    .filter(|&i| t.hover != i as isize);
                if let Some(i) = expired {
                    dismiss(hwnd, t, i);
                } else {
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                if t.shown.is_empty() {
                    KillTimer(hwnd, 1);
                }
            }
            0
        }
        WM_MOUSEMOVE => {
            if !t_ptr.is_null() {
                let t = &mut *t_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                let mut hov = -1isize;
                let mut hovc = -1isize;
                for (i, s) in t.shown.iter().enumerate() {
                    if in_rect(&s.rect, x, y) {
                        hov = i as isize;
                        if in_rect(&s.close_rc, x, y) {
                            hovc = i as isize;
                        }
                        break;
                    }
                }
                if hov != t.hover || hovc != t.hover_close {
                    t.hover = hov;
                    t.hover_close = hovc;
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !t_ptr.is_null() {
                let t = &mut *t_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                if let Some(i) = t.shown.iter().position(|s| in_rect(&s.rect, x, y)) {
                    // Pulsar en el cuerpo marca el aviso como leído; la X solo lo quita.
                    if !in_rect(&t.shown[i].close_rc, x, y) {
                        mark_all_read();
                    }
                    dismiss(hwnd, t, i);
                }
            }
            0
        }
        WM_DESTROY => {
            if !t_ptr.is_null() {
                drop(Box::from_raw(t_ptr));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// =============================================================== centro de notificaciones
#[derive(Clone, Copy, PartialEq)]
enum CHit {
    None,
    Row(usize),
    RowClose(usize),
    ClearAll,
    Dnd,
    Settings,
}

struct Center {
    rows: Vec<RECT>,
    closes: Vec<RECT>,
    scroll: i32,
    hover: CHit,
    clear_rc: RECT,
    dnd_rc: RECT,
    settings_rc: RECT,
    content_h: i32,
    list_rc: RECT,
}

fn center_metrics() -> (i32, i32) {
    (ui::s(390), ui::s(540))
}

fn row_h() -> i32 {
    ui::s(78)
}

fn center_layout(hwnd: HWND, c: &mut Center) {
    let mut rc = er();
    unsafe { GetClientRect(hwnd, &mut rc) };
    let pad = ui::s(12);
    let head = ui::s(52);
    let foot = ui::s(52);
    c.list_rc = RECT { left: pad, top: head, right: rc.right - pad, bottom: rc.bottom - foot };
    let n = count();
    let rh_ = row_h();
    c.content_h = n as i32 * (rh_ + ui::s(8));
    let max = (c.content_h - rh(&c.list_rc)).max(0);
    c.scroll = c.scroll.clamp(0, max);
    c.rows.clear();
    c.closes.clear();
    for i in 0..n {
        let y = c.list_rc.top - c.scroll + i as i32 * (rh_ + ui::s(8));
        let r = rect(c.list_rc.left, y, rw(&c.list_rc), rh_);
        c.closes.push(rect(r.right - ui::s(30), r.top + ui::s(8), ui::s(22), ui::s(22)));
        c.rows.push(r);
    }
    let by = rc.bottom - ui::s(42);
    c.dnd_rc = rect(pad, by, ui::s(132), ui::s(32));
    c.settings_rc = rect(pad + ui::s(140), by, ui::s(112), ui::s(32));
    c.clear_rc = rect(rc.right - pad - ui::s(104), by, ui::s(104), ui::s(32));
}

fn center_hit(c: &Center, x: i32, y: i32) -> CHit {
    if in_rect(&c.clear_rc, x, y) {
        return CHit::ClearAll;
    }
    if in_rect(&c.dnd_rc, x, y) {
        return CHit::Dnd;
    }
    if in_rect(&c.settings_rc, x, y) {
        return CHit::Settings;
    }
    if y < c.list_rc.top || y > c.list_rc.bottom {
        return CHit::None;
    }
    for (i, r) in c.rows.iter().enumerate() {
        if in_rect(r, x, y) {
            if in_rect(&c.closes[i], x, y) {
                return CHit::RowClose(i);
            }
            return CHit::Row(i);
        }
    }
    CHit::None
}

fn paint_center(hwnd: HWND, c: &Center) {
    let list = notes();
    let cfg = config::current();
    window::paint_buffered(hwnd, |g: &Gfx, w, h| {
        let s = theme::style();
        g.clear(theme::argb(0, 0, 0, 0));
        g.rounded(0.0, 0.0, w as f32, h as f32, ui::sf(10.0), Some(s.flyout_bg), Some((s.flyout_border, 1.0)));
        g.text_in("Notificaciones", ui::s(16) as f32, ui::s(12) as f32, (w - ui::s(32)) as f32,
                  ui::s(28) as f32, 15.0, true, s.text, Align::Near, Align::Center, false);
        if !list.is_empty() {
            g.text_in(&format!("{}", list.len()), (w - ui::s(60)) as f32, ui::s(12) as f32, ui::s(44) as f32,
                      ui::s(28) as f32, 12.0, false, s.text_faint, Align::Far, Align::Center, false);
        }

        if list.is_empty() {
            g.text_in("\u{E7E7}", 0.0, (h / 2 - ui::s(60)) as f32, w as f32, ui::s(40) as f32, 22.0, false,
                      s.text_faint, Align::Center, Align::Center, true);
            g.text_in("No hay notificaciones nuevas", 0.0, (h / 2 - ui::s(14)) as f32, w as f32,
                      ui::s(28) as f32, 12.5, false, s.text_dim, Align::Center, Align::Center, false);
        } else {
            let lr = c.list_rc;
            g.set_clip(lr.left, lr.top, rw(&lr), rh(&lr));
            for (i, n) in list.iter().enumerate() {
                let Some(r) = c.rows.get(i) else { continue };
                if r.bottom < lr.top || r.top > lr.bottom {
                    continue;
                }
                let hov = matches!(c.hover, CHit::Row(j) | CHit::RowClose(j) if j == i);
                g.rounded(r.left as f32, r.top as f32, rw(r) as f32, rh(r) as f32, ui::sf(8.0),
                          Some(if hov { s.menu_hover } else { s.card_bg }), Some((s.card_border, 1.0)));
                let col = n.kind.color(&s);
                if !n.read {
                    g.fill_circle((r.left + ui::s(12)) as f32, (r.top + ui::s(16)) as f32, ui::sf(3.5), col);
                }
                g.text_in(n.kind.glyph(), (r.left + ui::s(18)) as f32, (r.top + ui::s(8)) as f32,
                          ui::s(24) as f32, ui::s(24) as f32, 12.0, false, col, Align::Center, Align::Center, true);
                let tx = r.left + ui::s(48);
                let tw = r.right - tx - ui::s(40);
                g.text_in(&n.app, tx as f32, (r.top + ui::s(7)) as f32, tw as f32, ui::s(15) as f32, 10.0,
                          false, s.text_faint, Align::Near, Align::Center, false);
                g.text_in(&n.time, (r.right - ui::s(74)) as f32, (r.top + ui::s(7)) as f32, ui::s(40) as f32,
                          ui::s(15) as f32, 10.0, false, s.text_faint, Align::Far, Align::Center, false);
                g.text_in(&n.title, tx as f32, (r.top + ui::s(22)) as f32, tw as f32, ui::s(20) as f32, 12.5,
                          true, s.text, Align::Near, Align::Center, false);
                g.text_in(&n.body, tx as f32, (r.top + ui::s(42)) as f32, tw as f32, ui::s(30) as f32, 11.0,
                          false, s.text_dim, Align::Near, Align::Near, false);
                if let Some(cr) = c.closes.get(i) {
                    if c.hover == CHit::RowClose(i) {
                        g.rounded(cr.left as f32, cr.top as f32, rw(cr) as f32, rh(cr) as f32, ui::sf(4.0),
                                  Some(s.control_hover), None);
                    }
                    if hov {
                        g.text_in("\u{E8BB}", cr.left as f32, cr.top as f32, rw(cr) as f32, rh(cr) as f32,
                                  8.0, false, s.text_dim, Align::Center, Align::Center, true);
                    }
                }
            }
            g.reset_clip();
            let max = (c.content_h - rh(&lr)).max(0);
            if max > 0 {
                let bar = (rh(&lr) * rh(&lr) / c.content_h.max(1)).max(ui::s(30));
                let y = lr.top + (rh(&lr) - bar) * c.scroll / max;
                g.rounded((lr.right - ui::s(3)) as f32, y as f32, ui::s(3) as f32, bar as f32, ui::sf(2.0),
                          Some(s.track), None);
            }
        }

        // pie: no molestar, configuración y borrar todo
        g.hline(ui::s(12), w - ui::s(12), h - ui::s(52), s.separator);
        let dnd = cfg.b(Id::NotifNoMolestar);
        small_button(g, &s, c.dnd_rc, if dnd { "\u{E711}" } else { "\u{E7E7}" }, "No molestar",
                     c.hover == CHit::Dnd, dnd);
        small_button(g, &s, c.settings_rc, "\u{E713}", "Ajustes", c.hover == CHit::Settings, false);
        small_button(g, &s, c.clear_rc, "\u{E74D}", "Borrar todo", c.hover == CHit::ClearAll, false);
    });
}

fn small_button(g: &Gfx, s: &theme::Style, r: RECT, glyph: &str, label: &str, hov: bool, active: bool) {
    let fill = if active {
        s.accent_solid
    } else if hov {
        s.control_hover
    } else {
        s.control_bg
    };
    let fg = if active { s.on_accent } else { s.text };
    g.rounded(r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32, ui::sf(6.0), Some(fill),
              Some((s.control_border, 1.0)));
    g.text_in(glyph, (r.left + ui::s(8)) as f32, r.top as f32, ui::s(18) as f32, rh(&r) as f32, 9.5, false,
              fg, Align::Center, Align::Center, true);
    g.text_in(label, (r.left + ui::s(28)) as f32, r.top as f32, (rw(&r) - ui::s(34)) as f32, rh(&r) as f32,
              11.5, false, fg, Align::Near, Align::Center, false);
}

unsafe extern "system" fn center_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let c_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Center;
    if msg == ui::config_changed_msg() {
        if !c_ptr.is_null() {
            center_layout(hwnd, &mut *c_ptr);
            InvalidateRect(hwnd, std::ptr::null(), 0);
        }
        return 0;
    }
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            if !c_ptr.is_null() {
                paint_center(hwnd, &*c_ptr);
            }
            0
        }
        WM_SIZE => {
            if !c_ptr.is_null() {
                center_layout(hwnd, &mut *c_ptr);
            }
            0
        }
        WM_MOUSEMOVE => {
            if !c_ptr.is_null() {
                let c = &mut *c_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                let h = center_hit(c, x, y);
                if h != c.hover {
                    c.hover = h;
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            if !c_ptr.is_null() {
                let c = &mut *c_ptr;
                let delta = ((wp >> 16) & 0xFFFF) as i16 as i32;
                c.scroll -= (delta / 120) * ui::s(60);
                center_layout(hwnd, c);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !c_ptr.is_null() {
                let c = &mut *c_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                match center_hit(c, x, y) {
                    CHit::RowClose(i) => {
                        remove(i);
                        center_layout(hwnd, c);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                    CHit::Row(_) => {
                        mark_all_read();
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                    CHit::ClearAll => {
                        clear_all();
                        center_layout(hwnd, c);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                    CHit::Dnd => {
                        let mut cfg = config::current();
                        cfg.toggle(Id::NotifNoMolestar);
                        ui::apply_config(&cfg, true);
                        ui::broadcast_config_changed();
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                    CHit::Settings => {
                        ShowWindow(hwnd, SW_HIDE);
                        crate::shell::launch_own("--settings notificaciones");
                    }
                    CHit::None => {}
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
            if !c_ptr.is_null() {
                drop(Box::from_raw(c_ptr));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// =============================================================== creación
fn register(hinst: HINSTANCE, class: &str, proc_: WNDPROC) {
    unsafe {
        let name = ui::wide(class);
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: proc_,
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: name.as_ptr(),
        };
        RegisterClassW(&wc);
    }
}

/// Crea las ventanas de avisos y del centro de notificaciones (ocultas).
pub fn create(hinst: HINSTANCE) -> Windows {
    unsafe {
        register(hinst, TOAST_CLASS, Some(toast_proc));
        register(hinst, CENTER_CLASS, Some(center_proc));

        let toasts = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            ui::wide(TOAST_CLASS).as_ptr(),
            ui::wide("Avisos").as_ptr(),
            WS_POPUP,
            0, 0, 10, 10,
            std::ptr::null_mut(), std::ptr::null_mut(), hinst, std::ptr::null(),
        );
        if !toasts.is_null() {
            let t = Box::new(Toaster { shown: Vec::new(), queue: Vec::new(), hover: -1, hover_close: -1 });
            SetWindowLongPtrW(toasts, GWLP_USERDATA, Box::into_raw(t) as isize);
            window::apply_frame_opaque(toasts, config::current().dark());
        }

        let (cw, chh) = center_metrics();
        let center = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            ui::wide(CENTER_CLASS).as_ptr(),
            ui::wide("Notificaciones").as_ptr(),
            WS_POPUP,
            0, 0, cw, chh,
            std::ptr::null_mut(), std::ptr::null_mut(), hinst, std::ptr::null(),
        );
        if !center.is_null() {
            let c = Box::new(Center {
                rows: Vec::new(), closes: Vec::new(), scroll: 0, hover: CHit::None,
                clear_rc: er(), dnd_rc: er(), settings_rc: er(), content_h: 0, list_rc: er(),
            });
            SetWindowLongPtrW(center, GWLP_USERDATA, Box::into_raw(c) as isize);
            window::apply_frame_opaque(center, config::current().dark());
            center_layout(center, &mut *(GetWindowLongPtrW(center, GWLP_USERDATA) as *mut Center));
        }
        Windows { toasts, center }
    }
}

/// Abre o cierra el centro de notificaciones, anclado a la barra.
pub fn toggle_center(center: HWND, anchor_x: i32) {
    unsafe {
        if center.is_null() {
            return;
        }
        if IsWindowVisible(center) != 0 {
            ShowWindow(center, SW_HIDE);
            return;
        }
        mark_all_read();
        let (w, h) = center_metrics();
        let bar = crate::shell::bar_rect();
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let x = (anchor_x - w / 2).clamp(ui::s(8), (sw - w - ui::s(8)).max(0));
        let y = if config::current().get(Id::BarraPosicion) == 1 {
            bar.bottom + ui::s(8)
        } else {
            bar.top - h - ui::s(8)
        };
        SetWindowPos(center, HWND_TOPMOST, x, y.max(0), w, h, SWP_SHOWWINDOW);
        let c_ptr = GetWindowLongPtrW(center, GWLP_USERDATA) as *mut Center;
        if !c_ptr.is_null() {
            (*c_ptr).scroll = 0;
            (*c_ptr).hover = CHit::None;
            center_layout(center, &mut *c_ptr);
        }
        SetForegroundWindow(center);
        InvalidateRect(center, std::ptr::null(), 0);
    }
}
