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

// ------------------------------------------------------------ anclados
// Los programas anclados se guardan por ruta en pinned.ini, en el orden en que
// el usuario los ancló: es lo que el menú Inicio enseña arriba del todo.
fn pinned_file() -> PathBuf {
    config::config_dir().join("pinned.ini")
}

/// Rutas ancladas, en orden.
pub fn pinned() -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(pinned_file()) else { return Vec::new() };
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with(';') && !l.starts_with('['))
        .map(|l| l.to_string())
        .collect()
}

fn save_pinned(list: &[String]) {
    let mut out = String::from("; Programas anclados al menú Inicio de OptimizedShell.\n[Anclados]\n");
    for p in list {
        out.push_str(p);
        out.push('\n');
    }
    let _ = std::fs::write(pinned_file(), out);
}

pub fn is_pinned(path: &str) -> bool {
    pinned().iter().any(|p| p.eq_ignore_ascii_case(path))
}

/// Ancla o desancla según el estado actual. Devuelve true si queda anclado.
pub fn toggle_pin(path: &str) -> bool {
    let mut list = pinned();
    match list.iter().position(|p| p.eq_ignore_ascii_case(path)) {
        Some(i) => {
            list.remove(i);
            save_pinned(&list);
            false
        }
        None => {
            list.push(path.to_string());
            // Tope razonable: el menú tampoco da para más.
            if list.len() > 20 {
                list.remove(0);
            }
            save_pinned(&list);
            true
        }
    }
}

/// Índices de `apps` que están anclados, en el orden del archivo. Las rutas
/// que ya no existen se ignoran (programa desinstalado).
pub fn pinned_indices(apps: &[App]) -> Vec<usize> {
    let pins = pinned();
    let mut out = Vec::new();
    for p in pins {
        if let Some(i) = apps.iter().position(|a| a.path.eq_ignore_ascii_case(&p)) {
            out.push(i);
        }
    }
    out
}

/// Abre un acceso directo pidiendo elevación ("Ejecutar como administrador").
pub fn launch_elevated(path: &str) {
    record_use(path);
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            ui::wide("runas").as_ptr(),
            ui::wide(path).as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Carpeta que contiene el acceso directo (para «Abrir ubicación»).
pub fn parent_dir(path: &str) -> Option<String> {
    std::path::Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
}

// ------------------------------------------- anclados a la barra de tareas
// Lista aparte de la del menú Inicio: se guarda en taskbar.ini, una ruta por
// línea y en el orden en que se dibujan los iconos de la barra. Valen
// ejecutables, accesos directos (.lnk/.url), carpetas y documentos: todo lo
// que el shell sepa abrir.
fn taskbar_file() -> PathBuf {
    config::config_dir().join("taskbar.ini")
}

/// Rutas ancladas a la barra de tareas, en orden.
pub fn taskbar_pins() -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(taskbar_file()) else { return default_taskbar_pins() };
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with(';') && !l.starts_with('['))
        .map(|l| l.to_string())
        .collect()
}

/// Primeros iconos de una instalación nueva: los que trae cualquier Windows.
fn default_taskbar_pins() -> Vec<String> {
    let mut out = Vec::new();
    let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    for p in [format!(r"{}\explorer.exe", sys), format!(r"{}\System32\notepad.exe", sys)] {
        if std::path::Path::new(&p).exists() {
            out.push(p);
        }
    }
    out
}

fn save_taskbar_pins(list: &[String]) {
    let mut out = String::from(
        "; Programas anclados a la barra de tareas de OptimizedShell.\n\
         ; Una ruta por línea, en el orden en que se dibujan.\n[Anclados]\n");
    for p in list {
        out.push_str(p);
        out.push('\n');
    }
    let _ = std::fs::create_dir_all(config::config_dir());
    let _ = std::fs::write(taskbar_file(), out);
}

pub fn taskbar_is_pinned(path: &str) -> bool {
    taskbar_pins().iter().any(|p| p.eq_ignore_ascii_case(path))
}

/// Ancla una ruta al final de la barra. Devuelve false si ya estaba.
pub fn taskbar_pin(path: &str) -> bool {
    let mut list = taskbar_pins();
    if list.iter().any(|p| p.eq_ignore_ascii_case(path)) {
        return false;
    }
    if list.len() >= 24 {
        return false;
    }
    list.push(path.to_string());
    save_taskbar_pins(&list);
    true
}

pub fn taskbar_unpin(path: &str) {
    let mut list = taskbar_pins();
    list.retain(|p| !p.eq_ignore_ascii_case(path));
    save_taskbar_pins(&list);
}

/// Ancla o desancla. Devuelve true si queda anclado.
pub fn taskbar_toggle_pin(path: &str) -> bool {
    if taskbar_is_pinned(path) {
        taskbar_unpin(path);
        false
    } else {
        taskbar_pin(path)
    }
}

/// Mueve un icono anclado una posición a la izquierda (-1) o a la derecha (1).
pub fn taskbar_move(index: usize, dir: i32) {
    let mut list = taskbar_pins();
    let target = index as i32 + dir;
    if index >= list.len() || target < 0 || target as usize >= list.len() {
        return;
    }
    list.swap(index, target as usize);
    save_taskbar_pins(&list);
}

/// Nombre que se enseña para una ruta anclada (sin extensión ni carpeta).
pub fn pin_label(path: &str) -> String {
    let p = std::path::Path::new(path);
    p.file_stem()
        .or_else(|| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// Nombre del ejecutable al que apunta una ruta anclada, en minúsculas, para
/// emparejar el icono anclado con las ventanas que ya estén abiertas.
pub fn pin_exe_name(path: &str) -> String {
    let lower = path.to_lowercase();
    if lower.ends_with(".lnk") || lower.ends_with(".url") {
        // Del acceso directo solo se puede deducir el nombre: basta para
        // emparejar "Firefox.lnk" con firefox.exe en la mayoría de casos.
        return format!("{}.exe", pin_label(path).to_lowercase());
    }
    std::path::Path::new(&lower)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or(lower)
}

/// Abre una ruta anclada con el shell (ejecutable, acceso directo o carpeta).
pub fn open_pin(path: &str) {
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
