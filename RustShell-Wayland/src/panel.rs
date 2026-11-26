//! Barra de tareas (port de src/shell.rs de OptimizedShell): botón Iniciо
//! en la izquierda, el menú flotante y el reloj.
//!
//! Antes en OptimizedShell también había botones de ventana abiertas; en
//! Wayland sin acceso al compositor esos botones están en la hoja de ruta.

use iced::widget::{button, container, row};
use iced::{alignment::Vertical, Border, Element, Length};

use crate::theme::{self, Tokens};
use crate::{start_menu, Message, Shell};

/// La barra: fila con Inicio, el menú flotante y el reloj, al fondo de color
/// (predeterminado: oscuro).
pub fn bar(shell: &Shell) -> Element<'_, Message> {
    let t = Tokens::dark();

    let mut items: Vec<Element<Message>> = vec![start_button()];
    if shell.menu_open {
        items.push(start_menu::block(shell));
    }
    items.push(clock());

    container(row(items))
        .width(Length::Fill)
        .height(Length::Fixed(theme::BAR_HEIGHT))
        .padding([0.0, theme::PAD])
        .align_y(Vertical::Center)
        .style(move |_| container::Style {
            background: Some(t.taskbar_bg.into()),
            border: Border::default(),
            ..Default::default()
        })
        .into()
}

fn start_button() -> Element<'static, Message> {
    let t = Tokens::dark();
    button("⊞")
        .height(Length::Fill)
        .padding([0.0, 16.0])
        .style(move |_, _| button::Style {
            border: iced::border::rounded(theme::RADIUS),
            background: Some(t.accent.into()),
            text_color: t.on_accent,
            ..Default::default()
        })
        .on_press(Message::StartPressed)
        .into()
}

fn clock() -> Element<'static, Message> {
    let t = Tokens::dark();
    let (time, date) = crate::clock::now_text();
    iced::widget::column![
        iced::widget::text(time).size(theme::CLOCK_SIZE),
        iced::widget::text(date)
            .size(theme::CLOCK_DATE_SIZE)
            .color(t.text_dim),
    ]
    .align_x(iced::alignment::Horizontal::Right)
    .into()
}
