// Tema compartido: tokens de color calculados a partir de la configuración.
// Todos los colores de la interfaz salen de estos tokens; shell, explorador,
// administrador de tareas, menús y diálogos comparten tema.
//
// La persistencia vive en `ui::config` (config.ini); aquí solo se traducen los
// ajustes de apariencia (modo, acento, opacidades, tinte y contraste) a colores.
#![allow(dead_code)]

use std::sync::Mutex;

use super::config::{self, Config, Id};

/// Color GDI+ en formato ARGB (0xAARRGGBB).
pub type Color = u32;

#[inline]
pub const fn argb(a: u8, r: u8, g: u8, b: u8) -> Color {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}
#[inline]
pub const fn rgbc(r: u8, g: u8, b: u8) -> Color {
    argb(255, r, g, b)
}
#[inline]
pub const fn a_of(c: Color) -> u8 { (c >> 24) as u8 }
#[inline]
pub const fn r_of(c: Color) -> u8 { (c >> 16) as u8 }
#[inline]
pub const fn g_of(c: Color) -> u8 { (c >> 8) as u8 }
#[inline]
pub const fn b_of(c: Color) -> u8 { c as u8 }

#[inline]
pub fn with_alpha(c: Color, a: u8) -> Color {
    argb(a, r_of(c), g_of(c), b_of(c))
}

/// Mezcla a->b con factor t (0..1), con alfa resultante dado.
pub fn mix(a: Color, b: Color, t: f32, alpha: u8) -> Color {
    let m = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t + 0.5) as u8 };
    argb(alpha, m(r_of(a), r_of(b)), m(g_of(a), g_of(b)), m(b_of(a), b_of(b)))
}

/// Mezcla conservando el alfa del color original.
fn tint(c: Color, towards: Color, t: f32) -> Color {
    mix(c, towards, t, a_of(c))
}

/// Opacidad 0-100 -> canal alfa 0-255.
fn opacity_alpha(op: i32) -> u8 {
    let op = op.clamp(0, 100);
    ((op * 255 + 50) / 100) as u8
}

/// COLORREF de Windows (0x00BBGGRR) -> Color ARGB.
fn from_ref(c: u32, a: u8) -> Color {
    argb(a, (c & 0xFF) as u8, ((c >> 8) & 0xFF) as u8, ((c >> 16) & 0xFF) as u8)
}

/// COLORREF (0x00BBGGRR) -> Color ARGB opaco.
pub fn from_colorref(c: u32) -> Color {
    from_ref(c, 255)
}

