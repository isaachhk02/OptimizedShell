// OptimizedOptimizer: optimizador ligero de RAM para Windows 11.
#![windows_subsystem = "windows"]

fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(optimized_shell::optimizer::run() as u8)
}