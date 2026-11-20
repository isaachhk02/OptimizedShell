// OptimizedLogonUI.exe: la pantalla de bloqueo y de inicio de sesión, como
// proceso propio.
//
//   OptimizedLogonUI.exe --lock          bloquea la sesión actual
//   OptimizedLogonUI.exe --logon         pantalla de inicio de sesión
//   OptimizedLogonUI.exe --lock --demo   solo para verla, sin bloquear nada
//
// Sin argumentos se comporta como `--lock`, que es lo que se espera de un
// LogonUI lanzado a mano.
#![windows_subsystem = "windows"]

use optimized_shell::args::has_arg;

fn main() -> std::process::ExitCode {
    let logon = has_arg("--logon");
    let code = optimized_shell::logon::run(!logon, has_arg("--demo"));
    std::process::ExitCode::from(code as u8)
}
