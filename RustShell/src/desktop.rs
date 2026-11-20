// Escritorio: la ventana de fondo que dibuja el papel tapiz y los iconos.
//
// Cuando OptimizedShell sustituye al Explorador no hay nadie que pinte el
// escritorio: la pantalla se queda negra y los accesos directos del usuario
// desaparecen. Esta ventana ocupa toda la pantalla, se queda siempre al fondo
// (HWND_BOTTOM) y dibuja:
//
//   * la imagen de fondo del usuario con el ajuste elegido,
//   * los iconos esenciales (Este equipo, Papelera, carpeta del usuario, Red
//     y Panel de control), cada uno con su interruptor en Configuración,
//   * lo que haya en las carpetas Escritorio del usuario y la pública.
//
// Los iconos se colocan en una cuadrícula dentro del área de trabajo (sin
// tapar la barra de tareas), se ordenan según el ajuste «Ordenar los iconos
// por» y responden como los del Explorador: selección simple y múltiple,
// doble clic para abrir, menú contextual del shell, F2 para renombrar, Supr
// para eliminar, Ctrl+C/X/V, Ctrl+A y F5. Las carpetas se vigilan con
// ReadDirectoryChangesW, así que crear o borrar un archivo se ve al momento.
//
// Las carpetas se abren en el explorador de OptimizedShell (`--explorer`), no
// en el Explorador de Windows; los documentos y los accesos directos van al
// shell, que es quien sabe con qué aplicación abrirlos, y lo mismo las
// carpetas virtuales que no son del disco (Papelera, Red, Panel de control).
//
// La imagen sale de donde la deja Windows (HKCU\Control Panel\Desktop), así
// que cambiarla desde la Configuración de Windows o desde la galería de
// OptimizedShell se ve igual.
#![allow(non_snake_case, non_upper_case_globals)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;

use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, InvalidateRect};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F2, VK_F5,
    VK_HOME, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_UP,
};
use windows_sys::Win32::UI::Shell::{
    SHGetFileInfoW, ShellExecuteW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_PIDL,
    SHGFI_SYSICONINDEX,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::fileops::{self, Op};
use crate::ui::clipboard::{self, Effect};
use crate::ui::config::{self, Id};
use crate::ui::gdip::{Align, Gfx, Image};
use crate::ui::menu;
use crate::ui::{self, theme, window};
use crate::watcher::{self, Watcher};

pub const CLASS: &str = "OShellRustDesktop";

const DESKTOP_KEY: &str = "Control Panel\\Desktop";

thread_local! {
    /// Imagen cargada, junto a la ruta de la que salió (para no recargarla).
    static WALLPAPER: RefCell<Option<(String, Image)>> = RefCell::new(None);
    /// Iconos ya pedidos al shell, por clave (ruta o nombre de carpeta virtual).
    static ICONS: RefCell<HashMap<String, isize>> = RefCell::new(HashMap::new());
}

// ============================================================ papel tapiz
/// Ruta del papel tapiz actual según Windows.
pub fn wallpaper_path() -> String {
    reg_str(DESKTOP_KEY, "Wallpaper")
}

fn reg_str(sub: &str, name: &str) -> String {
    unsafe {
        let mut k: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, ui::wide(sub).as_ptr(), 0, KEY_QUERY_VALUE, &mut k) != 0 {
            return String::new();
        }
        let mut buf = [0u16; 520];
        let mut len = (buf.len() * 2) as u32;
        let mut kind = 0u32;
        let ok = RegQueryValueExW(k, ui::wide(name).as_ptr(), std::ptr::null_mut(), &mut kind,
                                  buf.as_mut_ptr() as *mut u8, &mut len) == 0;
        RegCloseKey(k);
        if !ok {
            return String::new();
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n])
    }
}

/// Cambia el papel tapiz del sistema. Windows lo guarda en el registro y
/// avisa al resto del escritorio; aquí además se recarga la imagen.
pub fn set_wallpaper(path: &str) -> bool {
    unsafe {
        let ok = SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            ui::wide(path).as_ptr() as *mut _,
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        ) != 0;
        if ok {
            WALLPAPER.with(|c| *c.borrow_mut() = None);
        }
        ok
    }
}

/// Carga (y cachea) la imagen de fondo.
fn with_wallpaper<F: FnOnce(Option<&Image>)>(f: F) {
    let path = wallpaper_path();
    WALLPAPER.with(|c| {
        let mut cell = c.borrow_mut();
        let stale = match cell.as_ref() {
            Some((p, _)) => *p != path,
            None => true,
        };
        if stale {
            *cell = if path.is_empty() {
                None
            } else {
                Image::load(&path).map(|img| (path.clone(), img))
            };
        }
        f(cell.as_ref().map(|(_, img)| img));
    });
}

// ============================================================ iconos
#[link(name = "shell32")]
extern "system" {
    fn SHParseDisplayName(name: *const u16, bc: *mut c_void, pidl: *mut *mut c_void,
                          attrs: u32, out_attrs: *mut u32) -> i32;
    fn SHGetImageList(which: i32, riid: *const GUID, out: *mut *mut c_void) -> i32;
    fn SHEmptyRecycleBinW(owner: HWND, root: *const u16, flags: u32) -> i32;
    fn SHQueryRecycleBinW(root: *const u16, info: *mut SHQUERYRBINFO) -> i32;
}

