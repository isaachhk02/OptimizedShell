//! Reloj de la barra: HH:MM (predeterminado) y la fecha (DD/MM/YYYY), como
//! los dibuja OptimizedShell en `src/shell.rs` (`clock_text`).

use chrono::Local;

/// (reloj, fecha) con el formato de OptimizedShell.
pub fn now_text() -> (String, String) {
    let now = Local::now();
    (
        now.format("%H:%M").to_string(),
        now.format("%d/%m/%Y").to_string(),
    )
}
