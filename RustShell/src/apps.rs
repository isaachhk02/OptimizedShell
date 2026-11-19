// Lista de aplicaciones del menú Inicio.
//
// Se recorren los accesos directos (.lnk / .url) de los dos menús Inicio de
// Windows —el del usuario y el de todos los usuarios—, se deduplican por
// nombre y se ordenan alfabéticamente. Para la lista de «frecuentes» se guarda
// un contador de usos en %APPDATA%\OptimizedShell\recent.ini.
//
// Los iconos se piden al shell (SHGetFileInfoW) solo para las filas visibles y
// se cachean por ruta: enumerar cientos de accesos directos con icono sería
// justo el tipo de gasto que este proyecto evita.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, ShellExecuteW};
use windows_sys::Win32::UI::WindowsAndMessaging::{HICON, SW_SHOWNORMAL};

use crate::ui::{self, config};

/// Una entrada del menú: un acceso directo del menú Inicio de Windows.
#[derive(Clone)]
pub struct App {
    pub name: String,
    pub path: String,
    /// Carpeta del menú Inicio en la que está (para agrupar o informar).
    pub group: String,
    pub uses: u32,
}

fn start_menu_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        out.push(PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Ok(pd) = std::env::var("ProgramData") {
        out.push(PathBuf::from(pd).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    out
}

fn walk(dir: &Path, root: &Path, depth: u32, out: &mut Vec<App>) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let path = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            walk(&path, root, depth + 1, out);
            continue;
        }
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
        if ext != "lnk" && ext != "url" {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        let group = path
            .parent()
            .and_then(|p| p.strip_prefix(root).ok())
            .and_then(|p| p.to_str())
            .unwrap_or("")
            .to_string();
        out.push(App {
            name: name.to_string(),
            path: path.to_string_lossy().to_string(),
            group,
            uses: 0,
        });
    }
}

/// Todas las aplicaciones del menú Inicio, sin repetidos y ordenadas.
pub fn scan() -> Vec<App> {
    let uses = load_uses();
    let mut out: Vec<App> = Vec::new();
    for root in start_menu_dirs() {
        walk(&root, &root, 0, &mut out);
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    for a in out.iter_mut() {
        a.uses = uses.get(&a.path.to_lowercase()).copied().unwrap_or(0);
    }
    out
}

/// Índices de las aplicaciones más usadas (y, si faltan, las primeras por
/// orden alfabético) para la columna izquierda del menú.
pub fn frequent(apps: &[App], n: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..apps.len()).collect();
    idx.sort_by(|&a, &b| {
        apps[b].uses.cmp(&apps[a].uses).then_with(|| apps[a].name.to_lowercase().cmp(&apps[b].name.to_lowercase()))
    });
    idx.truncate(n);
    idx
}

// ------------------------------------------------------------ contador de usos
fn recent_file() -> PathBuf {
    config::config_dir().join("recent.ini")
}

pub fn load_uses() -> HashMap<String, u32> {
    let mut map = HashMap::new();
    let Ok(text) = std::fs::read_to_string(recent_file()) else { return map };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('[') {
            continue;
        }
        // `ruta=veces`: la ruta puede llevar '=', así que se parte por el último.
        if let Some(i) = line.rfind('=') {
            let (p, v) = line.split_at(i);
            if let Ok(n) = v[1..].trim().parse::<u32>() {
                map.insert(p.trim().to_lowercase(), n);
            }
        }
    }
    map
}

/// Suma un uso a una aplicación y lo guarda.
pub fn record_use(path: &str) {
    let mut uses = load_uses();
    let key = path.to_lowercase();
    let n = uses.entry(key).or_insert(0);
    *n += 1;
    let mut out = String::from("; Veces que se ha abierto cada aplicación desde el menú Inicio.\n[Usos]\n");
    let mut pairs: Vec<(&String, &u32)> = uses.iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(a.1));
    for (p, n) in pairs.iter().take(200) {
        out.push_str(&format!("{}={}\n", p, n));
    }
    let _ = std::fs::write(recent_file(), out);
}

/// Abre un acceso directo y anota el uso.
pub fn launch(path: &str) {
    record_use(path);
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            ui::wide("open").as_ptr(),
            ui::wide(path).as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

// ------------------------------------------------------------ iconos
thread_local! {
    static ICONS: RefCell<HashMap<String, isize>> = RefCell::new(HashMap::new());
}

/// Icono del shell para una ruta, cacheado. Devuelve null si no hay.
pub fn icon(path: &str) -> HICON {
    ICONS.with(|c| {
        let mut map = c.borrow_mut();
        if let Some(&h) = map.get(path) {
            return h as HICON;
        }
        // Tope de seguridad: no acumular miles de iconos en memoria.
        if map.len() > 512 {
            return std::ptr::null_mut();
        }
        let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
        let h = unsafe {
            SHGetFileInfoW(
                ui::wide(path).as_ptr(),
                0,
                &mut info,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON | SHGFI_LARGEICON,
            )
        };
        let hicon = if h != 0 { info.hIcon } else { std::ptr::null_mut() };
        map.insert(path.to_string(), hicon as isize);
        hicon
    })
}