#[link(name = "comctl32")]
extern "system" {
    fn ImageList_GetIcon(himl: *mut c_void, i: i32, flags: u32) -> HICON;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SHQUERYRBINFO {
    cbSize: u32,
    i64Size: i64,
    i64NumItems: i64,
}

// IID_IImageList {46EB5926-582E-4017-9FDF-E8998DAA0950}
const IID_IMAGE_LIST: GUID = GUID::from_u128(0x46EB5926_582E_4017_9FDF_E8998DAA0950);
const SHIL_LARGE: i32 = 0; // 32 px
const SHIL_EXTRALARGE: i32 = 2; // 48 px
const SHIL_JUMBO: i32 = 4; // 256 px
const ILD_TRANSPARENT: u32 = 1;

/// Lista de imágenes del sistema con el tamaño más cercano (por encima) al
/// que se va a dibujar: escalar hacia abajo se ve mucho mejor que al revés.
fn image_list_for(size: i32) -> i32 {
    if size > 96 {
        SHIL_JUMBO
    } else if size > 32 {
        SHIL_EXTRALARGE
    } else {
        SHIL_LARGE
    }
}

/// Icono de un elemento del escritorio, cacheado por clave.
///
/// `target` puede ser una ruta del disco o un nombre de carpeta virtual
/// (`shell:RecycleBinFolder`): SHParseDisplayName entiende las dos cosas, y
/// así la Papelera trae su icono de verdad (lleno o vacío).
fn icon_of(key: &str, target: &str, size: i32) -> HICON {
    let cache_key = format!("{}|{}", image_list_for(size), key);
    ICONS.with(|c| {
        if let Some(&h) = c.borrow().get(&cache_key) {
            return h as HICON;
        }
        let hicon = load_icon(target, size);
        // Tope de seguridad: un escritorio con miles de archivos no debe
        // acumular miles de iconos.
        let mut map = c.borrow_mut();
        if map.len() < 1024 {
            map.insert(cache_key, hicon as isize);
        }
        hicon
    })
}

fn load_icon(target: &str, size: i32) -> HICON {
    fileops::init_com();
    unsafe {
        let mut pidl: *mut c_void = std::ptr::null_mut();
        if SHParseDisplayName(ui::wide(target).as_ptr(), std::ptr::null_mut(), &mut pidl, 0,
                              std::ptr::null_mut()) != 0 || pidl.is_null() {
            return std::ptr::null_mut();
        }
        let mut info: SHFILEINFOW = std::mem::zeroed();
        let ok = SHGetFileInfoW(pidl as *const u16, 0, &mut info,
                                std::mem::size_of::<SHFILEINFOW>() as u32,
                                SHGFI_PIDL | SHGFI_SYSICONINDEX) != 0;
        let mut hicon: HICON = std::ptr::null_mut();
        if ok {
            let mut himl: *mut c_void = std::ptr::null_mut();
            if SHGetImageList(image_list_for(size), &IID_IMAGE_LIST, &mut himl) == 0
                && !himl.is_null()
            {
                hicon = ImageList_GetIcon(himl, info.iIcon, ILD_TRANSPARENT);
            }
        }
        if hicon.is_null() {
            // Sin lista de imágenes: el icono grande de siempre (32 px).
            let mut info2: SHFILEINFOW = std::mem::zeroed();
            if SHGetFileInfoW(pidl as *const u16, 0, &mut info2,
                              std::mem::size_of::<SHFILEINFOW>() as u32,
                              SHGFI_PIDL | SHGFI_ICON | SHGFI_LARGEICON) != 0
            {
                hicon = info2.hIcon;
            }
        }
        CoTaskMemFree(pidl);
        hicon
    }
}

/// Un icono del escritorio.
#[derive(Clone)]
struct Icon {
    /// Nombre que se dibuja debajo.
    name: String,
    /// Lo que se abre al hacer doble clic (ruta o `shell:...`).
    target: String,
    /// Ruta en el disco. Vacía en los iconos del sistema.
    path: String,
    /// Clave de la caché de iconos.
    key: String,
    /// Icono esencial del sistema (no se puede renombrar ni eliminar).
    system: bool,
    is_dir: bool,
    size: u64,
    /// Fecha de modificación en segundos (para ordenar).
    stamp: u64,
    /// Extensión en minúsculas (para ordenar por tipo).
    ext: String,
}

impl Icon {
    fn system(name: &str, target: &str) -> Icon {
        Icon {
            name: name.to_string(),
            target: target.to_string(),
            path: String::new(),
            key: target.to_string(),
            system: true,
            is_dir: true,
            size: 0,
            stamp: 0,
            ext: String::new(),
        }
    }
}

fn user_profile() -> String {
    std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".to_string())
}

/// Las dos carpetas Escritorio: la del usuario y la de todos los usuarios.
fn desktop_dirs() -> Vec<String> {
    let c = config::current();
    let mut out = Vec::new();
    if !c.b(Id::EscritorioArchivos) {
        return out;
    }
    out.push(format!("{}\\Desktop", user_profile().trim_end_matches('\\')));
    if c.b(Id::EscritorioPublico) {
        let public = std::env::var("PUBLIC").unwrap_or_else(|_| "C:\\Users\\Public".to_string());
        out.push(format!("{}\\Desktop", public.trim_end_matches('\\')));
    }
    out.retain(|d| std::path::Path::new(d).is_dir());
    out
}

/// Reúne los iconos esenciales activados y el contenido de las carpetas.
fn collect_icons() -> Vec<Icon> {
    let c = config::current();
    let mut out: Vec<Icon> = Vec::new();
    if !c.b(Id::EscritorioIconos) {
        return out;
    }
    // Esenciales, en el orden de siempre del Escritorio de Windows.
    if c.b(Id::EscritorioEquipo) {
        out.push(Icon::system("Este equipo", "shell:MyComputerFolder"));
    }
    if c.b(Id::EscritorioUsuario) {
        let name = std::env::var("USERNAME").unwrap_or_else(|_| "Usuario".into());
        let mut it = Icon::system(&name, &user_profile());
        it.key = "shell:UsersFilesFolder".into();
        out.push(it);
    }
    if c.b(Id::EscritorioPapelera) {
        let mut it = Icon::system("Papelera de reciclaje", "shell:RecycleBinFolder");
        // La clave lleva el estado para que el icono cambie al vaciarla.
        it.key = format!("shell:RecycleBinFolder|{}", if recycle_bin_empty() { 0 } else { 1 });
        out.push(it);
    }
    if c.b(Id::EscritorioRed) {
        out.push(Icon::system("Red", "shell:NetworkPlacesFolder"));
    }
    if c.b(Id::EscritorioPanel) {
        out.push(Icon::system("Panel de control", "shell:ControlPanelFolder"));
    }

    // Archivos y accesos directos de las carpetas Escritorio.
    let show_hidden = c.b(Id::EscritorioOcultos);
    let mut files: Vec<Icon> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for dir in desktop_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            let attrs = file_attributes(&md);
            // Oculto o del sistema: solo si el usuario lo pide. desktop.ini
            // nunca: es la configuración de la carpeta, no un icono.
            let hidden = attrs & 0x2 != 0 || attrs & 0x4 != 0;
            if hidden && !show_hidden {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if name.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            // El escritorio público se mezcla con el del usuario: si el
            // nombre se repite, gana el del usuario (se vio antes).
            if !seen.insert(name.to_lowercase()) {
                continue;
            }
            let path = format!("{}\\{}", dir.trim_end_matches('\\'), name);
            let is_dir = md.is_dir();
            let p = std::path::Path::new(&name);
            // Los accesos directos se enseñan sin la extensión, como Windows.
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
            let label = if ext == "lnk" || ext == "url" {
                p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| name.clone())
            } else {
                name.clone()
            };
            files.push(Icon {
                name: label,
                target: path.clone(),
                key: path.clone(),
                path,
                system: false,
                is_dir,
                size: if is_dir { 0 } else { md.len() },
                stamp: modified_secs(&md),
                ext,
            });
        }
    }
    sort_icons(&mut files, c.get(Id::EscritorioOrden));
    out.append(&mut files);
    out
}

