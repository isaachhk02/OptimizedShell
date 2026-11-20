// Menú contextual propio, con el tema de OptimizedShell.
//
// Windows ya trae menús (TrackPopupMenu), pero no siguen el tema ni la escala
// de la aplicación. Este es una ventana emergente más, dibujada con GDI+ como
// el resto de la interfaz, con su propio bucle de mensajes modal: `popup`
// devuelve el identificador elegido (0 si se cierra sin elegir).
//
// Admite elementos marcables que no cierran el menú (`keep_open`), que es lo
// que necesita el selector de afinidad del administrador de tareas.
#![allow(dead_code)]

use std::cell::RefCell;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_ESCAPE, VK_RETURN, VK_UP};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::gdip::{Align, Gfx};
use super::{self as ui, theme, window};

pub const CLASS: &str = "OShellRustMenu";

#[derive(Clone)]
pub struct Item {
    pub id: i32,
    pub label: String,
    /// Texto secundario a la derecha (atajo, valor actual...).
    pub hint: String,
    pub glyph: &'static str,
    pub checked: bool,
    pub enabled: bool,
    pub separator: bool,
    /// Encabezado de sección: ni se puede elegir ni se resalta.
    pub header: bool,
    /// Al elegirlo el menú no se cierra (casillas que se marcan en grupo).
    pub keep_open: bool,
    pub danger: bool,
}

impl Default for Item {
    fn default() -> Self {
        Item {
            id: 0,
            label: String::new(),
            hint: String::new(),
            glyph: "",
            checked: false,
            enabled: true,
            separator: false,
            header: false,
            keep_open: false,
            danger: false,
        }
    }
}

impl Item {
    pub fn new(id: i32, label: &str) -> Item {
        Item { id, label: label.to_string(), ..Default::default() }
    }
    pub fn glyph(mut self, g: &'static str) -> Item {
        self.glyph = g;
        self
    }
    pub fn hint(mut self, h: &str) -> Item {
        self.hint = h.to_string();
        self
    }
    pub fn checked(mut self, v: bool) -> Item {
        self.checked = v;
        self
    }
    pub fn enabled(mut self, v: bool) -> Item {
        self.enabled = v;
        self
    }
    pub fn danger(mut self) -> Item {
        self.danger = true;
        self
    }
    pub fn keep_open(mut self) -> Item {
        self.keep_open = true;
        self
    }
    pub fn sep() -> Item {
        Item { separator: true, enabled: false, ..Default::default() }
    }
    pub fn header(label: &str) -> Item {
        Item { label: label.to_string(), header: true, enabled: false, ..Default::default() }
    }
}

struct Menu {
    items: Vec<Item>,
    rects: Vec<RECT>,
    hover: isize,
    /// Identificador elegido; -1 mientras no se ha elegido nada.
    result: i32,
    done: bool,
}

thread_local! {
    static MENU: RefCell<Option<Menu>> = RefCell::new(None);
}

fn er() -> RECT {
    RECT { left: 0, top: 0, right: 0, bottom: 0 }
}
fn in_rect(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}
fn rw(r: &RECT) -> i32 {
    r.right - r.left
}
fn rh(r: &RECT) -> i32 {
    r.bottom - r.top
}

fn item_h() -> i32 {
    ui::s(30)
}
fn sep_h() -> i32 {
    ui::s(9)
}
fn header_h() -> i32 {
    ui::s(26)
}
fn pad() -> i32 {
    ui::s(6)
}

/// Ancho que necesita el menú para que quepa el texto más largo.
/// El cálculo tiene en cuenta el ajuste «Tamaño del texto»: si no, con la
/// escala alta las etiquetas se cortarían.
fn measure(items: &[Item]) -> (i32, i32) {
    let per_char = ui::s(7) * super::config::font_pct() / 100;
    let mut w = ui::s(150);
    for it in items {
        if it.separator {
            continue;
        }
        let chars = it.label.chars().count() as i32 + it.hint.chars().count() as i32;
        let need = ui::s(74) + chars * per_char + if it.hint.is_empty() { 0 } else { ui::s(18) };
        w = w.max(need);
    }
    let w = w.min(ui::s(520));
    let mut h = pad() * 2;
    for it in items {
        h += if it.separator {
            sep_h()
        } else if it.header {
            header_h()
        } else {
            item_h()
        };
    }
    (w, h)
}

