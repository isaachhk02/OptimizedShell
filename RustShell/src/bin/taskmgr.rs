// OptimizedTaskmgr.exe: el administrador de tareas, como proceso propio.
//
//   OptimizedTaskmgr.exe [procesos|rendimiento|inicio|servicios]
//
// La pestaña se elige con el argumento posicional, igual que en `--taskmgr`.
#![windows_subsystem = "windows"]

fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(optimized_shell::taskmgr::run() as u8)
}