fn file_attributes(md: &std::fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    md.file_attributes()
}

fn modified_secs(md: &std::fs::Metadata) -> u64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Ordena los archivos: las carpetas primero y luego según el ajuste.
fn sort_icons(v: &mut [Icon], order: i32) {
    let name_of = |i: &Icon| i.name.to_lowercase();
    v.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| match order {
            1 => a.ext.cmp(&b.ext).then_with(|| name_of(a).cmp(&name_of(b))),
            2 => b.stamp.cmp(&a.stamp).then_with(|| name_of(a).cmp(&name_of(b))),
            3 => b.size.cmp(&a.size).then_with(|| name_of(a).cmp(&name_of(b))),
            _ => name_of(a).cmp(&name_of(b)),
        })
    });
}

/// ¿Está vacía la Papelera? (para elegir el icono y habilitar «Vaciar»)
fn recycle_bin_empty() -> bool {
    unsafe {
        let mut info: SHQUERYRBINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHQUERYRBINFO>() as u32;
        if SHQueryRecycleBinW(std::ptr::null(), &mut info) != 0 {
            return true;
        }
        info.i64NumItems == 0
    }
}

// ============================================================ estado
struct State {
    hwnd: HWND,
    icons: Vec<Icon>,
    /// Rectángulo de cada icono, en coordenadas de cliente.
    cells: Vec<RECT>,
    /// Índice resaltado por el ratón, o -1.
    hover: isize,
    /// Índice con el foco del teclado, o -1.
    sel: isize,
    /// Selección múltiple.
    marked: HashSet<usize>,
    /// Ancla de la selección con Mayús.
    anchor: isize,
    /// Renombrado en curso: (índice, nombre que se está escribiendo).
    editing: Option<(usize, String)>,
    /// Vigilantes de las carpetas Escritorio.
    watchers: Vec<Watcher>,
}

macro_rules! state {
    ($hwnd:expr, $msg:expr, $wp:expr, $lp:expr) => {{
        let p = GetWindowLongPtrW($hwnd, GWLP_USERDATA) as *mut State;
        if p.is_null() {
            return DefWindowProcW($hwnd, $msg, $wp, $lp);
        }
        &mut *p
    }};
}

fn icon_px() -> i32 {
    ui::s(config::current().get(Id::EscritorioTamanoIcono))
}

fn labels_on() -> bool {
    config::current().b(Id::EscritorioEtiquetas)
}

/// Tamaño de una celda de la cuadrícula: icono, hueco y dos líneas de rótulo.
fn cell_size() -> (i32, i32) {
    let isz = icon_px();
    let w = isz + ui::s(32);
    let h = isz + ui::s(10) + if labels_on() { ui::s(34) } else { ui::s(4) };
    (w.max(ui::s(60)), h)
}

/// Área de trabajo (sin la barra de tareas) en coordenadas de cliente: la
/// ventana cubre toda la pantalla virtual, así que hay que restar su origen.
fn work_area(hwnd: HWND) -> RECT {
    unsafe {
        let mut wa: RECT = std::mem::zeroed();
        if SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut _ as *mut c_void, 0) == 0 {
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rc);
            return rc;
        }
        let mut origin = POINT { x: 0, y: 0 };
        ClientToScreen(hwnd, &mut origin);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        // Intersección del área de trabajo con la ventana, ya en cliente.
        let left = (wa.left - origin.x).max(0);
        let top = (wa.top - origin.y).max(0);
        let right = (wa.right - origin.x).min(rc.right);
        let bottom = (wa.bottom - origin.y).min(rc.bottom);
        RECT { left, top, right: right.max(left), bottom: bottom.max(top) }
    }
}

/// Coloca los iconos en columnas, de arriba abajo y luego a la derecha.
fn layout(st: &mut State) {
    st.cells.clear();
    let wa = work_area(st.hwnd);
    let (cw, ch) = cell_size();
    let pad = ui::s(8);
    let rows = (((wa.bottom - wa.top - pad) / ch).max(1)) as usize;
    for i in 0..st.icons.len() {
        let col = (i / rows) as i32;
        let row = (i % rows) as i32;
        st.cells.push(RECT {
            left: wa.left + pad + col * cw,
            top: wa.top + pad + row * ch,
            right: wa.left + pad + col * cw + cw,
            bottom: wa.top + pad + row * ch + ch,
        });
    }
}

fn hit_test(st: &State, x: i32, y: i32) -> isize {
    for (i, r) in st.cells.iter().enumerate() {
        if x >= r.left && x < r.right && y >= r.top && y < r.bottom {
            return i as isize;
        }
    }
    -1
}

/// ¿Cuántos iconos caben en una columna? (para moverse con las flechas)
fn rows_per_col(st: &State) -> isize {
    let wa = work_area(st.hwnd);
    let (_, ch) = cell_size();
    (((wa.bottom - wa.top - ui::s(8)) / ch).max(1)) as isize
}

fn reload(st: &mut State) {
    let keep: Vec<String> = st
        .marked
        .iter()
        .filter_map(|&i| st.icons.get(i).map(|it| it.key.clone()))
        .collect();
    st.icons = collect_icons();
    layout(st);
    // La selección se mantiene por clave: tras un refresco los índices cambian.
    st.marked = st
        .icons
        .iter()
        .enumerate()
        .filter(|(_, it)| keep.iter().any(|k| *k == it.key))
        .map(|(i, _)| i)
        .collect();
    if st.sel >= st.icons.len() as isize {
        st.sel = st.icons.len() as isize - 1;
    }
    st.editing = None;
    start_watchers(st);
}

