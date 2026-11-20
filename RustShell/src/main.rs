// OptimizedShell: barra de tareas (modo por defecto) y router de modos, igual
// que la versión C++.
//
// Modos: (sin argumentos) shell | --taskmgr | --explorer [ruta] | --settings [seccion]
//        --logon | --lock | --demo | --install | --uninstall
//        --notify "Título|Mensaje" [--ok|--aviso|--error]
//        --update [--silent|--forzar]  (autoactualización desde GitHub)
//
// Las herramientas también son ejecutables propios —`OptimizedExplorer.exe`,
// `OptimizedTaskmgr.exe` y `OptimizedLogonUI.exe`—; este router sigue
// aceptando los flags de siempre y, si los binarios están instalados al lado,
// `shell::launch_own` lanza el que toque en vez de otra copia de este.
#![windows_subsystem = "windows"]

use optimized_shell::args::has_arg;
use optimized_shell::{explorer, logon, send_notification, settings, shell, taskmgr, update};

fn main() -> std::process::ExitCode {
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
