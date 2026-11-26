//! Menú Iniciо estilo Windows 7 (port de src/startmenu.rs de OptimizedShell):
//! campo de búsqueda, programas frecuentes, lista completa y columna derecha
//! con el usuario y los botones de apagado/reinicio.

use iced::widget::{button, column, container, row, space, text, TextInput};
use iced::{Element, Length};

use crate::theme::{self, Tokens};
use crate::{apps::AppEntry, Message, Shell};

/// Altura del menú (ajuste `MenuAlto` de OptimizedShell, predeterminado 560).
const MENU_HEIGHT: f32 = 560.0;
/// Programas frecuentes mostrados al abrir (`MenuFrecuentes`, predeterminado 5).
const FREQUENT: usize = 5;
/// Alto de cada ítem de la lista de programas.
const ITEM_HEIGHT: f32 = 28.0;
/// Alto de la fila de botones inferiores.
const FOOTER_HEIGHT: f32 = 40.0;

fn matches(entry: &AppEntry, search: &str) -> bool {
    let q = search.trim().to_lowercase();
    q.is_empty()
        || entry.name.to_lowercase().contains(&q)
        || entry.categories.iter().any(|c| c.to_lowercase().contains(&q))
}

fn app_button(entry: &AppEntry) -> Element<'_, Message> {
    button(entry.name.as_str())
        .width(Length::Fill)
        .height(Length::Fixed(ITEM_HEIGHT))
        .on_press(Message::MenuItem(entry.clone()))
        .into()
}

/// El menú flota dentro de la barra, debajo del botón Iniciо.
pub fn block(shell: &Shell) -> Element<'_, Message> {
    let t = Tokens::dark();

    // Lista filtrada por la búsqueda (si no hay, son todos).
    let apps: Vec<Element<Message>> = shell
        .apps
        .iter()
        .filter(|a| matches(a, &shell.search))
        .map(app_button)
        .collect();

    // Programas frecuentes, solo cuando no hay búsqueda.
    let frequent: Vec<Element<Message>> = if shell.search.trim().is_empty() {
        shell.apps.iter().take(FREQUENT).map(app_button).collect()
    } else {
        Vec::new()
    };

    let mut menu = column![
        space().height(10.0),
        TextInput::new("Buscar programas...", &shell.search)
            .on_input(Message::SearchChanged)
            .width(Length::Fill),
        space().height(10.0),
    ];

    if !frequent.is_empty() {
        let mut row = row![];
        for item in frequent {
            row = row.push(item);
        }
        menu = menu.push(row.spacing(theme::GAP));
        menu = menu.push(space().height(8.0));
    }

    menu = menu
        .push(column(apps).height(Length::Fixed(ITEM_HEIGHT * 8.0)).spacing(theme::GAP))
        .push(space().height(8.0));

    let mut footer = row![
        text("Usuario").size(14.0),
        space().width(Length::Fill),
        button("Bloquear").on_press(Message::LockPressed),
    ];
    footer = footer.push(button("Reiniciar").on_press(Message::RestartPressed));
    footer = footer.push(button("Apagar").on_press(Message::ShutdownPressed));

    menu = menu.push(footer.height(Length::Fixed(FOOTER_HEIGHT)).spacing(theme::GAP));

    container(menu)
        .height(Length::Fixed(MENU_HEIGHT))
        .width(Length::Fixed(300.0))
        .style(move |_| {
            iced::widget::container::Style {
                background: Some(t.menu_bg.into()),
                border: iced::border::rounded(4.0),
                ..Default::default()
            }
        })
        .into()
}