/// Vigila las carpetas Escritorio (si el ajuste está activo).
fn start_watchers(st: &mut State) {
    st.watchers.clear();
    if !config::current().b(Id::EscritorioAutoRefresco) {
        return;
    }
    for dir in desktop_dirs() {
        if let Some(w) = Watcher::start(&dir, st.hwnd) {
            st.watchers.push(w);
        }
    }
}

fn repaint(hwnd: HWND) {
    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
}

// ============================================================ dibujo
/// Recorta un rótulo a dos líneas como mucho, con puntos suspensivos.
fn wrap_label(g: &Gfx, text: &str, max_w: f32, pt: f32, lines: usize) -> Vec<String> {
    if g.text_width(text, pt, false) <= max_w {
        return vec![text.to_string()];
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut start = 0usize;
    while start < chars.len() && out.len() < lines {
        let last = out.len() + 1 == lines;
        let mut end = start;
        let mut taken = start;
        while end < chars.len() {
            end += 1;
            let slice: String = chars[start..end].iter().collect();
            let probe = if last && end < chars.len() { format!("{}…", slice) } else { slice };
            if g.text_width(&probe, pt, false) > max_w {
                break;
            }
            taken = end;
        }
        if taken == start {
            // Ni un carácter entra: se fuerza uno para no dar vueltas.
            taken = (start + 1).min(chars.len());
        }
        let mut piece: String = chars[start..taken].iter().collect();
        if last && taken < chars.len() {
            piece = format!("{}…", piece.trim_end());
        }
        out.push(piece);
        start = taken;
    }
    out
}

fn paint(hwnd: HWND, st: &mut State) {
    let c = config::current();
    let fit = c.get(Id::EscritorioAjuste);
    let bg = theme::from_colorref({
        // El ajuste guarda 0xRRGGBB; el helper espera COLORREF (0xBBGGRR).
        let v = c.get(Id::EscritorioColor) as u32;
        (v >> 16 & 0xFF) | (v & 0xFF00) | ((v & 0xFF) << 16)
    });
    let s = theme::style();
    let isz = icon_px();
    let show_labels = labels_on();
    let hover = st.hover;
    let sel = st.sel;
    let editing = st.editing.clone();
    let icons = st.icons.clone();
    let cells = st.cells.clone();
    let marked = st.marked.clone();

    window::paint_buffered(hwnd, |g: &Gfx, w, h| {
        g.clear(bg);
        g.fill_rect(0, 0, w, h, bg);
        with_wallpaper(|img| {
            let Some(img) = img else {
                // Sin imagen: color liso y una marca discreta.
                g.text_in("OptimizedShell", 0.0, (h / 2 - ui::s(16)) as f32, w as f32, ui::s(32) as f32,
                          16.0, true, theme::with_alpha(s.text, 40), Align::Center, Align::Center, false);
                return;
            };
            match fit {
                1 => g.draw_image_contain(img, 0, 0, w, h),
                2 => g.draw_image(img, 0, 0, w, h),
                3 => {
                    // Mosaico.
                    let iw = img.width as i32;
                    let ih = img.height as i32;
                    if iw > 0 && ih > 0 {
                        let mut y = 0;
                        while y < h {
                            let mut x = 0;
                            while x < w {
                                g.draw_image(img, x, y, iw, ih);
                                x += iw;
                            }
                            y += ih;
                        }
                    }
                }
                4 => {
                    let iw = img.width as i32;
                    let ih = img.height as i32;
                    g.draw_image(img, (w - iw) / 2, (h - ih) / 2, iw, ih);
                }
                _ => g.draw_image_cover(img, 0, 0, w, h),
            }
        });

        // ------------------------------------------------------- los iconos
        let radius = ui::sf(6.0);
        let label_pt = 9.0;
        let line_h = ui::sf(15.0);
        for (i, it) in icons.iter().enumerate() {
            let Some(r) = cells.get(i) else { continue };
            let cw = r.right - r.left;
            let is_marked = marked.contains(&i);
            let is_hover = hover == i as isize;

            // Fondo de selección / resalte.
            if is_marked || is_hover {
                let fill = if is_marked {
                    theme::with_alpha(s.accent_solid, if is_hover { 150 } else { 110 })
                } else {
                    theme::argb(60, 255, 255, 255)
                };
                g.rounded(r.left as f32 + ui::sf(2.0), r.top as f32 + ui::sf(2.0),
                          (cw - ui::s(4)) as f32, (r.bottom - r.top - ui::s(4)) as f32,
                          radius, Some(fill), None);
            }
            if sel == i as isize && !is_marked {
                g.rounded(r.left as f32 + ui::sf(2.0), r.top as f32 + ui::sf(2.0),
                          (cw - ui::s(4)) as f32, (r.bottom - r.top - ui::s(4)) as f32,
                          radius, None, Some((s.focus_ring, ui::sf(1.0))));
            }

            // El icono del shell; si no hay, un glifo según el tipo.
            let hicon = icon_of(&it.key, &it.target, isz);
            let ix = r.left + (cw - isz) / 2;
            let iy = r.top + ui::s(6);
            if hicon.is_null() {
                let glyph = if it.system { "\u{E7F4}" } else if it.is_dir { "\u{E8B7}" } else { "\u{E7C3}" };
                g.text_in(glyph, ix as f32, iy as f32, isz as f32, isz as f32,
                          (isz as f32) * 0.62, false, s.text, Align::Center, Align::Center, true);
            } else {
                g.draw_hicon(hicon, ix, iy, isz);
            }

            if !show_labels {
                continue;
            }

            // Rótulo: dos líneas, o una caja de edición si se está renombrando.
            let ly = (iy + isz + ui::s(4)) as f32;
            let max_w = (cw - ui::s(6)) as f32;
            if let Some((idx, name)) = editing.as_ref() {
                if *idx == i {
                    g.rounded(r.left as f32 + ui::sf(3.0), ly - ui::sf(2.0), max_w,
                              line_h + ui::sf(6.0), ui::sf(4.0), Some(s.card_bg), Some((s.accent_solid, ui::sf(1.0))));
                    let shown = {
                        // Se enseña el final del nombre mientras se escribe.
                        let mut t = format!("{}|", name);
                        while g.text_width(&t, label_pt, false) > max_w - ui::sf(8.0) && t.chars().count() > 2 {
                            let mut it2 = t.chars();
                            it2.next();
                            t = it2.collect();
                        }
                        t
                    };
                    g.text_in(&shown, r.left as f32 + ui::sf(6.0), ly, max_w - ui::sf(6.0),
                              line_h + ui::sf(4.0), label_pt, false, s.text,
                              Align::Near, Align::Center, false);
                    continue;
                }
            }

            let lines = wrap_label(g, &it.name, max_w - ui::sf(6.0), label_pt, 2);
            for (n, line) in lines.iter().enumerate() {
                let y = ly + n as f32 * line_h;
                if !is_marked {
                    // Sombra: el rótulo tiene que leerse sobre cualquier fondo.
                    g.text_in(line, r.left as f32 + ui::sf(3.0) + ui::sf(1.0), y + ui::sf(1.0),
                              max_w, line_h, label_pt, false, theme::argb(170, 0, 0, 0),
                              Align::Center, Align::Center, false);
                }
                let col = if is_marked { s.on_accent } else { theme::rgbc(255, 255, 255) };
                g.text_in(line, r.left as f32 + ui::sf(3.0), y, max_w, line_h,
                          label_pt, false, col, Align::Center, Align::Center, false);
            }
        }
    });
}

// ============================================================ acciones
/// Ruta que el explorador de OptimizedShell puede abrir, si el icono es una
/// carpeta navegable. `None` para lo que no es del disco (Papelera, Red,
/// Panel de control) y para los archivos, que necesitan su aplicación.
fn own_explorer_path(it: &Icon) -> Option<String> {
    if it.system {
        return match it.target.as_str() {
            // «Este equipo»: el explorador propio no tiene carpeta virtual de
            // equipo, pero su panel lateral ya lista todas las unidades, así
            // que se entra por la del sistema.
            "shell:MyComputerFolder" => Some(format!(
                "{}\\",
                std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).trim_end_matches('\\')
            )),
            // Papelera, Red y Panel de control no son carpetas del disco: de
            // esas se sigue encargando el shell de Windows.
            t if t.starts_with("shell:") => None,
            // La carpeta del usuario sí es una ruta de verdad.
            t => Some(t.to_string()),
        };
    }
    if it.is_dir && !it.path.is_empty() {
        return Some(it.path.clone());
    }
    None
}

