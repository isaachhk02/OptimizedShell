// OptimizedShell (port a Rust). Un solo binario multi-modo seleccionado por flag,
// igual que la versión C++. Sin consola.
//
// Modos: (sin argumentos) shell | --taskmgr | --explorer [ruta] | --settings [seccion]
//        --logon | --lock | --demo | --install | --uninstall
#![windows_subsystem = "windows"]

mod args;
mod ui;
mod apps;
mod perf;
mod startup;
mod taskmgr;
mod settings;
mod explorer;
mod logon;
mod shell;

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
    } else {
        // Modo shell (por defecto): barra de tareas.
        shell::run()
    };
    std::process::ExitCode::from(code as u8)
}
