//! Fichas de estilo del shell Wayland, portadas de `ui::theme` (src/ui/theme.rs)
//! y los valores por defecto de `src/ui/config.rs`. El tema oscuro es el
//! predeterminado (`Modo = Oscuro`) y el acento por defecto es "Azul"
//! (`Acento = 0x3B82F6`); opacidades de la barra 60 % y de las ventanas 96 %.
//!
//! Aquí se ignora lo sin usar, igual que en el proyecto Windows.
#![allow(dead_code)]

use iced::Color;

/// Alto de la barra de tareas (`BarraAlto`).
pub const BAR_HEIGHT: f32 = 48.0;
/// Tamaño de los iconos de la barra (`TamanoIcono`).
pub const ICON_SIZE: f32 = 20.0;
/// Radio de las esquinas (`Radio`).
pub const RADIUS: f32 = 10.0;
/// Padding lateral de la barra.
pub const PAD: f32 = 12.0;
/// Espaciado entre elementos de la barra.
pub const GAP: f32 = 6.0;
/// Reloj en la barra (`clock_text`).
pub const CLOCK_SIZE: f32 = 18.0;
/// Fecha bajo el reloj, si se muestra.
pub const CLOCK_DATE_SIZE: f32 = 16.0;

fn rgb(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color::from_rgba8(r, g, b, a)
}

/// Tokens visuales del shell, mirror de `ui::theme::Style`.
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub background: Color,
    pub title_bg: Color,
    pub card_bg: Color,
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
    pub taskbar_bg: Color,
    pub task_button: Color,
    pub task_button_hover: Color,
    pub task_button_active: Color,
    pub menu_bg: Color,
    pub menu_hover: Color,
    pub menu_border: Color,
    pub flyout_bg: Color,
    pub graph_bg: Color,
    pub separator: Color,
    pub track: Color,
}

impl Tokens {
    /// Tema oscuro: el predeterminado de OptimizedShell.
    pub fn dark() -> Self {
        Self {
            background: rgb(17, 20, 27, 0.96),
            title_bg: rgb(23, 27, 36, 0.96),
            card_bg: rgb(27, 31, 41, 1.0),
            text: rgb(232, 236, 244, 1.0),
            text_dim: rgb(148, 160, 178, 1.0),
            text_faint: rgb(105, 116, 136, 1.0),
            accent: rgb(94, 153, 248, 1.0),
            accent_solid: rgb(59, 130, 246, 1.0),
            accent_hover: rgb(83, 145, 247, 1.0),
            on_accent: rgb(255, 255, 255, 1.0),
            row_selected: rgb(59, 130, 246, 0.353),
            focus_ring: rgb(94, 153, 248, 0.59),
            danger: rgb(248, 113, 113, 1.0),
            success: rgb(74, 222, 128, 1.0),
            warning: rgb(251, 191, 36, 1.0),
            close_hover: rgb(196, 43, 43, 1.0),
            taskbar_bg: rgb(16, 18, 24, 0.60),
            task_button: rgb(28, 31, 40, 1.0),
            task_button_hover: rgb(38, 42, 54, 1.0),
            task_button_active: rgb(46, 51, 66, 1.0),
            menu_bg: rgb(30, 34, 44, 1.0),
            menu_hover: rgb(48, 54, 68, 1.0),
            menu_border: rgb(255, 255, 255, 0.235),
            flyout_bg: rgb(21, 24, 32, 0.96),
            graph_bg: rgb(22, 26, 35, 1.0),
            separator: rgb(255, 255, 255, 0.118),
            track: rgb(255, 255, 255, 0.235),
        }
    }

    /// Tema claro (`Modo = Claro`).
    pub fn light() -> Self {
        Self {
            background: rgb(247, 248, 250, 0.96),
            title_bg: rgb(237, 239, 243, 0.96),
            card_bg: rgb(255, 255, 255, 1.0),
            text: rgb(26, 30, 38, 1.0),
            text_dim: rgb(78, 88, 104, 1.0),
            text_faint: rgb(124, 132, 146, 1.0),
            accent: rgb(52, 114, 216, 1.0),
            accent_solid: rgb(59, 130, 246, 1.0),
            accent_hover: rgb(53, 117, 221, 1.0),
            on_accent: rgb(255, 255, 255, 1.0),
            row_selected: rgb(59, 130, 246, 0.235),
            focus_ring: rgb(52, 114, 216, 0.59),
            danger: rgb(220, 38, 38, 1.0),
            success: rgb(22, 163, 74, 1.0),
            warning: rgb(202, 138, 4, 1.0),
            close_hover: rgb(196, 43, 43, 1.0),
            taskbar_bg: rgb(238, 240, 244, 0.60),
            task_button: rgb(226, 229, 235, 1.0),
            task_button_hover: rgb(214, 219, 227, 1.0),
            task_button_active: rgb(250, 251, 252, 1.0),
            menu_bg: rgb(252, 252, 253, 1.0),
            menu_hover: rgb(229, 233, 240, 1.0),
            menu_border: rgb(0, 0, 0, 0.18),
            flyout_bg: rgb(248, 249, 251, 0.96),
            graph_bg: rgb(255, 255, 255, 1.0),
            separator: rgb(0, 0, 0, 0.10),
            track: rgb(0, 0, 0, 0.18),
        }
    }
}