#[inline]
pub const fn colorref(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

pub struct AccentPreset {
    pub color: u32,
    pub name: &'static str,
}

pub const ACCENT_PRESETS: &[AccentPreset] = &[
    AccentPreset { color: colorref(59, 130, 246), name: "Azul" },
    AccentPreset { color: colorref(14, 165, 233), name: "Cielo" },
    AccentPreset { color: colorref(20, 184, 166), name: "Turquesa" },
    AccentPreset { color: colorref(34, 197, 94), name: "Verde" },
    AccentPreset { color: colorref(132, 204, 22), name: "Lima" },
    AccentPreset { color: colorref(234, 179, 8), name: "Ámbar" },
    AccentPreset { color: colorref(249, 115, 22), name: "Naranja" },
    AccentPreset { color: colorref(239, 68, 68), name: "Rojo" },
    AccentPreset { color: colorref(236, 72, 153), name: "Rosa" },
    AccentPreset { color: colorref(139, 92, 246), name: "Violeta" },
    AccentPreset { color: colorref(100, 116, 139), name: "Pizarra" },
];

// ------------------------------------------------------------ tokens de estilo
#[derive(Clone, Copy)]
pub struct Style {
    pub dark: bool,
    pub blur: bool,
    pub taskbar_opacity: i32,
    pub window_opacity: i32,
    pub window_bg: Color,
    pub title_bg: Color,
    pub card_bg: Color,
    pub card_border: Color,
    pub text: Color,
    pub text_dim: Color,
    pub text_faint: Color,
    pub accent: Color,
    pub accent_solid: Color,
    pub accent_hover: Color,
    pub on_accent: Color,
    pub row_selected: Color,
    pub focus_ring: Color,
    pub danger: Color,
    pub success: Color,
    pub warning: Color,
    pub close_hover: Color,
    pub row_hover: Color,
    pub control_bg: Color,
    pub control_hover: Color,
    pub control_border: Color,
    pub separator: Color,
    pub nav_bg: Color,
    pub dialog_bg: Color,
    pub menu_bg: Color,
    pub menu_hover: Color,
    pub menu_border: Color,
    pub taskbar_bg: Color,
    pub task_button: Color,
    pub task_button_hover: Color,
    pub task_button_active: Color,
    pub flyout_bg: Color,
    pub flyout_border: Color,
    pub graph_bg: Color,
    pub grid_line: Color,
    pub track: Color,
    pub taskbar_tint: u32,
    pub flyout_tint: u32,
}

static STYLE: Mutex<Option<Style>> = Mutex::new(None);

/// Devuelve una copia del estilo actual. Barato (todos los campos son Copy).
pub fn style() -> Style {
    let cached = *STYLE.lock().unwrap();
    match cached {
        Some(s) => s,
        None => {
            // Primer uso sin apply(): calcula con la configuración activa.
            let s = compute(&config::current());
            *STYLE.lock().unwrap() = Some(s);
            s
        }
    }
}

/// Recalcula los tokens a partir de la configuración y los publica.
pub fn apply(c: &Config) {
    let s = compute(c);
    *STYLE.lock().unwrap() = Some(s);
}

/// Recalcula el tema con la configuración activa.
pub fn refresh() {
    apply(&config::current());
}

fn compute(c: &Config) -> Style {
    let dark = c.dark();
    let wa = opacity_alpha(c.get(Id::OpacidadVentanas));
    let ta = opacity_alpha(c.get(Id::OpacidadBarra));
    let a = from_ref(c.accent_colorref(), 255);
    let white = argb(255, 255, 255, 255);
    let black = argb(255, 0, 0, 0);
    let lum = (0.299 * r_of(a) as f32 + 0.587 * g_of(a) as f32 + 0.114 * b_of(a) as f32) / 255.0;

    let mut s: Style = unsafe { std::mem::zeroed() };
    s.dark = dark;
    s.blur = c.b(Id::Desenfoque);
    s.taskbar_opacity = c.get(Id::OpacidadBarra);
    s.window_opacity = c.get(Id::OpacidadVentanas);

    if dark {
        s.window_bg = argb(wa, 17, 20, 27);
        s.title_bg = argb(wa, 23, 27, 36);
        s.card_bg = rgbc(27, 31, 41);
        s.card_border = argb(40, 255, 255, 255);
        s.text = rgbc(232, 236, 244);
        s.text_dim = rgbc(148, 160, 178);
        s.text_faint = rgbc(105, 116, 136);
        s.danger = rgbc(248, 113, 113);
        s.success = rgbc(74, 222, 128);
        s.warning = rgbc(251, 191, 36);
        s.row_hover = rgbc(34, 39, 51);
        s.control_bg = rgbc(35, 40, 52);
        s.control_hover = rgbc(44, 50, 64);
        s.control_border = argb(45, 255, 255, 255);
        s.separator = argb(30, 255, 255, 255);
        s.nav_bg = argb(wa, 19, 22, 30);
        s.dialog_bg = argb(wa, 20, 23, 31);
        s.menu_bg = rgbc(30, 34, 44);
        s.menu_hover = rgbc(48, 54, 68);
        s.menu_border = argb(60, 255, 255, 255);
        s.taskbar_bg = argb(ta, 16, 18, 24);
        s.task_button = rgbc(28, 31, 40);
        s.task_button_hover = rgbc(38, 42, 54);
        s.task_button_active = rgbc(46, 51, 66);
        s.flyout_bg = argb(wa, 21, 24, 32);
        s.flyout_border = argb(65, 255, 255, 255);
        s.graph_bg = rgbc(22, 26, 35);
        s.grid_line = argb(18, 255, 255, 255);
        s.track = argb(60, 255, 255, 255);
        s.taskbar_tint = 0xE614161C;
        s.flyout_tint = 0xF214161C;
        s.accent = mix(a, white, 0.18, 255);
        s.accent_hover = mix(a, white, 0.12, 255);
        s.row_selected = with_alpha(a, 90);
    } else {
        s.window_bg = argb(wa, 247, 248, 250);
        s.title_bg = argb(wa, 237, 239, 243);
        s.card_bg = rgbc(255, 255, 255);
        s.card_border = argb(36, 0, 0, 0);
        s.text = rgbc(26, 30, 38);
        s.text_dim = rgbc(78, 88, 104);
        s.text_faint = rgbc(124, 132, 146);
        s.danger = rgbc(220, 38, 38);
        s.success = rgbc(22, 163, 74);
        s.warning = rgbc(202, 138, 4);
        s.row_hover = rgbc(230, 233, 239);
        s.control_bg = rgbc(255, 255, 255);
        s.control_hover = rgbc(234, 237, 242);
        s.control_border = argb(48, 0, 0, 0);
        s.separator = argb(26, 0, 0, 0);
        s.nav_bg = argb(wa, 240, 242, 245);
        s.dialog_bg = argb(wa, 249, 250, 251);
        s.menu_bg = rgbc(252, 252, 253);
        s.menu_hover = rgbc(229, 233, 240);
        s.menu_border = argb(45, 0, 0, 0);
        s.taskbar_bg = argb(ta, 238, 240, 244);
        s.task_button = rgbc(226, 229, 235);
        s.task_button_hover = rgbc(214, 219, 227);
        s.task_button_active = rgbc(250, 251, 252);
        s.flyout_bg = argb(wa, 248, 249, 251);
        s.flyout_border = argb(45, 0, 0, 0);
        s.graph_bg = rgbc(255, 255, 255);
        s.grid_line = argb(20, 0, 0, 0);
        s.track = argb(45, 0, 0, 0);
        s.taskbar_tint = 0xD8F4F2F0;
        s.flyout_tint = 0xEEFAF9F8;
        s.accent = mix(a, black, 0.12, 255);
        s.accent_hover = mix(a, black, 0.08, 255);
        s.row_selected = with_alpha(a, 60);
    }
    s.accent_solid = a;
    s.accent_hover = if dark { mix(a, white, 0.12, 255) } else { mix(a, black, 0.1, 255) };
    s.on_accent = if lum > 0.68 { rgbc(20, 20, 24) } else { white };
    s.focus_ring = with_alpha(s.accent, 150);
    s.close_hover = rgbc(196, 43, 43);

    // Tinte de los fondos hacia el acento (0-40 %).
    let t = c.get(Id::TinteFondo) as f32 / 100.0;
    if t > 0.0 {
        for f in [
            &mut s.window_bg, &mut s.title_bg, &mut s.card_bg, &mut s.nav_bg,
            &mut s.dialog_bg, &mut s.menu_bg, &mut s.taskbar_bg, &mut s.flyout_bg,
            &mut s.control_bg, &mut s.graph_bg, &mut s.task_button, &mut s.row_hover,
        ] {
            *f = tint(*f, a, t);
        }
        s.control_hover = tint(s.control_hover, a, t);
        s.menu_hover = tint(s.menu_hover, a, t);
        s.task_button_hover = tint(s.task_button_hover, a, t);
        s.task_button_active = tint(s.task_button_active, a, t);
    }

    // Contraste del texto secundario (acerca los tonos atenuados al principal).
    let k = c.get(Id::ContrasteTexto) as f32 / 100.0;
    if k > 0.0 {
        s.text_dim = tint(s.text_dim, s.text, k);
        s.text_faint = tint(s.text_faint, s.text, k * 0.8);
    }
    s
}