fn layout(m: &mut Menu, w: i32) {
    m.rects.clear();
    let mut y = pad();
    for it in &m.items {
        let hgt = if it.separator {
            sep_h()
        } else if it.header {
            header_h()
        } else {
            item_h()
        };
        m.rects.push(RECT { left: pad(), top: y, right: w - pad(), bottom: y + hgt });
        y += hgt;
    }
}

fn paint(hwnd: HWND, m: &Menu) {
    window::paint_buffered(hwnd, |g: &Gfx, w, h| {
        let s = theme::style();
        g.clear(theme::argb(0, 0, 0, 0));
        g.rounded(0.0, 0.0, w as f32, h as f32, ui::sf(8.0), Some(s.menu_bg), Some((s.menu_border, 1.0)));
        for (i, it) in m.items.iter().enumerate() {
            let r = m.rects[i];
            if it.separator {
                g.hline(r.left + ui::s(8), r.right - ui::s(8), (r.top + r.bottom) / 2, s.separator);
                continue;
            }
            if it.header {
                g.text_in(&it.label, (r.left + ui::s(12)) as f32, r.top as f32, rw(&r) as f32,
                          rh(&r) as f32, 10.5, true, s.text_faint, Align::Near, Align::Far, false);
                continue;
            }
            let hov = m.hover == i as isize && it.enabled;
            if hov {
                g.rounded(r.left as f32, r.top as f32, rw(&r) as f32, (rh(&r) - ui::s(2)) as f32,
                          ui::sf(5.0), Some(if it.danger { s.close_hover } else { s.menu_hover }), None);
            }
            let fg = if !it.enabled {
                theme::with_alpha(s.text_faint, 150)
            } else if hov && it.danger {
                theme::rgbc(255, 255, 255)
            } else if it.danger {
                s.danger
            } else {
                s.text
            };
            // Marca de verificación o glifo, en la misma columna.
            if it.checked {
                g.check((r.left + ui::s(18)) as f32, (r.top + rh(&r) / 2) as f32, ui::sf(11.0),
                        if it.enabled { s.accent } else { fg }, ui::sf(1.8));
            } else if !it.glyph.is_empty() {
                g.text_in(it.glyph, (r.left + ui::s(8)) as f32, r.top as f32, ui::s(20) as f32,
                          rh(&r) as f32, 10.5, false, if it.enabled { s.accent } else { fg },
                          Align::Center, Align::Center, true);
            }
            let tx = r.left + ui::s(34);
            let hint_w = if it.hint.is_empty() { 0 } else { ui::s(12) + it.hint.chars().count() as i32 * ui::s(7) };
            g.text_in(&it.label, tx as f32, r.top as f32, (rw(&r) - ui::s(40) - hint_w) as f32,
                      rh(&r) as f32, 12.0, false, fg, Align::Near, Align::Center, false);
            if !it.hint.is_empty() {
                g.text_in(&it.hint, (r.right - hint_w - ui::s(8)) as f32, r.top as f32, hint_w as f32,
                          rh(&r) as f32, 11.0, false, s.text_faint, Align::Far, Align::Center, false);
            }
        }
    });
}

fn hit(m: &Menu, x: i32, y: i32) -> isize {
    for (i, r) in m.rects.iter().enumerate() {
        if in_rect(r, x, y) && !m.items[i].separator && !m.items[i].header {
            return i as isize;
        }
    }
    -1
}

/// Mueve la selección saltando separadores y encabezados.
fn move_sel(m: &mut Menu, dir: isize) {
    let n = m.items.len() as isize;
    if n == 0 {
        return;
    }
    let mut i = m.hover;
    for _ in 0..n {
        i = (i + dir).rem_euclid(n);
        let it = &m.items[i as usize];
        if !it.separator && !it.header && it.enabled {
            m.hover = i;
            return;
        }
    }
}

