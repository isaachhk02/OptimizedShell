// Lectura de argumentos de línea de comandos (equivale a HasArg del C++).

/// ¿Está presente el flag dado en la línea de comandos? (sin distinguir mayúsculas)
pub fn has_arg(flag: &str) -> bool {
    std::env::args().skip(1).any(|a| a.eq_ignore_ascii_case(flag))
}

/// Primer argumento que no empieza por `-`, si existe (p. ej. la ruta de --explorer
/// o la sección de --settings).
pub fn positional() -> Option<String> {
    std::env::args().skip(1).find(|a| !a.starts_with('-'))
}

/// Valor que sigue a un flag (p. ej. `--tab servicios` -> "servicios").
pub fn value(flag: &str) -> Option<String> {
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        if a.eq_ignore_ascii_case(flag) {
            return it.next();
        }
    }
    None
}