/// Abre el explorador propio en una ruta (entre comillas: los nombres del
/// escritorio llevan espacios muy a menudo).
fn open_in_own_explorer(path: &str) {
    crate::shell::launch_own(&format!("--explorer \"{}\"", path.trim_end_matches('\\')));
}

/// Doble clic (o «Abrir»): las carpetas van al explorador de OptimizedShell y
/// el resto —documentos, accesos directos y las carpetas virtuales— al shell.
fn open_icon(st: &mut State, i: usize) {
    let Some(it) = st.icons.get(i) else { return };
    if let Some(path) = own_explorer_path(it) {
        open_in_own_explorer(&path);
        return;
    }
    let target = it.target.clone();
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), ui::wide("open").as_ptr(),
                      ui::wide(&target).as_ptr(), std::ptr::null(), std::ptr::null(),
                      SW_SHOWNORMAL);
    }
}

fn selected_paths(st: &State) -> Vec<String> {
    let mut out: Vec<String> = st
        .marked
        .iter()
        .filter_map(|&i| st.icons.get(i))
        .filter(|it| !it.system && !it.path.is_empty())
        .map(|it| it.path.clone())
        .collect();
    out.sort();
    out
}

/// Carpeta en la que se crean o pegan cosas: la del usuario, siempre.
fn user_desktop() -> String {
    format!("{}\\Desktop", user_profile().trim_end_matches('\\'))
}

fn delete_selection(st: &mut State, permanent: bool) {
    let paths = selected_paths(st);
    if paths.is_empty() {
        return;
    }
    if !fileops::confirm_delete(st.hwnd, &paths, permanent) {
        return;
    }
    let _ = fileops::delete(st.hwnd, &paths, permanent);
    reload(st);
    repaint(st.hwnd);
}

fn copy_selection(st: &mut State, cut: bool) {
    let paths = selected_paths(st);
    if paths.is_empty() {
        return;
    }
    clipboard::set_files(&paths, if cut { Effect::Move } else { Effect::Copy });
}

fn paste_here(st: &mut State) {
    let Some((paths, effect)) = clipboard::get_files() else { return };
    let dest = user_desktop();
    let kind = if effect == Effect::Move { Op::Move } else { Op::Copy };
    let same_dir = |p: &String| {
        std::path::Path::new(p)
            .parent()
            .map(|d| d.to_string_lossy().trim_end_matches('\\').eq_ignore_ascii_case(&dest))
            .unwrap_or(false)
    };
    let (here, elsewhere): (Vec<String>, Vec<String>) = paths.into_iter().partition(same_dir);
    if !elsewhere.is_empty() {
        let _ = fileops::transfer(st.hwnd, &elsewhere, &dest, kind);
    }
    if kind == Op::Copy {
        // Pegar una copia en la misma carpeta duplica con otro nombre.
        for p in &here {
            let name = std::path::Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let new_name = fileops::free_copy_name(&dest, &name);
            let _ = fileops::copy_as(st.hwnd, p, &dest, &new_name);
        }
    }
    reload(st);
    repaint(st.hwnd);
}

fn begin_rename(st: &mut State) {
    if st.sel < 0 {
        return;
    }
    let i = st.sel as usize;
    let Some(it) = st.icons.get(i) else { return };
    if it.system || it.path.is_empty() {
        return;
    }
    // Se edita el nombre del archivo, con extensión: es lo que se renombra.
    let name = std::path::Path::new(&it.path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| it.name.clone());
    st.editing = Some((i, name));
    repaint(st.hwnd);
}

fn commit_rename(st: &mut State) {
    let Some((i, name)) = st.editing.take() else { return };
    let Some(it) = st.icons.get(i) else { return };
    let path = it.path.clone();
    let old = std::path::Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if !name.trim().is_empty() && name != old {
        let _ = fileops::rename(st.hwnd, &path, name.trim());
    }
    reload(st);
    repaint(st.hwnd);
}

