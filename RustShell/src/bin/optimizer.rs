// OptimizedOptimizer: optimizador ligero de RAM para Windows 11.
#![windows_subsystem = "windows"]

fn main() -> std::process::ExitCode {
    // Si el SCM arranca el binario como servicio, no ejecutar el modo CLI.
    if optimized_shell::optimizer::try_run_as_service() {
        return std::process::ExitCode::SUCCESS;
    }
    std::process::ExitCode::from(optimized_shell::optimizer::run() as u8)
}