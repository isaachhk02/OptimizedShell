// OptimizedExplorer.exe: el explorador de archivos, como proceso propio.
//
//   OptimizedExplorer.exe [ruta]
//
// Sin ruta abre la carpeta del usuario. Es el mismo explorador que `--explorer`
// del binario principal, pero en su propio proceso: cerrarlo o colgarlo no se
// lleva por delante la barra de tareas.
#![windows_subsystem = "windows"]

fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(optimized_shell::explorer::run() as u8)
}