fn create_folder(st: &mut State) {
    let _ = fileops::new_folder(st.hwnd, &user_desktop(), "Nueva carpeta");
    reload(st);
    repaint(st.hwnd);
}

/// Mueve el foco con las flechas (las columnas van de arriba abajo).
fn move_focus(st: &mut State, dx: isize, dy: isize, shift: bool) {
    let n = st.icons.len() as isize;
    if n == 0 {
        return;
    }
    let rows = rows_per_col(st);
    let cur = if st.sel < 0 { 0 } else { st.sel };
    let mut next = cur + dy + dx * rows;
    if next < 0 {
        next = 0;
    }
    if next >= n {
        next = n - 1;
    }
    st.sel = next;
    if shift {
        let a = st.anchor.max(0).min(n - 1);
        st.marked = (a.min(next)..=a.max(next)).map(|i| i as usize).collect();
    } else {
        st.marked.clear();
        st.marked.insert(next as usize);
        st.anchor = next;
    }
    repaint(st.hwnd);
}

// ============================================================ menús
/// Menú del botón derecho sobre el escritorio vacío.
fn context_menu(st: &mut State, hwnd: HWND, x: i32, y: i32) {
    let c = config::current();
    let items = vec![
        menu::Item::new(1, "Actualizar").glyph("\u{E72C}").hint("F5"),
        menu::Item::new(8, "Carpeta nueva").glyph("\u{E8F4}").hint("Ctrl+N"),
        menu::Item::new(9, "Pegar").glyph("\u{E77F}").hint("Ctrl+V").enabled(clipboard::has_files()),
        menu::Item::sep(),
        menu::Item::new(10, "Mostrar los iconos del escritorio")
            .glyph("\u{ECAA}").checked(c.b(Id::EscritorioIconos)),
        menu::Item::new(11, "Iconos grandes").glyph("\u{E8EA}")
            .checked(c.get(Id::EscritorioTamanoIcono) >= 64),
        menu::Item::header("Ordenar por"),
        menu::Item::new(20, "Nombre").checked(c.get(Id::EscritorioOrden) == 0),
        menu::Item::new(21, "Tipo").checked(c.get(Id::EscritorioOrden) == 1),
        menu::Item::new(22, "Fecha").checked(c.get(Id::EscritorioOrden) == 2),
        menu::Item::new(23, "Tamaño").checked(c.get(Id::EscritorioOrden) == 3),
        menu::Item::sep(),
        menu::Item::new(2, "Explorador de archivos").glyph("\u{EC50}"),
        menu::Item::new(3, "Administrador de tareas").glyph("\u{E9D9}"),
        menu::Item::new(4, "Ejecutar...").glyph("\u{E756}"),
        menu::Item::sep(),
        menu::Item::new(5, "Personalizar el fondo").glyph("\u{E91B}"),
        menu::Item::new(12, "Iconos del escritorio").glyph("\u{E713}"),
        menu::Item::new(6, "Configuración").glyph("\u{E713}"),
    ];
    match menu::popup(hwnd, x, y, items) {
        1 => {
            WALLPAPER.with(|c| *c.borrow_mut() = None);
            ICONS.with(|c| c.borrow_mut().clear());
            reload(st);
            repaint(hwnd);
        }
        2 => open_in_own_explorer(&user_profile()),
        3 => crate::shell::launch_own("--taskmgr"),
        4 => unsafe {
            ShellExecuteW(std::ptr::null_mut(), ui::wide("open").as_ptr(),
                          ui::wide("explorer.exe").as_ptr(),
                          ui::wide("shell:::{2559a1f3-21d7-11d4-bdaf-00c04f60b9f0}").as_ptr(),
                          std::ptr::null(), SW_SHOWNORMAL);
        },
        5 => crate::shell::launch_own("--settings fondo"),
        6 => crate::shell::launch_own("--settings"),
        8 => create_folder(st),
        9 => paste_here(st),
        10 => set_and_broadcast(st, Id::EscritorioIconos, i32::from(!c.b(Id::EscritorioIconos))),
        11 => {
            let big = c.get(Id::EscritorioTamanoIcono) >= 64;
            set_and_broadcast(st, Id::EscritorioTamanoIcono, if big { 48 } else { 72 });
        }
        12 => crate::shell::launch_own("--settings pantalla"),
        v @ 20..=23 => set_and_broadcast(st, Id::EscritorioOrden, v - 20),
        _ => {}
    }
}

/// Cambia un ajuste desde el menú del escritorio y avisa al resto del shell.
fn set_and_broadcast(st: &mut State, id: Id, value: i32) {
    let mut c = config::current();
    c.set(id, value);
    ui::apply_config(&c, true);
    ui::broadcast_config_changed();
    reload(st);
    repaint(st.hwnd);
}

