// OptimizedShell (port a Rust). Un solo binario multi-modo seleccionado por flag,
// igual que la versión C++. Sin consola.
//
// Modos: (sin argumentos) shell | --taskmgr | --explorer [ruta] | --settings [seccion]
//        --logon | --lock | --demo | --install | --uninstall
//        --notify "Título|Mensaje" [--tipo info|ok|aviso|error]
//        --update [--silent|--forzar]  (autoactualización desde GitHub)
#![windows_subsystem = "windows"]

mod args;
mod ui;
mod apps;
mod perf;
mod sysinfo;
mod startup;
mod taskmgr;
mod settings;
mod explorer;
mod logon;
mod shell;
mod startmenu;
mod tray;
mod notify;
mod services;
mod desktop;
mod radio;
mod traynotify;
mod display;
mod fileops;
mod watcher;
mod update;

use args::has_arg;

fn main() -> std::process::ExitCode {
    // Router de modos (equivale a la cabecera de shell.cpp).
    let code = if has_arg("--logon") || has_arg("--lock") {
        logon::run(has_arg("--lock"), has_arg("--demo"))
    } else if has_arg("--taskmgr") {
        taskmgr::run()
    } else if has_arg("--settings") {
        settings::run()
    } else if has_arg("--explorer") {
        explorer::run()
    } else if has_arg("--install") || has_arg("--uninstall") {
        shell::install(has_arg("--install"))
    } else if has_arg("--update") {
        update::run()
    } else if has_arg("--notify") {
        send_notification()
    } else {
        // Modo shell (por defecto): barra de tareas.
        shell::run()
    };
    std::process::ExitCode::from(code as u8)
}

/// `--notify "Título|Mensaje"`: manda un aviso a la barra de tareas que ya
/// esté en ejecución. Sirve para que un script, una tarea programada o
/// cualquier otra parte de OptimizedShell avise al usuario.
fn send_notification() -> i32 {
    let text = args::positional().unwrap_or_default();
    let (title, body) = match text.split_once('|') {
        Some((t, b)) => (t.trim().to_string(), b.trim().to_string()),
        None => (text.trim().to_string(), String::new()),
    };
    if title.is_empty() && body.is_empty() {
        return 2;
    }
    let kind = if has_arg("--error") {
        notify::Kind::Error
    } else if has_arg("--aviso") {
        notify::Kind::Warning
    } else if has_arg("--ok") {
        notify::Kind::Success
    } else {
        notify::Kind::Info
    };
    let app = args::value("--app").unwrap_or_else(|| "OptimizedShell".to_string());
    if notify::post(&app, &title, &body, kind) { 0 } else { 1 }
}