fn choose(hwnd: HWND, m: &mut Menu, i: usize) {
    let it = &m.items[i];
    if !it.enabled || it.separator || it.header {
        return;
    }
    m.result = it.id;
    if it.keep_open {
        // Casilla de un grupo: se marca y el menú sigue abierto.
        m.items[i].checked = !m.items[i].checked;
        unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
    } else {
        m.done = true;
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            MENU.with(|c| {
                if let Some(m) = c.borrow().as_ref() {
                    paint(hwnd, m);
                }
            });
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
            let changed = MENU.with(|c| {
                let mut b = c.borrow_mut();
                let Some(m) = b.as_mut() else { return false };
                let h = hit(m, x, y);
                if h != m.hover {
                    m.hover = h;
                    return true;
                }
                false
            });
            if changed {
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
            MENU.with(|c| {
                let mut b = c.borrow_mut();
                if let Some(m) = b.as_mut() {
                    let h = hit(m, x, y);
                    if h >= 0 {
                        choose(hwnd, m, h as usize);
                    }
                }
            });
            0
        }
        WM_KEYDOWN => {
            let vk = wp as u32;
            MENU.with(|c| {
                let mut b = c.borrow_mut();
                let Some(m) = b.as_mut() else { return };
                if vk == VK_ESCAPE as u32 {
                    m.result = 0;
                    m.done = true;
                } else if vk == VK_DOWN as u32 {
                    move_sel(m, 1);
                } else if vk == VK_UP as u32 {
                    move_sel(m, -1);
                } else if vk == VK_RETURN as u32 && m.hover >= 0 {
                    let i = m.hover as usize;
                    choose(hwnd, m, i);
                }
            });
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_ACTIVATE => {
            // Pulsar fuera cierra el menú sin elegir nada.
            if (wp & 0xFFFF) == WA_INACTIVE as usize {
                MENU.with(|c| {
                    let mut b = c.borrow_mut();
                    if let Some(m) = b.as_mut() {
                        if !m.done {
                            m.result = 0;
                            m.done = true;
                        }
                    }
                });
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub fn register(hinst: windows_sys::Win32::Foundation::HINSTANCE) {
    unsafe {
        let name = ui::wide(CLASS);
        let wc = WNDCLASSW {
            style: CS_SAVEBITS,
            lpfnWndProc: Some(wndproc),
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

/// Resultado de un menú con casillas: el identificador elegido y el estado
/// final de cada elemento.
pub struct Outcome {
    pub id: i32,
    pub checked: Vec<bool>,
}

/// Muestra el menú en coordenadas de pantalla y espera a que se elija algo.
/// Devuelve el identificador (0 si se cerró sin elegir).
pub fn popup(owner: HWND, x: i32, y: i32, items: Vec<Item>) -> i32 {
    popup_ex(owner, x, y, items).id
}

/// Como `popup`, pero devuelve además el estado de las casillas.
pub fn popup_ex(owner: HWND, x: i32, y: i32, items: Vec<Item>) -> Outcome {
    unsafe {
        let hinst = ui::hinst();
        register(hinst);
        let (w, h) = measure(&items);
        // Se coloca dentro de la pantalla: si no cabe debajo, va encima.
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        let px = x.min(sw - w - ui::s(4)).max(ui::s(4));
        let py = if y + h > sh - ui::s(4) { (y - h).max(ui::s(4)) } else { y };

        let mut m = Menu { items, rects: Vec::new(), hover: -1, result: 0, done: false };
        layout(&mut m, w);
        let n = m.items.len();
        MENU.with(|c| *c.borrow_mut() = Some(m));

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            ui::wide(CLASS).as_ptr(),
            ui::wide("").as_ptr(),
            WS_POPUP,
            px, py, w, h,
            owner,
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            MENU.with(|c| *c.borrow_mut() = None);
            return Outcome { id: 0, checked: vec![false; n] };
        }
        window::apply_frame_opaque(hwnd, super::config::current().dark());
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetForegroundWindow(hwnd);
        set_focus_safe(hwnd);

        // Bucle modal propio: no se vuelve hasta que el menú se cierra.
        let mut msg: MSG = std::mem::zeroed();
        loop {
            let done = MENU.with(|c| c.borrow().as_ref().map(|m| m.done).unwrap_or(true));
            if done {
                break;
            }
            if GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) <= 0 {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        DestroyWindow(hwnd);
        let out = MENU.with(|c| {
            let m = c.borrow_mut().take();
            match m {
                Some(m) => Outcome {
                    id: m.result,
                    checked: m.items.iter().map(|i| i.checked).collect(),
                },
                None => Outcome { id: 0, checked: vec![false; n] },
            }
        });
        // Devuelve el foco a quien abrió el menú.
        if !owner.is_null() {
            SetForegroundWindow(owner);
        }
        out
    }
}

#[inline]
unsafe fn set_focus_safe(hwnd: HWND) {
    windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(hwnd);
}