/// Menú del botón derecho sobre un icono.
fn icon_menu(st: &mut State, hwnd: HWND, i: usize, x: i32, y: i32) {
    let Some(it) = st.icons.get(i).cloned() else { return };
    let paths = selected_paths(st);
    let multi = paths.len() > 1;

    if it.system {
        let recycle = it.target.eq_ignore_ascii_case("shell:RecycleBinFolder");
        let mut items = vec![
            menu::Item::new(1, "Abrir").glyph("\u{E8E5}"),
        ];
        if recycle {
            items.push(menu::Item::new(2, "Vaciar la Papelera de reciclaje")
                .glyph("\u{E74D}").enabled(!recycle_bin_empty()).danger());
        }
        items.push(menu::Item::sep());
        items.push(menu::Item::new(3, "Propiedades").glyph("\u{E946}"));
        items.push(menu::Item::new(4, "Quitar este icono").glyph("\u{E711}"));
        match menu::popup(hwnd, x, y, items) {
            1 => open_icon(st, i),
            2 => {
                unsafe { SHEmptyRecycleBinW(hwnd, std::ptr::null(), 0) };
                ICONS.with(|c| c.borrow_mut().clear());
                reload(st);
                repaint(hwnd);
            }
            3 => {
                if it.path.is_empty() {
                    open_icon(st, i);
                } else {
                    fileops::show_properties(hwnd, &it.path);
                }
            }
            4 => {
                let id = match it.target.as_str() {
                    "shell:MyComputerFolder" => Id::EscritorioEquipo,
                    "shell:RecycleBinFolder" => Id::EscritorioPapelera,
                    "shell:NetworkPlacesFolder" => Id::EscritorioRed,
                    "shell:ControlPanelFolder" => Id::EscritorioPanel,
                    _ => Id::EscritorioUsuario,
                };
                set_and_broadcast(st, id, 0);
            }
            _ => {}
        }
        return;
    }

    let items = vec![
        menu::Item::new(1, "Abrir").glyph("\u{E8E5}").hint("Entrar"),
        menu::Item::new(2, "Abrir la ubicación").glyph("\u{EC50}"),
        menu::Item::sep(),
        menu::Item::new(3, "Cortar").glyph("\u{E8C6}").hint("Ctrl+X"),
        menu::Item::new(4, "Copiar").glyph("\u{E8C8}").hint("Ctrl+C"),
        menu::Item::new(5, "Copiar la ruta").glyph("\u{E8C8}"),
        menu::Item::sep(),
        menu::Item::new(6, "Cambiar el nombre").glyph("\u{E8AC}").hint("F2").enabled(!multi),
        menu::Item::new(7, "Eliminar").glyph("\u{E74D}").hint("Supr").danger(),
        menu::Item::sep(),
        menu::Item::new(10, "Más opciones de Windows").glyph("\u{E712}"),
        menu::Item::new(11, "Propiedades").glyph("\u{E946}"),
    ];
    match menu::popup(hwnd, x, y, items) {
        1 => open_icon(st, i),
        2 => {
            if let Some(dir) = std::path::Path::new(&it.path).parent() {
                open_in_own_explorer(&dir.to_string_lossy());
            }
        }
        3 => copy_selection(st, true),
        4 => copy_selection(st, false),
        5 => {
            ui::set_clipboard_text(&paths.join("\r\n"));
        }
        6 => begin_rename(st),
        7 => delete_selection(st, false),
        10 => {
            fileops::shell_context_menu(hwnd, &paths, x, y);
            reload(st);
            repaint(hwnd);
        }
        11 => {
            fileops::show_properties(hwnd, &it.path);
        }
        _ => {}
    }
}

