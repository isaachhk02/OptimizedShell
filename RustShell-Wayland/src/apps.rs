//! Escaneo de entradas `.desktop` del sistema, equivalente a los accesos
//! del menú Iniciо de OptimizedShell (`src/apps.rs` y `src/startmenu.rs`).

use std::fs;
use std::path::Path;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppEntry {
    pub name: String,
    pub exec: String,
    pub icon: Option<String>,
    pub categories: Vec<String>,
}

/// Directorios de aplicaciones (`XDG_DATA_DIRS` o las rutas estándar de Linux).
fn data_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var("XDG_DATA_DIRS")
        .map(|v| v.split(':').map(PathBuf::from).collect())
        .unwrap_or_else(|_| vec![PathBuf::from("/usr/local/share"), PathBuf::from("/usr/share")]);
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share"));
    }
    dirs
}

fn field(content: &str, key: &str) -> Option<String> {
    content
        .lines()
        .find_map(|l| l.trim_start().strip_prefix(&format!("{key}=")))
        .map(|v| v.trim().to_string())
}

/// Limpia un `Exec` de marcadores de argumento (%f, %u, %F...).
fn clean_exec(exec: &str) -> String {
    exec.split_whitespace()
        .filter(|t| !t.starts_with('%'))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Lanza una entrada de menú (equivalente a "Abrir" del acceso directo).
pub fn launch(entry: &AppEntry) {
    let exec = clean_exec(&entry.exec);
    if !exec.is_empty() {
        let _ = std::process::Command::new("sh").arg("-c").arg(exec).spawn();
    }
}

fn scan_dir(dir: &Path, out: &mut Vec<AppEntry>) {
    let Ok(rd) = fs::read_dir(dir.join("applications")) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let Some(name) = field(&content, "Name").filter(|n| !n.is_empty()) else {
            continue;
        };
        let Some(exec) = field(&content, "Exec").filter(|e| !e.is_empty()) else {
            continue;
        };
        let icon = field(&content, "Icon");
        let categories = field(&content, "Categories")
            .map(|c| c.split(';').filter(|s| !s.is_empty()).map(String::from).collect())
            .unwrap_or_default();
        out.push(AppEntry { name, exec, icon, categories });
    }
}

/// Lista de aplicaciones disponibles, por nombre, sin duplicados.
pub fn scan() -> Vec<AppEntry> {
    let mut out = Vec::new();
    for dir in data_dirs() {
        scan_dir(&dir, &mut out);
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.dedup_by(|a, b| a.name == b.name && a.exec == b.exec);
    out.truncate(256);
    out
}
