//! OptimizedShell para Wayland — shell de escritorio con el lenguaje visual
//! del proyecto Windows (panel de barra, menú Iniciо estilo Windows 7,
//! reloj). Construido sobre iced + iced_layershell.
//!
//! Ejecución: `cargo run --release`. Requiere una superficie de capa del
//! compositor (wlroots) o equivalente.

mod apps;
mod clock;
mod panel;
mod start_menu;
mod theme;

use iced::{Element, Subscription, Task};
use iced_layershell::application;
use iced_layershell::reexport::Anchor;
use iced_layershell::settings::{LayerShellSettings, Settings, StartMode};

use crate::apps::AppEntry;
use crate::panel::bar;

/// Estado del shell.
#[derive(Default)]
struct Shell {
    /// El menú Iniciо se despliega al pulsar el botón Iniciо.
    menu_open: bool,
    /// Texto de la búsqueda del menú.
    search: String,
    /// Entradas de aplicaciones escaneadas de los ficheros `.desktop`.
    apps: Vec<AppEntry>,
}

/// Mensajes del shell.
#[derive(Debug, Clone)]
enum Message {
    /// Reloj (cada segundo, por `subscription`).
    Tick,
    /// Pulsado el botón Iniciо: abre/cierra el menú.
    StartPressed,
    /// Lanzar una entrada del menú.
    MenuItem(AppEntry),
    /// Escribir en la búsqueda del menú.
    SearchChanged(String),
    /// Bloquear la sesión (port de `LogonUI --lock`).
    LockPressed,
    /// Reiniciar el equipo.
    RestartPressed,
    /// Apagar el equipo.
    ShutdownPressed,
}

/// Convierte el mensaje en acción de capa. Sin esta impl
/// `application()` no acepta el tipo, y todos los mensajes quedan de
/// "mensaje normal" del runtime (no se use al cambiar la capa).
impl TryInto<iced_layershell::actions::LayerShellCustomActionWithId> for Message {
    type Error = Self;

    fn try_into(self) -> Result<iced_layershell::actions::LayerShellCustomActionWithId, Self> {
        Err(self)
    }
}

fn namespace() -> String {
    String::from("OptimizedShell-Wayland")
}

fn update(shell: &mut Shell, message: Message) -> Task<Message> {
    match message {
        Message::StartPressed => shell.menu_open = !shell.menu_open,
        Message::MenuItem(entry) => {
            shell.menu_open = false;
            shell.search.clear();
            apps::launch(&entry);
        }
        Message::SearchChanged(text) => shell.search = text,
        Message::LockPressed => {
            shell.menu_open = false;
            let _ = std::process::Command::new("loginctl").arg("lock-session").spawn();
        }
        Message::RestartPressed => {
            shell.menu_open = false;
            let _ = std::process::Command::new("systemctl").arg("reboot").spawn();
        }
        Message::ShutdownPressed => {
            shell.menu_open = false;
            let _ = std::process::Command::new("systemctl").arg("poweroff").spawn();
        }
        Message::Tick => {}
    }
    Task::none()
}

fn view(shell: &Shell) -> Element<'_, Message> {
    bar(shell)
}

fn subscription(_: &Shell) -> Subscription<Message> {
    iced::time::every(iced::time::Duration::from_secs(1)).map(|_| Message::Tick)
}

fn main() -> iced_layershell::Result {
    application(
        || Shell { apps: apps::scan(), ..Default::default() },
        namespace,
        update,
        view,
    )
    .subscription(subscription)
    .settings(Settings {
        layer_settings: LayerShellSettings {
            anchor: Anchor::Bottom | Anchor::Left | Anchor::Right,
            exclusive_zone: theme::BAR_HEIGHT as i32,
            size: Some((0, theme::BAR_HEIGHT as u32)),
            start_mode: StartMode::Active,
            ..Default::default()
        },
        ..Default::default()
    })
    .run()
}