// ============================================================ ventana
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == ui::config_changed_msg() {
        ui::reload_config();
        let st = state!(hwnd, msg, wp, lp);
        apply_placement(hwnd);
        // El ajuste «Dibujar el escritorio» se aplica al momento.
        let on = config::current().b(Id::EscritorioActivo);
        ShowWindow(hwnd, if on { SW_SHOWNOACTIVATE } else { SW_HIDE });
        ICONS.with(|c| c.borrow_mut().clear());
        reload(st);
        InvalidateRect(hwnd, std::ptr::null(), 0);
        return 0;
    }
    if msg == watcher::WM_DIR_CHANGED {
        let st = state!(hwnd, msg, wp, lp);
        reload(st);
        InvalidateRect(hwnd, std::ptr::null(), 0);
        return 0;
    }
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let st = state!(hwnd, msg, wp, lp);
            if st.cells.len() != st.icons.len() {
                layout(st);
            }
            paint(hwnd, st);
            0
        }
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
            // Cambió la resolución o los ajustes del sistema (puede ser el fondo).
            WALLPAPER.with(|c| *c.borrow_mut() = None);
            apply_placement(hwnd);
            let st = state!(hwnd, msg, wp, lp);
            layout(st);
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_MOUSEMOVE => {
            let st = state!(hwnd, msg, wp, lp);
            let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
            let h = hit_test(st, x, y);
            if h != st.hover {
                st.hover = h;
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_LBUTTONDOWN => {
            let st = state!(hwnd, msg, wp, lp);
            SetFocus(hwnd);
            let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
            let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
            let shift = GetKeyState(VK_SHIFT as i32) < 0;
            if st.editing.is_some() {
                commit_rename(st);
            }
            let h = hit_test(st, x, y);
            if h < 0 {
                st.marked.clear();
                st.sel = -1;
            } else {
                let i = h as usize;
                if ctrl {
                    if !st.marked.remove(&i) {
                        st.marked.insert(i);
                    }
                } else if shift && st.anchor >= 0 {
                    let a = st.anchor as usize;
                    st.marked = (a.min(i)..=a.max(i)).collect();
                } else {
                    st.marked.clear();
                    st.marked.insert(i);
                    st.anchor = h;
                }
                st.sel = h;
            }
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_LBUTTONDBLCLK => {
            let st = state!(hwnd, msg, wp, lp);
            let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
            let h = hit_test(st, x, y);
            if h >= 0 {
                open_icon(st, h as usize);
            }
            0
        }
        WM_RBUTTONUP => {
            let st = state!(hwnd, msg, wp, lp);
            SetFocus(hwnd);
            let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
            let h = hit_test(st, x, y);
            let mut pt = POINT { x, y };
            ClientToScreen(hwnd, &mut pt);
            if h >= 0 {
                let i = h as usize;
                // Clic derecho sobre algo no seleccionado: pasa a serlo.
                if !st.marked.contains(&i) {
                    st.marked.clear();
                    st.marked.insert(i);
                    st.anchor = h;
                }
                st.sel = h;
                InvalidateRect(hwnd, std::ptr::null(), 0);
                icon_menu(st, hwnd, i, pt.x, pt.y);
            } else {
                context_menu(st, hwnd, pt.x, pt.y);
            }
            0
        }
        WM_CHAR => {
            let st = state!(hwnd, msg, wp, lp);
            if let Some((_, name)) = st.editing.as_mut() {
                let ch = wp as u32;
                if ch == 8 {
                    name.pop();
                } else if ch >= 0x20 {
                    if let Some(c) = char::from_u32(ch) {
                        // Caracteres que Windows no admite en un nombre.
                        if !r#"\/:*?"<>|"#.contains(c) {
                            name.push(c);
                        }
                    }
                }
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_KEYDOWN => {
            let st = state!(hwnd, msg, wp, lp);
            let vk = wp as u32;
            let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
            let shift = GetKeyState(VK_SHIFT as i32) < 0;

            if st.editing.is_some() {
                if vk == VK_RETURN as u32 {
                    commit_rename(st);
                } else if vk == VK_ESCAPE as u32 {
                    st.editing = None;
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                return 0;
            }
            if ctrl {
                match vk as u8 as char {
                    'C' => copy_selection(st, false),
                    'X' => copy_selection(st, true),
                    'V' => paste_here(st),
                    'N' => create_folder(st),
                    'A' => {
                        st.marked = (0..st.icons.len()).collect();
                        if st.sel < 0 && !st.icons.is_empty() {
                            st.sel = 0;
                        }
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                    _ => {}
                }
                return 0;
            }
            match vk {
                v if v == VK_F5 as u32 => {
                    WALLPAPER.with(|c| *c.borrow_mut() = None);
                    ICONS.with(|c| c.borrow_mut().clear());
                    reload(st);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                v if v == VK_F2 as u32 => begin_rename(st),
                v if v == VK_DELETE as u32 => delete_selection(st, shift),
                v if v == VK_RETURN as u32 => {
                    if st.sel >= 0 {
                        open_icon(st, st.sel as usize);
                    }
                }
                v if v == VK_ESCAPE as u32 => {
                    st.marked.clear();
                    st.sel = -1;
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                v if v == VK_UP as u32 => move_focus(st, 0, -1, shift),
                v if v == VK_DOWN as u32 => move_focus(st, 0, 1, shift),
                v if v == VK_LEFT as u32 => move_focus(st, -1, 0, shift),
                v if v == VK_RIGHT as u32 => move_focus(st, 1, 0, shift),
                v if v == VK_HOME as u32 => {
                    st.sel = 0;
                    move_focus(st, 0, 0, shift);
                }
                v if v == VK_END as u32 => {
                    st.sel = st.icons.len() as isize - 1;
                    move_focus(st, 0, 0, shift);
                }
                _ => {}
            }
            0
        }
        // Nunca se pone delante: es el fondo.
        WM_WINDOWPOSCHANGING => {
            let p = &mut *(lp as *mut WINDOWPOS);
            p.hwndInsertAfter = HWND_BOTTOM;
            p.flags &= !SWP_NOZORDER;
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_DESTROY => {
            WALLPAPER.with(|c| *c.borrow_mut() = None);
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
            if !p.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(p));
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Coloca la ventana cubriendo toda la pantalla virtual, al fondo del todo.
fn apply_placement(hwnd: HWND) {
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        SetWindowPos(hwnd, HWND_BOTTOM, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
}

/// ¿Está el Explorador de Windows haciendo de shell? Si lo está, él ya dibuja
/// el escritorio (y sus iconos), y taparlo sería un estorbo.
fn explorer_is_shell() -> bool {
    unsafe {
        !FindWindowW(ui::wide("Progman").as_ptr(), std::ptr::null()).is_null()
    }
}

/// Crea la ventana del escritorio. Devuelve null si está desactivada o si el
/// Explorador de Windows ya está dibujando el escritorio.
pub fn create(hinst: HINSTANCE) -> HWND {
    if !config::current().b(Id::EscritorioActivo) || explorer_is_shell() {
        return std::ptr::null_mut();
    }
    unsafe {
        let name = ui::wide(CLASS);
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: name.as_ptr(),
        };
        RegisterClassW(&wc);
        // Sin WS_EX_NOACTIVATE: el escritorio recibe el foco al pulsarlo (hace
        // falta para el teclado). WS_EX_TOOLWINDOW lo deja fuera de Alt+Tab, y
        // WM_WINDOWPOSCHANGING lo mantiene siempre al fondo.
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            name.as_ptr(),
            ui::wide("Escritorio").as_ptr(),
            WS_POPUP,
            0, 0, 10, 10,
            std::ptr::null_mut(), std::ptr::null_mut(), hinst, std::ptr::null(),
        );
        if hwnd.is_null() {
            return hwnd;
        }
        let st = Box::new(State {
            hwnd,
            icons: Vec::new(),
            cells: Vec::new(),
            hover: -1,
            sel: -1,
            marked: HashSet::new(),
            anchor: -1,
            editing: None,
            watchers: Vec::new(),
        });
        let raw = Box::into_raw(st);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
        apply_placement(hwnd);
        reload(&mut *raw);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        hwnd
    }
}

/// Minimiza todas las ventanas para enseñar el escritorio (Win+D).
pub fn show_desktop() {
    unsafe {
        // El shell real usa el mensaje 419 de la bandeja; aquí se hace a mano:
        // minimizar todas las ventanas de nivel superior visibles.
        EnumWindows(Some(minimize_proc), 0);
    }
}

unsafe extern "system" fn minimize_proc(hwnd: HWND, _lp: LPARAM) -> i32 {
    if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
        return 1;
    }
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex & WS_EX_TOOLWINDOW != 0 {
        return 1;
    }
    if GetWindow(hwnd, GW_OWNER) != std::ptr::null_mut() {
        return 1;
    }
    if GetWindowTextLengthW(hwnd) == 0 {
        return 1;
    }
    ShowWindow(hwnd, SW_MINIMIZE);
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El orden de los iconos: carpetas primero y luego el criterio elegido.
    #[test]
    fn orden_de_iconos() {
        let mk = |name: &str, dir: bool, ext: &str, stamp: u64, size: u64| Icon {
            name: name.into(),
            target: name.into(),
            path: name.into(),
            key: name.into(),
            system: false,
            is_dir: dir,
            size,
            stamp,
            ext: ext.into(),
        };
        let base = vec![
            mk("b.txt", false, "txt", 10, 100),
            mk("Carpeta", true, "", 5, 0),
            mk("a.zip", false, "zip", 20, 50),
        ];

        let mut v = base.clone();
        sort_icons(&mut v, 0);
        assert_eq!(v.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
                   vec!["Carpeta", "a.zip", "b.txt"]);

        let mut v = base.clone();
        sort_icons(&mut v, 1); // tipo
        assert_eq!(v[1].ext, "txt");

        let mut v = base.clone();
        sort_icons(&mut v, 2); // fecha, la más reciente primero
        assert_eq!(v[1].name, "a.zip");

        let mut v = base.clone();
        sort_icons(&mut v, 3); // tamaño, el mayor primero
        assert_eq!(v[1].name, "b.txt");
    }

    /// `desktop.ini` y los nombres repetidos no deben colarse dos veces.
    #[test]
    fn carpetas_del_escritorio_existen() {
        for d in desktop_dirs() {
            assert!(std::path::Path::new(&d).is_dir(), "{} no es una carpeta", d);
        }
    }
}
