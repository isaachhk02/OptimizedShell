// OptimizedShell como biblioteca.
//
// Todos los módulos viven aquí para que cada herramienta pueda ser su propio
// ejecutable sin duplicar nada: `OptimizedShell.exe` (barra de tareas y router
// de modos), `OptimizedExplorer.exe`, `OptimizedTaskmgr.exe` y
// `OptimizedLogonUI.exe` son cuatro binarios muy finos que llaman a
// `explorer::run()`, `taskmgr::run()` y compañía.
//
// Así cada ventana es un proceso independiente —si el explorador se cuelga, la
// barra sigue viva— y el entorno se puede instalar y lanzar pieza a pieza.
#![allow(clippy::all)]

pub mod apps;
pub mod args;
pub mod desktop;
pub mod display;
pub mod explorer;
pub mod fileops;
pub mod logon;
pub mod notify;
pub mod optimizer;pub mod perf;
pub mod radio;
pub mod services;
pub mod settings;
pub mod shell;
pub mod startmenu;
pub mod startup;
pub mod sysinfo;
pub mod taskmgr;
pub mod tray;
pub mod traynotify;
pub mod ui;
pub mod update;
pub mod watcher;
pub mod winset;

/// `--notify "Título|Mensaje" [--ok|--aviso|--error] [--app Nombre]`: manda un
/// aviso a la barra de tareas que ya esté en ejecución. Sirve para que un
/// script, una tarea programada o cualquier otra parte del entorno avise al
/// usuario. Devuelve el código de salida del proceso.
pub fn send_notification() -> i32 {
    let text = args::positional().unwrap_or_default();
    let (title, body) = match text.split_once('|') {
        Some((t, b)) => (t.trim().to_string(), b.trim().to_string()),
        None => (text.trim().to_string(), String::new()),
    };
    if title.is_empty() && body.is_empty() {
        return 2;
    }
    let kind = if args::has_arg("--error") {
        notify::Kind::Error
    } else if args::has_arg("--aviso") {
        notify::Kind::Warning
    } else if args::has_arg("--ok") {
        notify::Kind::Success
    } else {
        notify::Kind::Info
    };
    let app = args::value("--app").unwrap_or_else(|| "OptimizedShell".to_string());
    if notify::post(&app, &title, &body, kind) {
        0
    } else {
        1
    }
}
