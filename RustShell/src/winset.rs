// Ajustes del sistema, nativos: el reemplazo de `ms-settings:`.
//
// Todo lo que la Configuración de OptimizedShell delegaba antes en la
// Configuración de Windows (`ms-settings:powersleep`, `:sound`, `:bluetooth`,
// `:printers`, `:mousetouchpad`, `:keyboard`, `:network-*`, `:appsfeatures`,
// `:defaultapps`, `:easeofaccess`, `:lockscreen`...) se lee y se escribe aquí
// con la API de Windows, sin abrir ninguna ventana ajena.
//
// Criterios, los mismos que en `perf` y `sysinfo`:
// - FFI propia y mínima; COM a mano por vtable, sin capa generada.
// - Las bibliotecas que pueden faltar (wlanapi, bthprops, powrprof) se cargan
//   con LoadLibrary: si no están, la lista sale vacía con su explicación.
// - Nada se consulta en cada repintado: la página pide `entries()` cuando se
//   abre o después de actuar, y guarda el resultado.
//
// Toda la interfaz se reduce a dos tipos: una lista de `Entry` (icono, título,
// detalle y botones) por cada `ListKind`, y `act()`, que ejecuta el botón
// pulsado y devuelve el mensaje que se enseña debajo.
#![allow(dead_code)]
#![allow(non_snake_case, non_upper_case_globals)]

use std::ffi::c_void;

use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE, HWND};
use windows_sys::Win32::System::Com::{
    CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, CLSCTX_INPROC_SERVER,
};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetDoubleClickTime, SetDoubleClickTime};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SW_SHOWNORMAL,
};

use crate::ui::{self, wide};

// ============================================================ modelo
/// Botón de una fila.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Convertir en predeterminado / activar.
    Usar,
    Menos,
    Mas,
    Alternar,
    Quitar,
    Abrir,
    Cambiar,
    Desinstalar,
    Subir,
    Bajar,
    Anclar,
    Examinar,
    Buscar,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Usar => "Usar",
            Action::Menos => "\u{E949}",
            Action::Mas => "\u{E948}",
            Action::Alternar => "Cambiar",
            Action::Quitar => "Quitar",
            Action::Abrir => "Abrir",
            Action::Cambiar => "Elegir...",
            Action::Desinstalar => "Desinstalar",
            Action::Subir => "\u{E70E}",
            Action::Bajar => "\u{E70D}",
            Action::Anclar => "Anclar",
            Action::Examinar => "Examinar...",
            Action::Buscar => "Buscar",
        }
    }

    /// Los botones con glifo se dibujan cuadrados.
    pub fn is_glyph(self) -> bool {
        matches!(self, Action::Menos | Action::Mas | Action::Subir | Action::Bajar)
    }
}

/// Una fila de una lista nativa.
pub struct Entry {
    pub glyph: &'static str,
    pub title: String,
    pub sub: String,
    /// Identificador interno con el que `act()` sabe sobre qué actuar.
    pub key: String,
    /// `Some(true)` dibuja la fila como la opción activa.
    pub on: Option<bool>,
    pub actions: Vec<Action>,
}

impl Entry {
    fn new(glyph: &'static str, title: impl Into<String>, sub: impl Into<String>) -> Entry {
        Entry {
            glyph,
            title: title.into(),
            sub: sub.into(),
            key: String::new(),
            on: None,
            actions: Vec::new(),
        }
    }
    fn key(mut self, k: impl Into<String>) -> Entry {
        self.key = k.into();
        self
    }
    fn on(mut self, v: bool) -> Entry {
        self.on = Some(v);
        self
    }
    fn act(mut self, a: &[Action]) -> Entry {
        self.actions = a.to_vec();
        self
    }
}

/// Cada una de las listas nativas que dibuja la Configuración.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ListKind {
    /// Planes de energía, apagado de pantalla y suspensión.
    Energia,
    /// Dispositivos de salida de audio y volumen.
    Audio,
    /// Dispositivos de entrada (micrófonos).
    AudioEntrada,
    /// Radios y dispositivos Bluetooth emparejados.
    Bluetooth,
    Impresoras,
    Raton,
    Teclado,
    /// Adaptadores de red con su IP y su puerta de enlace.
    Adaptadores,
    /// Redes Wi-Fi al alcance.
    Wifi,
    /// Programas instalados (desinstalar).
    Programas,
    /// Tipos de archivo y protocolos con su aplicación predeterminada.
    Predeterminadas,
    /// Opciones de accesibilidad del sistema.
    Accesibilidad,
    /// Iconos anclados a la barra de tareas.
    Anclados,
    /// Pantalla de bloqueo de Windows y la propia.
    Bloqueo,
    /// Herramientas clásicas (administrador de dispositivos, discos...).
    Herramientas,
}

/// Filas de una lista, recién leídas del sistema.
pub fn entries(kind: ListKind) -> Vec<Entry> {
    match kind {
        ListKind::Energia => energia(),
        ListKind::Audio => audio(false),
        ListKind::AudioEntrada => audio(true),
        ListKind::Bluetooth => bluetooth(),
        ListKind::Impresoras => impresoras(),
        ListKind::Raton => raton(),
        ListKind::Teclado => teclado(),
        ListKind::Adaptadores => adaptadores(),
        ListKind::Wifi => wifi(),
        ListKind::Programas => programas(),
        ListKind::Predeterminadas => predeterminadas(),
        ListKind::Accesibilidad => accesibilidad(),
        ListKind::Anclados => anclados(),
        ListKind::Bloqueo => bloqueo(),
        ListKind::Herramientas => herramientas(),
    }
}

/// Ejecuta el botón pulsado. Devuelve el mensaje que hay que enseñar.
pub fn act(kind: ListKind, index: usize, key: &str, action: Action, owner: HWND) -> Option<String> {
    match kind {
        ListKind::Energia => energia_act(key, action),
        ListKind::Audio => audio_act(key, action, false),
        ListKind::AudioEntrada => audio_act(key, action, true),
        ListKind::Bluetooth => bluetooth_act(key, action),
        ListKind::Impresoras => impresoras_act(key, action),
        ListKind::Raton => raton_act(key, action),
        ListKind::Teclado => teclado_act(key, action),
        ListKind::Adaptadores => adaptadores_act(key, action),
        ListKind::Wifi => wifi_act(key, action),
        ListKind::Programas => programas_act(key, action),
        ListKind::Predeterminadas => predeterminadas_act(key, action, owner),
        ListKind::Accesibilidad => accesibilidad_act(key, action),
        ListKind::Anclados => anclados_act(index, key, action, owner),
        ListKind::Bloqueo => bloqueo_act(key, action, owner),
        ListKind::Herramientas => herramientas_act(key, action),
    }
}

// ============================================================ utilidades
fn from_utf16(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

unsafe fn from_utf16_ptr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0usize;
    while *p.add(n) != 0 && n < 4096 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

/// Biblioteca cargada bajo demanda, con los procedimientos que se le piden.
struct Dll(HMODULE);

impl Dll {
    fn open(name: &str) -> Option<Dll> {
        let m = unsafe { LoadLibraryW(wide(name).as_ptr()) };
        if m.is_null() {
            None
        } else {
            Some(Dll(m))
        }
    }
    fn proc(&self, name: &str) -> Option<*const c_void> {
        let c = format!("{}\0", name);
        let p = unsafe { GetProcAddress(self.0, c.as_ptr()) };
        p.map(|f| f as *const c_void)
    }
}

impl Drop for Dll {
    fn drop(&mut self) {
        unsafe { FreeLibrary(self.0) };
    }
}

// ---------------------------------------------------------------- registro
fn reg_read_sz(root: HKEY, sub: &str, name: &str) -> Option<String> {
    unsafe {
        let mut k: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(root, wide(sub).as_ptr(), 0, KEY_QUERY_VALUE, &mut k) != 0 {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        let mut kind = 0u32;
        let ok = RegQueryValueExW(k, wide(name).as_ptr(), std::ptr::null_mut(), &mut kind,
                                  buf.as_mut_ptr() as *mut u8, &mut len) == 0;
        RegCloseKey(k);
        if ok { Some(from_utf16(&buf)) } else { None }
    }
}

fn reg_read_dword(root: HKEY, sub: &str, name: &str) -> Option<u32> {
    unsafe {
        let mut k: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(root, wide(sub).as_ptr(), 0, KEY_QUERY_VALUE, &mut k) != 0 {
            return None;
        }
        let mut v = 0u32;
        let mut len = 4u32;
        let mut kind = 0u32;
        let ok = RegQueryValueExW(k, wide(name).as_ptr(), std::ptr::null_mut(), &mut kind,
                                  &mut v as *mut u32 as *mut u8, &mut len) == 0;
        RegCloseKey(k);
        if ok { Some(v) } else { None }
    }
}

fn reg_write_sz(root: HKEY, sub: &str, name: &str, value: &str) -> bool {
    unsafe {
        let mut k: HKEY = std::ptr::null_mut();
        if RegCreateKeyExW(root, wide(sub).as_ptr(), 0, std::ptr::null(), 0, KEY_SET_VALUE,
                           std::ptr::null(), &mut k, std::ptr::null_mut()) != 0 {
            return false;
        }
        let data = wide(value);
        let ok = RegSetValueExW(k, wide(name).as_ptr(), 0, REG_SZ,
                                data.as_ptr() as *const u8, (data.len() * 2) as u32) == 0;
        RegCloseKey(k);
        ok
    }
}

fn reg_write_dword(root: HKEY, sub: &str, name: &str, value: u32) -> bool {
    unsafe {
        let mut k: HKEY = std::ptr::null_mut();
        if RegCreateKeyExW(root, wide(sub).as_ptr(), 0, std::ptr::null(), 0, KEY_SET_VALUE,
                           std::ptr::null(), &mut k, std::ptr::null_mut()) != 0 {
            return false;
        }
        let ok = RegSetValueExW(k, wide(name).as_ptr(), 0, REG_DWORD,
                                &value as *const u32 as *const u8, 4) == 0;
        RegCloseKey(k);
        ok
    }
}

/// Nombres de las subclaves de una clave (con la vista de 32 o 64 bits).
fn reg_subkeys(root: HKEY, sub: &str, extra: u32) -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let mut k: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(root, wide(sub).as_ptr(), 0, KEY_READ | extra, &mut k) != 0 {
            return out;
        }
        let mut i = 0u32;
        loop {
            let mut name = [0u16; 512];
            let mut len = name.len() as u32;
            let r = RegEnumKeyExW(k, i, name.as_mut_ptr(), &mut len, std::ptr::null_mut(),
                                  std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
            if r != 0 {
                break;
            }
            out.push(from_utf16(&name));
            i += 1;
            if i > 4096 {
                break;
            }
        }
        RegCloseKey(k);
    }
    out
}

fn reg_read_sz_view(root: HKEY, sub: &str, name: &str, extra: u32) -> Option<String> {
    unsafe {
        let mut k: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(root, wide(sub).as_ptr(), 0, KEY_QUERY_VALUE | extra, &mut k) != 0 {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        let mut kind = 0u32;
        let ok = RegQueryValueExW(k, wide(name).as_ptr(), std::ptr::null_mut(), &mut kind,
                                  buf.as_mut_ptr() as *mut u8, &mut len) == 0;
        RegCloseKey(k);
        if ok {
            let s = from_utf16(&buf);
            if s.is_empty() { None } else { Some(s) }
        } else {
            None
        }
    }
}

fn reg_read_dword_view(root: HKEY, sub: &str, name: &str, extra: u32) -> Option<u32> {
    unsafe {
        let mut k: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(root, wide(sub).as_ptr(), 0, KEY_QUERY_VALUE | extra, &mut k) != 0 {
            return None;
        }
        let mut v = 0u32;
        let mut len = 4u32;
        let mut kind = 0u32;
        let ok = RegQueryValueExW(k, wide(name).as_ptr(), std::ptr::null_mut(), &mut kind,
                                  &mut v as *mut u32 as *mut u8, &mut len) == 0;
        RegCloseKey(k);
        if ok { Some(v) } else { None }
    }
}

// ------------------------------------------------------------ parámetros SPI
const SPIF_APPLY: u32 = 0x0003; // UPDATEINIFILE | SENDCHANGE

fn spi_get_u32(action: u32) -> u32 {
    let mut v = 0u32;
    unsafe { SystemParametersInfoW(action, 0, &mut v as *mut u32 as *mut c_void, 0) };
    v
}

fn spi_set_uiparam(action: u32, value: u32) -> bool {
    unsafe { SystemParametersInfoW(action, value, std::ptr::null_mut(), SPIF_APPLY) != 0 }
}

fn spi_get_bool(action: u32) -> bool {
    let mut v = 0i32;
    unsafe { SystemParametersInfoW(action, 0, &mut v as *mut i32 as *mut c_void, 0) };
    v != 0
}

fn spi_set_bool(action: u32, on: bool) -> bool {
    unsafe { SystemParametersInfoW(action, on as u32, std::ptr::null_mut(), SPIF_APPLY) != 0 }
}

/// Estructura de accesibilidad de dos campos (`cbSize`, `dwFlags`).
#[repr(C)]
#[derive(Clone, Copy)]
struct Flags2 {
    cb: u32,
    flags: u32,
}

fn flags2_get(action: u32) -> u32 {
    let mut s = Flags2 { cb: std::mem::size_of::<Flags2>() as u32, flags: 0 };
    unsafe {
        SystemParametersInfoW(action, s.cb, &mut s as *mut Flags2 as *mut c_void, 0);
    }
    s.flags
}

fn flags2_set(action: u32, flags: u32) -> bool {
    let mut s = Flags2 { cb: std::mem::size_of::<Flags2>() as u32, flags };
    unsafe {
        SystemParametersInfoW(action, s.cb, &mut s as *mut Flags2 as *mut c_void, SPIF_APPLY) != 0
    }
}

fn si(on: bool) -> &'static str {
    if on { "Activado" } else { "Desactivado" }
}

// ============================================================ COM a mano
type ComFn = unsafe extern "system" fn();

unsafe fn vtable(obj: *mut c_void) -> *const *const ComFn {
    *(obj as *const *const *const ComFn)
}

unsafe fn com_release(obj: *mut c_void) {
    if obj.is_null() {
        return;
    }
    let f: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(*vtable(obj).add(2));
    f(obj);
}

unsafe fn com_method<T>(obj: *mut c_void, index: usize) -> T {
    std::mem::transmute_copy(&*vtable(obj).add(index))
}

// ============================================================ energía
// `powrprof.dll`: planes de energía y los dos tiempos que de verdad se tocan
// (apagar la pantalla y suspender), con corriente y con batería.
const GUID_VIDEO_SUBGROUP: GUID = GUID::from_u128(0x7516b95f_f776_4464_8c53_06167f40cc99);
const GUID_VIDEO_TIMEOUT: GUID = GUID::from_u128(0x3c0bc021_c8a8_4e07_a973_6b14cbcb2b7e);
const GUID_SLEEP_SUBGROUP: GUID = GUID::from_u128(0x238c9fa8_0aad_41ed_83f4_97be242c8f20);
const GUID_STANDBY_TIMEOUT: GUID = GUID::from_u128(0x29f6c1db_86da_48c5_9fdb_f2b67b1f44da);
const ACCESS_SCHEME: u32 = 16;

type FnPowerEnumerate = unsafe extern "system" fn(*mut c_void, *const GUID, *const GUID, u32, u32, *mut u8, *mut u32) -> u32;
type FnPowerReadFriendlyName = unsafe extern "system" fn(*mut c_void, *const GUID, *const GUID, *const GUID, *mut u8, *mut u32) -> u32;
type FnPowerGetActiveScheme = unsafe extern "system" fn(*mut c_void, *mut *mut GUID) -> u32;
type FnPowerSetActiveScheme = unsafe extern "system" fn(*mut c_void, *const GUID) -> u32;
type FnPowerReadIndex = unsafe extern "system" fn(*mut c_void, *const GUID, *const GUID, *const GUID, *mut u32) -> u32;
type FnPowerWriteIndex = unsafe extern "system" fn(*mut c_void, *const GUID, *const GUID, *const GUID, u32) -> u32;
type FnSetSuspendState = unsafe extern "system" fn(u8, u8, u8) -> u8;

struct PowerApi {
    dll: Dll,
    enumerate: FnPowerEnumerate,
    friendly: FnPowerReadFriendlyName,
    get_active: FnPowerGetActiveScheme,
    set_active: FnPowerSetActiveScheme,
    read_ac: FnPowerReadIndex,
    read_dc: FnPowerReadIndex,
    write_ac: FnPowerWriteIndex,
    write_dc: FnPowerWriteIndex,
}

impl PowerApi {
    fn load() -> Option<PowerApi> {
        let dll = Dll::open("powrprof.dll")?;
        unsafe {
            Some(PowerApi {
                enumerate: std::mem::transmute(dll.proc("PowerEnumerate")?),
                friendly: std::mem::transmute(dll.proc("PowerReadFriendlyName")?),
                get_active: std::mem::transmute(dll.proc("PowerGetActiveScheme")?),
                set_active: std::mem::transmute(dll.proc("PowerSetActiveScheme")?),
                read_ac: std::mem::transmute(dll.proc("PowerReadACValueIndex")?),
                read_dc: std::mem::transmute(dll.proc("PowerReadDCValueIndex")?),
                write_ac: std::mem::transmute(dll.proc("PowerWriteACValueIndex")?),
                write_dc: std::mem::transmute(dll.proc("PowerWriteDCValueIndex")?),
                dll,
            })
        }
    }

    fn active(&self) -> Option<GUID> {
        unsafe {
            let mut p: *mut GUID = std::ptr::null_mut();
            if (self.get_active)(std::ptr::null_mut(), &mut p) != 0 || p.is_null() {
                return None;
            }
            let g = *p;
            windows_sys::Win32::Foundation::LocalFree(p as *mut c_void);
            Some(g)
        }
    }

    fn schemes(&self) -> Vec<(GUID, String)> {
        let mut out = Vec::new();
        unsafe {
            for i in 0..32u32 {
                let mut g: GUID = std::mem::zeroed();
                let mut size = std::mem::size_of::<GUID>() as u32;
                if (self.enumerate)(std::ptr::null_mut(), std::ptr::null(), std::ptr::null(),
                                    ACCESS_SCHEME, i, &mut g as *mut GUID as *mut u8, &mut size) != 0 {
                    break;
                }
                let mut buf = [0u16; 256];
                let mut len = (buf.len() * 2) as u32;
                let name = if (self.friendly)(std::ptr::null_mut(), &g, std::ptr::null(),
                                              std::ptr::null(), buf.as_mut_ptr() as *mut u8, &mut len) == 0 {
                    from_utf16(&buf)
                } else {
                    "Plan de energía".to_string()
                };
                out.push((g, name));
            }
        }
        out
    }

    fn timeout(&self, scheme: &GUID, sub: &GUID, setting: &GUID, ac: bool) -> u32 {
        unsafe {
            let mut v = 0u32;
            let f = if ac { self.read_ac } else { self.read_dc };
            f(std::ptr::null_mut(), scheme, sub, setting, &mut v);
            v
        }
    }

    fn set_timeout(&self, scheme: &GUID, sub: &GUID, setting: &GUID, ac: bool, seconds: u32) -> bool {
        unsafe {
            let f = if ac { self.write_ac } else { self.write_dc };
            if f(std::ptr::null_mut(), scheme, sub, setting, seconds) != 0 {
                return false;
            }
            // Los cambios no surten efecto hasta reactivar el plan.
            (self.set_active)(std::ptr::null_mut(), scheme) == 0
        }
    }
}

fn minutos(seconds: u32) -> String {
    if seconds == 0 {
        "Nunca".to_string()
    } else if seconds % 3600 == 0 {
        format!("{} h", seconds / 3600)
    } else {
        format!("{} min", seconds / 60)
    }
}

fn energia() -> Vec<Entry> {
    let Some(api) = PowerApi::load() else {
        return vec![Entry::new("\u{E945}", "Energía",
                               "Este Windows no expone powrprof.dll: no se pueden leer los planes.")];
    };
    let active = api.active();
    let mut out = Vec::new();
    for (g, name) in api.schemes() {
        let on = active.as_ref().map(|a| guid_eq(a, &g)).unwrap_or(false);
        let guid = guid_text(&g);
        let mut e = Entry::new("\u{E945}", name, if on { "Plan activo" } else { "Disponible" })
            .key(format!("plan:{}", guid))
            .on(on);
        if !on {
            e = e.act(&[Action::Usar]);
        }
        out.push(e);
    }
    if let Some(scheme) = active {
        let battery = crate::sysinfo::battery().present;
        let rows: &[(&str, &GUID, &GUID, bool, &str)] = &[
            ("Apagar la pantalla (con corriente)", &GUID_VIDEO_SUBGROUP, &GUID_VIDEO_TIMEOUT, true, "video_ac"),
            ("Suspender el equipo (con corriente)", &GUID_SLEEP_SUBGROUP, &GUID_STANDBY_TIMEOUT, true, "sleep_ac"),
            ("Apagar la pantalla (con batería)", &GUID_VIDEO_SUBGROUP, &GUID_VIDEO_TIMEOUT, false, "video_dc"),
            ("Suspender el equipo (con batería)", &GUID_SLEEP_SUBGROUP, &GUID_STANDBY_TIMEOUT, false, "sleep_dc"),
        ];
        for (label, sub, setting, ac, key) in rows {
            if !ac && !battery {
                continue;
            }
            let v = api.timeout(&scheme, sub, setting, *ac);
            out.push(Entry::new("\u{E916}", *label, minutos(v))
                .key(*key)
                .act(&[Action::Menos, Action::Mas]));
        }
    }
    out.push(Entry::new("\u{E708}", "Suspender ahora", "El equipo entra en suspensión")
        .key("suspend").act(&[Action::Usar]));
    out.push(Entry::new("\u{E7E8}", "Hibernar ahora",
                        "Guarda la sesión en disco y apaga (si la hibernación está habilitada)")
        .key("hibernate").act(&[Action::Usar]));
    out
}

fn guid_eq(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

fn guid_text(g: &GUID) -> String {
    format!("{:08x}-{:04x}-{:04x}-{}", g.data1, g.data2, g.data3,
            g.data4.iter().map(|b| format!("{:02x}", b)).collect::<String>())
}

fn guid_from_text(s: &str) -> Option<GUID> {
    let hex: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 32 {
        return None;
    }
    let v = u128::from_str_radix(&hex, 16).ok()?;
    Some(GUID::from_u128(v))
}

fn energia_act(key: &str, action: Action) -> Option<String> {
    let api = PowerApi::load()?;
    if let Some(rest) = key.strip_prefix("plan:") {
        let g = guid_from_text(rest)?;
        let ok = unsafe { (api.set_active)(std::ptr::null_mut(), &g) == 0 };
        return Some(if ok {
            "Plan de energía activado.".to_string()
        } else {
            "Windows no ha dejado cambiar el plan de energía.".to_string()
        });
    }
    if key == "suspend" || key == "hibernate" {
        let dll = Dll::open("powrprof.dll")?;
        let f: FnSetSuspendState = unsafe { std::mem::transmute(dll.proc("SetSuspendState")?) };
        let hibernate = (key == "hibernate") as u8;
        let ok = unsafe { f(hibernate, 0, 0) != 0 };
        return Some(if ok {
            String::new()
        } else {
            "Windows no ha podido suspender el equipo.".to_string()
        });
    }
    let scheme = api.active()?;
    let (sub, setting, ac) = match key {
        "video_ac" => (&GUID_VIDEO_SUBGROUP, &GUID_VIDEO_TIMEOUT, true),
        "video_dc" => (&GUID_VIDEO_SUBGROUP, &GUID_VIDEO_TIMEOUT, false),
        "sleep_ac" => (&GUID_SLEEP_SUBGROUP, &GUID_STANDBY_TIMEOUT, true),
        "sleep_dc" => (&GUID_SLEEP_SUBGROUP, &GUID_STANDBY_TIMEOUT, false),
        _ => return None,
    };
    let cur = api.timeout(&scheme, sub, setting, ac);
    // Pasos cómodos: 1, 2, 3, 5, 10, 15, 20, 30, 45 min, 1 h, 2 h, 3 h, nunca.
    const PASOS: &[u32] = &[60, 120, 180, 300, 600, 900, 1200, 1800, 2700, 3600, 7200, 10800, 0];
    let idx = PASOS.iter().position(|&v| v == cur).unwrap_or_else(|| {
        PASOS.iter().position(|&v| v >= cur && v != 0).unwrap_or(PASOS.len() - 1)
    });
    let next = match action {
        Action::Mas => PASOS[(idx + 1).min(PASOS.len() - 1)],
        Action::Menos => PASOS[idx.saturating_sub(1)],
        _ => return None,
    };
    let ok = api.set_timeout(&scheme, sub, setting, ac, next);
    Some(if ok {
        format!("Nuevo valor: {}.", minutos(next))
    } else {
        "Windows no ha dejado escribir el plan de energía.".to_string()
    })
}

// ============================================================ audio
const CLSID_MM_DEVICE_ENUMERATOR: GUID = GUID::from_u128(0xBCDE0395_E52F_467C_8E3D_C4579291692E);
const IID_IMM_DEVICE_ENUMERATOR: GUID = GUID::from_u128(0xA95664D2_9614_4F35_A746_DE8DB63617E6);
const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870AF99C_171D_4F9E_AF0D_E63DF40C2BC9);
const IID_IPOLICY_CONFIG: GUID = GUID::from_u128(0xF8679F50_850A_41CF_9C72_430F290290C8);
/// PKEY_Device_FriendlyName.
const FMTID_DEVICE: GUID = GUID::from_u128(0xA45C254E_DF1C_4EFD_8020_67D146A850E0);
const PID_FRIENDLY_NAME: u32 = 14;

const V_ENUM_ENDPOINTS: usize = 3;
const V_GET_DEFAULT_ENDPOINT: usize = 4;
const V_COLLECTION_COUNT: usize = 3;
const V_COLLECTION_ITEM: usize = 4;
const V_DEV_OPEN_PROPS: usize = 4;
const V_DEV_GET_ID: usize = 5;
const V_DEV_GET_STATE: usize = 6;
const V_PROPS_GET_VALUE: usize = 5;
const V_SET_DEFAULT_ENDPOINT: usize = 13;

#[repr(C)]
struct PropertyKey {
    fmtid: GUID,
    pid: u32,
}

#[repr(C)]
struct PropVariant {
    vt: u16,
    r1: u16,
    r2: u16,
    r3: u16,
    val: *mut u16,
    pad: usize,
}

/// (id, nombre, activo) de cada punto final de audio.
fn audio_devices(capture: bool) -> (Vec<(String, String)>, String) {
    let mut out = Vec::new();
    let mut default_id = String::new();
    crate::ui::com_init();
    unsafe {
        let mut enumer: *mut c_void = std::ptr::null_mut();
        if CoCreateInstance(&CLSID_MM_DEVICE_ENUMERATOR, std::ptr::null_mut(), CLSCTX_INPROC_SERVER,
                            &IID_IMM_DEVICE_ENUMERATOR, &mut enumer) != 0 || enumer.is_null() {
            return (out, default_id);
        }
        let flow = if capture { 1i32 } else { 0i32 }; // eCapture / eRender
        // DEVICE_STATE_ACTIVE = 1.
        let enum_endpoints: unsafe extern "system" fn(*mut c_void, i32, u32, *mut *mut c_void) -> i32 =
            com_method(enumer, V_ENUM_ENDPOINTS);
        let get_default: unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void) -> i32 =
            com_method(enumer, V_GET_DEFAULT_ENDPOINT);

        let mut dev: *mut c_void = std::ptr::null_mut();
        if get_default(enumer, flow, 0, &mut dev) == 0 && !dev.is_null() {
            default_id = device_id(dev);
            com_release(dev);
        }

        let mut coll: *mut c_void = std::ptr::null_mut();
        if enum_endpoints(enumer, flow, 1, &mut coll) == 0 && !coll.is_null() {
            let count_fn: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 =
                com_method(coll, V_COLLECTION_COUNT);
            let item_fn: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32 =
                com_method(coll, V_COLLECTION_ITEM);
            let mut n = 0u32;
            count_fn(coll, &mut n);
            for i in 0..n.min(32) {
                let mut d: *mut c_void = std::ptr::null_mut();
                if item_fn(coll, i, &mut d) != 0 || d.is_null() {
                    continue;
                }
                let id = device_id(d);
                let name = device_name(d);
                com_release(d);
                if !id.is_empty() {
                    out.push((id, name));
                }
            }
            com_release(coll);
        }
        com_release(enumer);
    }
    (out, default_id)
}

unsafe fn device_id(dev: *mut c_void) -> String {
    let get_id: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> i32 =
        com_method(dev, V_DEV_GET_ID);
    let mut p: *mut u16 = std::ptr::null_mut();
    if get_id(dev, &mut p) != 0 || p.is_null() {
        return String::new();
    }
    let s = from_utf16_ptr(p);
    CoTaskMemFree(p as *mut c_void);
    s
}

unsafe fn device_name(dev: *mut c_void) -> String {
    let open: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32 =
        com_method(dev, V_DEV_OPEN_PROPS);
    let mut store: *mut c_void = std::ptr::null_mut();
    // STGM_READ = 0.
    if open(dev, 0, &mut store) != 0 || store.is_null() {
        return "Dispositivo de audio".to_string();
    }
    let get_value: unsafe extern "system" fn(*mut c_void, *const PropertyKey, *mut PropVariant) -> i32 =
        com_method(store, V_PROPS_GET_VALUE);
    let key = PropertyKey { fmtid: FMTID_DEVICE, pid: PID_FRIENDLY_NAME };
    let mut pv: PropVariant = std::mem::zeroed();
    let mut name = String::new();
    if get_value(store, &key, &mut pv) == 0 && pv.vt == 31 {
        name = from_utf16_ptr(pv.val);
        CoTaskMemFree(pv.val as *mut c_void);
    }
    com_release(store);
    if name.is_empty() {
        "Dispositivo de audio".to_string()
    } else {
        name
    }
}

fn audio(capture: bool) -> Vec<Entry> {
    let (devs, default_id) = audio_devices(capture);
    let mut out = Vec::new();
    if devs.is_empty() {
        out.push(Entry::new("\u{E767}",
                            if capture { "Sin micrófonos" } else { "Sin dispositivos de salida" },
                            "Core Audio no devuelve ningún punto final activo."));
    }
    for (id, name) in devs {
        let on = id.eq_ignore_ascii_case(&default_id);
        let mut e = Entry::new(if capture { "\u{E720}" } else { "\u{E767}" }, name,
                               if on { "Predeterminado" } else { "Disponible" })
            .key(id)
            .on(on);
        if !on {
            e = e.act(&[Action::Usar]);
        }
        out.push(e);
    }
    if !capture {
        let (vol, muted) = crate::sysinfo::volume().unwrap_or((0, false));
        out.push(Entry::new("\u{E767}", "Volumen general", format!("{} %", vol))
            .key("vol").act(&[Action::Menos, Action::Mas]));
        out.push(Entry::new("\u{E74F}", "Silencio", si(muted))
            .key("mute").on(muted).act(&[Action::Alternar]));
    }
    out
}

fn set_default_endpoint(id: &str) -> bool {
    crate::ui::com_init();
    unsafe {
        let mut pc: *mut c_void = std::ptr::null_mut();
        if CoCreateInstance(&CLSID_POLICY_CONFIG, std::ptr::null_mut(), CLSCTX_ALL,
                            &IID_IPOLICY_CONFIG, &mut pc) != 0 || pc.is_null() {
            return false;
        }
        let set: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32 =
            com_method(pc, V_SET_DEFAULT_ENDPOINT);
        let w = wide(id);
        // eConsole = 0, eMultimedia = 1, eCommunications = 2.
        let mut ok = true;
        for role in 0..3 {
            if set(pc, w.as_ptr(), role) != 0 {
                ok = false;
            }
        }
        com_release(pc);
        ok
    }
}

fn audio_act(key: &str, action: Action, capture: bool) -> Option<String> {
    match (key, action) {
        ("vol", Action::Mas) | ("vol", Action::Menos) => {
            let (v, _) = crate::sysinfo::volume().unwrap_or((0, false));
            let next = (v + if action == Action::Mas { 5 } else { -5 }).clamp(0, 100);
            crate::sysinfo::set_volume(next);
            Some(format!("Volumen al {} %.", next))
        }
        ("mute", _) => {
            let muted = crate::sysinfo::toggle_mute();
            Some(if muted { "Sonido silenciado.".into() } else { "Sonido restablecido.".into() })
        }
        (id, Action::Usar) => {
            let ok = set_default_endpoint(id);
            Some(if ok {
                format!("Dispositivo de {} predeterminado cambiado.",
                        if capture { "entrada" } else { "salida" })
            } else {
                "Windows no ha dejado cambiar el dispositivo predeterminado.".to_string()
            })
        }
        _ => None,
    }
}

// ============================================================ Bluetooth
#[repr(C)]
struct BtFindRadioParams {
    size: u32,
}

#[repr(C)]
struct BtDeviceSearchParams {
    size: u32,
    authenticated: i32,
    remembered: i32,
    unknown: i32,
    connected: i32,
    issue_inquiry: i32,
    timeout_multiplier: u8,
    radio: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BtDeviceInfo {
    size: u32,
    address: u64,
    class_of_device: u32,
    connected: i32,
    remembered: i32,
    authenticated: i32,
    last_seen: [u16; 8],
    last_used: [u16; 8],
    name: [u16; 248],
}

type FnBtFindFirstRadio = unsafe extern "system" fn(*const BtFindRadioParams, *mut *mut c_void) -> *mut c_void;
type FnBtFindNextRadio = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32;
type FnBtFindRadioClose = unsafe extern "system" fn(*mut c_void) -> i32;
type FnBtFindFirstDevice = unsafe extern "system" fn(*const BtDeviceSearchParams, *mut BtDeviceInfo) -> *mut c_void;
type FnBtFindNextDevice = unsafe extern "system" fn(*mut c_void, *mut BtDeviceInfo) -> i32;
type FnBtFindDeviceClose = unsafe extern "system" fn(*mut c_void) -> i32;
type FnBtRemoveDevice = unsafe extern "system" fn(*const u64) -> u32;

/// Clase mayor del dispositivo -> glifo e idea de qué es.
fn bt_kind(class: u32) -> (&'static str, &'static str) {
    match (class >> 8) & 0x1F {
        1 => ("\u{E770}", "Equipo"),
        2 => ("\u{E8EA}", "Teléfono"),
        4 => ("\u{E7F6}", "Audio o vídeo"),
        5 => ("\u{E765}", "Periférico"),
        6 => ("\u{E749}", "Imagen"),
        7 => ("\u{E7EE}", "Ponible"),
        _ => ("\u{E702}", "Dispositivo"),
    }
}

fn bluetooth() -> Vec<Entry> {
    let mut out = Vec::new();
    let Some(dll) = Dll::open("bthprops.cpl") else {
        return vec![Entry::new("\u{E702}", "Bluetooth",
                               "Este equipo no tiene la pila de Bluetooth instalada.")];
    };
    unsafe {
        // ---- radios
        let mut radios = 0;
        if let (Some(first), Some(next), Some(close)) = (dll.proc("BluetoothFindFirstRadio"),
                                                         dll.proc("BluetoothFindNextRadio"),
                                                         dll.proc("BluetoothFindRadioClose")) {
            let first: FnBtFindFirstRadio = std::mem::transmute(first);
            let next: FnBtFindNextRadio = std::mem::transmute(next);
            let close: FnBtFindRadioClose = std::mem::transmute(close);
            let params = BtFindRadioParams { size: std::mem::size_of::<BtFindRadioParams>() as u32 };
            let mut radio: *mut c_void = std::ptr::null_mut();
            let find = first(&params, &mut radio);
            if !find.is_null() {
                loop {
                    radios += 1;
                    if !radio.is_null() {
                        windows_sys::Win32::Foundation::CloseHandle(radio);
                    }
                    radio = std::ptr::null_mut();
                    if next(find, &mut radio) == 0 {
                        break;
                    }
                }
                close(find);
            }
        }
        out.push(Entry::new("\u{E702}", "Radio Bluetooth",
                            if radios > 0 {
                                format!("{} radio(s) disponible(s)", radios)
                            } else {
                                "Sin radio activa".to_string()
                            })
            .key("radio").on(radios > 0));
        if radios == 0 {
            return out;
        }

        // ---- dispositivos emparejados y a la vista
        let (Some(first), Some(next), Some(close)) = (dll.proc("BluetoothFindFirstDevice"),
                                                      dll.proc("BluetoothFindNextDevice"),
                                                      dll.proc("BluetoothFindDeviceClose"))
        else {
            return out;
        };
        let first: FnBtFindFirstDevice = std::mem::transmute(first);
        let next: FnBtFindNextDevice = std::mem::transmute(next);
        let close: FnBtFindDeviceClose = std::mem::transmute(close);

        let params = BtDeviceSearchParams {
            size: std::mem::size_of::<BtDeviceSearchParams>() as u32,
            authenticated: 1,
            remembered: 1,
            unknown: 0,
            connected: 1,
            issue_inquiry: 0,
            timeout_multiplier: 2,
            radio: std::ptr::null_mut(),
        };
        let mut info: BtDeviceInfo = std::mem::zeroed();
        info.size = std::mem::size_of::<BtDeviceInfo>() as u32;
        let find = first(&params, &mut info);
        if find.is_null() {
            out.push(Entry::new("\u{E702}", "Ningún dispositivo emparejado",
                                "Empareja un dispositivo para verlo aquí"));
            return out;
        }
        let mut n = 0;
        loop {
            let (glyph, what) = bt_kind(info.class_of_device);
            let name = from_utf16(&info.name);
            let estado = if info.connected != 0 {
                "conectado"
            } else if info.authenticated != 0 {
                "emparejado"
            } else {
                "recordado"
            };
            out.push(Entry::new(glyph,
                                if name.is_empty() { format!("{:012X}", info.address) } else { name },
                                format!("{} · {}", what, estado))
                .key(format!("{:x}", info.address))
                .on(info.connected != 0)
                .act(&[Action::Quitar]));
            n += 1;
            if n >= 32 {
                break;
            }
            info = std::mem::zeroed();
            info.size = std::mem::size_of::<BtDeviceInfo>() as u32;
            if next(find, &mut info) == 0 {
                break;
            }
        }
        close(find);
    }
    out
}

fn bluetooth_act(key: &str, action: Action) -> Option<String> {
    if action != Action::Quitar {
        return None;
    }
    let addr = u64::from_str_radix(key, 16).ok()?;
    let dll = Dll::open("bthprops.cpl")?;
    let f: FnBtRemoveDevice = unsafe { std::mem::transmute(dll.proc("BluetoothRemoveDevice")?) };
    let r = unsafe { f(&addr) };
    Some(if r == 0 {
        "Dispositivo desemparejado.".to_string()
    } else {
        "No se ha podido desemparejar (hace falta permiso de administrador).".to_string()
    })
}

// ============================================================ impresoras
#[repr(C)]
struct PrinterInfo4 {
    name: *mut u16,
    server: *mut u16,
    attributes: u32,
}

type FnEnumPrinters = unsafe extern "system" fn(u32, *const u16, u32, *mut u8, u32, *mut u32, *mut u32) -> i32;
type FnGetDefaultPrinter = unsafe extern "system" fn(*mut u16, *mut u32) -> i32;
type FnSetDefaultPrinter = unsafe extern "system" fn(*const u16) -> i32;

fn impresoras() -> Vec<Entry> {
    let Some(dll) = Dll::open("winspool.drv") else { return Vec::new() };
    let mut out = Vec::new();
    unsafe {
        let default = dll.proc("GetDefaultPrinterW").map(|p| {
            let f: FnGetDefaultPrinter = std::mem::transmute(p);
            let mut buf = [0u16; 512];
            let mut len = buf.len() as u32;
            if f(buf.as_mut_ptr(), &mut len) != 0 {
                from_utf16(&buf)
            } else {
                String::new()
            }
        }).unwrap_or_default();

        let Some(p) = dll.proc("EnumPrintersW") else { return out };
        let enum_printers: FnEnumPrinters = std::mem::transmute(p);
        // PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS
        let flags = 0x00000002u32 | 0x00000004u32;
        let mut needed = 0u32;
        let mut returned = 0u32;
        enum_printers(flags, std::ptr::null(), 4, std::ptr::null_mut(), 0, &mut needed, &mut returned);
        if needed == 0 {
            out.push(Entry::new("\u{E749}", "Sin impresoras",
                                "No hay ninguna impresora instalada en este equipo"));
            return out;
        }
        let mut buf = vec![0u8; needed as usize + 64];
        if enum_printers(flags, std::ptr::null(), 4, buf.as_mut_ptr(), buf.len() as u32,
                         &mut needed, &mut returned) == 0 {
            return out;
        }
        let items = buf.as_ptr() as *const PrinterInfo4;
        for i in 0..returned.min(64) {
            let it = &*items.add(i as usize);
            let name = from_utf16_ptr(it.name);
            if name.is_empty() {
                continue;
            }
            let local = it.attributes & 0x00000040 != 0; // PRINTER_ATTRIBUTE_LOCAL
            let on = name.eq_ignore_ascii_case(&default);
            let mut e = Entry::new("\u{E749}", name.clone(),
                                   format!("{}{}", if local { "Local" } else { "De red" },
                                           if on { " · predeterminada" } else { "" }))
                .key(name)
                .on(on);
            e = if on {
                e.act(&[Action::Abrir])
            } else {
                e.act(&[Action::Usar, Action::Abrir])
            };
            out.push(e);
        }
    }
    out
}

fn impresoras_act(key: &str, action: Action) -> Option<String> {
    match action {
        Action::Usar => {
            let dll = Dll::open("winspool.drv")?;
            let f: FnSetDefaultPrinter = unsafe { std::mem::transmute(dll.proc("SetDefaultPrinterW")?) };
            let ok = unsafe { f(wide(key).as_ptr()) != 0 };
            Some(if ok {
                format!("«{}» es ahora la impresora predeterminada.", key)
            } else {
                "No se ha podido cambiar la impresora predeterminada.".to_string()
            })
        }
        Action::Abrir => {
            // Cola de impresión, sin pasar por la Configuración de Windows.
            run_hidden("rundll32.exe", &format!("printui.dll,PrintUIEntry /o /n \"{}\"", key));
            Some(String::new())
        }
        _ => None,
    }
}

// ============================================================ ratón y teclado
const SPI_GETMOUSESPEED: u32 = 0x0070;
const SPI_SETMOUSESPEED: u32 = 0x0071;
const SPI_GETWHEELSCROLLLINES: u32 = 0x0068;
const SPI_SETWHEELSCROLLLINES: u32 = 0x0069;
const SPI_GETKEYBOARDSPEED: u32 = 0x000A;
const SPI_SETKEYBOARDSPEED: u32 = 0x000B;
const SPI_GETKEYBOARDDELAY: u32 = 0x0016;
const SPI_SETKEYBOARDDELAY: u32 = 0x0017;
const SPI_GETCARETWIDTH: u32 = 0x2006;
const SPI_SETCARETWIDTH: u32 = 0x2007;
const SPI_SETCURSORS: u32 = 0x0057;
const SPI_GETHIGHCONTRAST: u32 = 0x0042;
const SPI_SETHIGHCONTRAST: u32 = 0x0043;
const SPI_GETSTICKYKEYS: u32 = 0x003A;
const SPI_SETSTICKYKEYS: u32 = 0x003B;
const SPI_GETFILTERKEYS: u32 = 0x0032;
const SPI_SETFILTERKEYS: u32 = 0x0033;
const SPI_GETTOGGLEKEYS: u32 = 0x0034;
const SPI_SETTOGGLEKEYS: u32 = 0x0035;
const SPI_GETMOUSEKEYS: u32 = 0x0036;
const SPI_SETMOUSEKEYS: u32 = 0x0037;
const SPI_GETSHOWSOUNDS: u32 = 0x0038;
const SPI_SETSHOWSOUNDS: u32 = 0x0039;
const SM_SWAPBUTTON: i32 = 23;

fn raton() -> Vec<Entry> {
    let speed = spi_get_u32(SPI_GETMOUSESPEED);
    let lines = spi_get_u32(SPI_GETWHEELSCROLLLINES);
    let dbl = unsafe { GetDoubleClickTime() };
    let swapped = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(SM_SWAPBUTTON) != 0
    };
    vec![
        Entry::new("\u{E962}", "Velocidad del puntero", format!("{} de 20", speed.max(1)))
            .key("speed").act(&[Action::Menos, Action::Mas]),
        Entry::new("\u{E962}", "Líneas por muesca de la rueda",
                   if lines == u32::MAX { "Una pantalla".to_string() } else { format!("{} líneas", lines) })
            .key("wheel").act(&[Action::Menos, Action::Mas]),
        Entry::new("\u{E8B0}", "Tiempo del doble clic", format!("{} ms", dbl))
            .key("dblclk").act(&[Action::Menos, Action::Mas]),
        Entry::new("\u{E962}", "Intercambiar los botones principal y secundario", si(swapped))
            .key("swap").on(swapped).act(&[Action::Alternar]),
        Entry::new("\u{E7C9}", "Tamaño del puntero",
                   format!("{} px", reg_read_dword(HKEY_CURRENT_USER, r"Control Panel\Cursors",
                                                   "CursorBaseSize").unwrap_or(32)))
            .key("cursor").act(&[Action::Menos, Action::Mas]),
    ]
}

fn raton_act(key: &str, action: Action) -> Option<String> {
    let dir: i32 = match action {
        Action::Mas => 1,
        Action::Menos => -1,
        Action::Alternar => 0,
        _ => return None,
    };
    match key {
        "speed" => {
            let v = (spi_get_u32(SPI_GETMOUSESPEED) as i32 + dir).clamp(1, 20);
            spi_set_uiparam(SPI_SETMOUSESPEED, v as u32);
            Some(format!("Velocidad del puntero: {} de 20.", v))
        }
        "wheel" => {
            let cur = spi_get_u32(SPI_GETWHEELSCROLLLINES);
            let v = if cur == u32::MAX { 1 } else { (cur as i32 + dir).clamp(1, 24) as u32 };
            spi_set_uiparam(SPI_SETWHEELSCROLLLINES, v);
            Some(format!("{} líneas por muesca.", v))
        }
        "dblclk" => {
            let v = (unsafe { GetDoubleClickTime() } as i32 + dir * 50).clamp(100, 900);
            unsafe { SetDoubleClickTime(v as u32) };
            Some(format!("Doble clic: {} ms.", v))
        }
        "swap" => {
            let swapped = unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(SM_SWAPBUTTON) != 0
            };
            unsafe {
                windows_sys::Win32::UI::Input::KeyboardAndMouse::SwapMouseButton((!swapped) as i32)
            };
            Some(if swapped { "Botones en su sitio.".into() } else { "Botones intercambiados.".into() })
        }
        "cursor" => {
            let cur = reg_read_dword(HKEY_CURRENT_USER, r"Control Panel\Cursors", "CursorBaseSize")
                .unwrap_or(32);
            let v = (cur as i32 + dir * 8).clamp(32, 256) as u32;
            reg_write_dword(HKEY_CURRENT_USER, r"Control Panel\Cursors", "CursorBaseSize", v);
            unsafe { SystemParametersInfoW(SPI_SETCURSORS, 0, std::ptr::null_mut(), SPIF_APPLY) };
            Some(format!("Puntero de {} px.", v))
        }
        _ => None,
    }
}

fn teclado() -> Vec<Entry> {
    let speed = spi_get_u32(SPI_GETKEYBOARDSPEED);
    let delay = spi_get_u32(SPI_GETKEYBOARDDELAY);
    let caret = spi_get_u32(SPI_GETCARETWIDTH);
    let blink = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCaretBlinkTime() };
    let layout = layout_name();
    vec![
        Entry::new("\u{E765}", "Velocidad de repetición", format!("{} de 31", speed))
            .key("speed").act(&[Action::Menos, Action::Mas]),
        Entry::new("\u{E916}", "Retardo antes de repetir",
                   format!("{} ms", 250 * (delay + 1)))
            .key("delay").act(&[Action::Menos, Action::Mas]),
        Entry::new("\u{E8E9}", "Grosor del cursor de texto", format!("{} px", caret.max(1)))
            .key("caret").act(&[Action::Menos, Action::Mas]),
        Entry::new("\u{E916}", "Intermitencia del cursor", format!("{} ms", blink))
            .key("blink").act(&[Action::Menos, Action::Mas]),
        Entry::new("\u{E8C1}", "Distribución del teclado", layout)
            .key("layout").act(&[Action::Alternar]),
    ]
}

/// Nombre legible de la distribución activa, sacado del registro.
fn layout_name() -> String {
    let mut buf = [0u16; 16];
    unsafe {
        windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayoutNameW(buf.as_mut_ptr());
    }
    let id = from_utf16(&buf);
    if id.is_empty() {
        return "Desconocida".to_string();
    }
    let sub = format!(r"SYSTEM\CurrentControlSet\Control\Keyboard Layouts\{}", id);
    reg_read_sz(HKEY_LOCAL_MACHINE, &sub, "Layout Text").unwrap_or(id)
}

fn teclado_act(key: &str, action: Action) -> Option<String> {
    let dir: i32 = match action {
        Action::Mas => 1,
        Action::Menos => -1,
        Action::Alternar => 0,
        _ => return None,
    };
    match key {
        "speed" => {
            let v = (spi_get_u32(SPI_GETKEYBOARDSPEED) as i32 + dir).clamp(0, 31);
            spi_set_uiparam(SPI_SETKEYBOARDSPEED, v as u32);
            Some(format!("Velocidad de repetición: {} de 31.", v))
        }
        "delay" => {
            let v = (spi_get_u32(SPI_GETKEYBOARDDELAY) as i32 + dir).clamp(0, 3);
            spi_set_uiparam(SPI_SETKEYBOARDDELAY, v as u32);
            Some(format!("Retardo de {} ms.", 250 * (v + 1)))
        }
        "caret" => {
            let v = (spi_get_u32(SPI_GETCARETWIDTH) as i32 + dir).clamp(1, 20);
            unsafe {
                SystemParametersInfoW(SPI_SETCARETWIDTH, 0, v as usize as *mut c_void, SPIF_APPLY);
            }
            Some(format!("Cursor de {} px.", v))
        }
        "blink" => {
            let cur = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCaretBlinkTime() };
            let v = (cur as i32 + dir * 100).clamp(200, 1200);
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::SetCaretBlinkTime(v as u32) };
            Some(format!("Intermitencia de {} ms.", v))
        }
        "layout" => {
            // HKL_NEXT: pasa a la siguiente distribución instalada.
            unsafe {
                windows_sys::Win32::UI::Input::KeyboardAndMouse::ActivateKeyboardLayout(
                    1 as *mut c_void, 0);
            }
            Some(format!("Distribución activa: {}.", layout_name()))
        }
        _ => None,
    }
}

// ============================================================ red
#[repr(C)]
struct IpAddrString {
    next: *mut IpAddrString,
    ip: [u8; 16],
    mask: [u8; 16],
    context: u32,
}

#[repr(C)]
struct IpAdapterInfo {
    next: *mut IpAdapterInfo,
    combo_index: u32,
    adapter_name: [u8; 260],
    description: [u8; 132],
    address_length: u32,
    address: [u8; 8],
    index: u32,
    kind: u32,
    dhcp_enabled: u32,
    current_ip: *mut IpAddrString,
    ip_list: IpAddrString,
    gateway_list: IpAddrString,
    dhcp_server: IpAddrString,
    have_wins: i32,
    primary_wins: IpAddrString,
    secondary_wins: IpAddrString,
    lease_obtained: isize,
    lease_expires: isize,
}

type FnGetAdaptersInfo = unsafe extern "system" fn(*mut u8, *mut u32) -> u32;
type FnDnsFlush = unsafe extern "system" fn() -> i32;

fn ascii(buf: &[u8]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).to_string()
}

fn if_type_name(kind: u32) -> &'static str {
    match kind {
        6 => "Ethernet",
        71 => "Wi-Fi",
        24 => "Bucle local",
        23 => "PPP (VPN)",
        131 => "Túnel",
        _ => "Adaptador",
    }
}

fn adaptadores() -> Vec<Entry> {
    let mut out = Vec::new();
    let Some(dll) = Dll::open("iphlpapi.dll") else { return out };
    unsafe {
        let Some(p) = dll.proc("GetAdaptersInfo") else { return out };
        let get: FnGetAdaptersInfo = std::mem::transmute(p);
        let mut size = 0u32;
        get(std::ptr::null_mut(), &mut size);
        if size == 0 {
            out.push(Entry::new("\u{E839}", "Sin adaptadores",
                                "Windows no declara ningún adaptador con IPv4"));
        } else {
            let mut buf = vec![0u8; size as usize + 1024];
            let mut size2 = buf.len() as u32;
            if get(buf.as_mut_ptr(), &mut size2) == 0 {
                let mut cur = buf.as_ptr() as *const IpAdapterInfo;
                let mut n = 0;
                while !cur.is_null() && n < 32 {
                    let a = &*cur;
                    let ip = ascii(&a.ip_list.ip);
                    let gw = ascii(&a.gateway_list.ip);
                    let mac: Vec<String> = a.address[..(a.address_length as usize).min(8)]
                        .iter().map(|b| format!("{:02X}", b)).collect();
                    let up = ip != "0.0.0.0" && !ip.is_empty();
                    let mut sub = format!("{} · IP {}", if_type_name(a.kind), if up { ip.as_str() } else { "sin asignar" });
                    if up && gw != "0.0.0.0" && !gw.is_empty() {
                        sub.push_str(&format!(" · puerta {}", gw));
                    }
                    if a.dhcp_enabled != 0 {
                        sub.push_str(" · DHCP");
                    }
                    if !mac.is_empty() {
                        sub.push_str(&format!(" · {}", mac.join(":")));
                    }
                    out.push(Entry::new(if a.kind == 71 { "\u{E701}" } else { "\u{E839}" },
                                        ascii(&a.description), sub)
                        .key(format!("if:{}", a.index))
                        .on(up));
                    cur = a.next;
                    n += 1;
                }
            }
        }
        // Velocidad del enlace y estado general, ya resueltos por `sysinfo`.
        let estado = match crate::sysinfo::network() {
            crate::sysinfo::Net::Off => "Sin conexión".to_string(),
            crate::sysinfo::Net::Ethernet { mbps } if mbps > 0 => format!("Ethernet a {} Mbps", mbps),
            crate::sysinfo::Net::Ethernet { .. } => "Ethernet conectado".to_string(),
            crate::sysinfo::Net::Wifi { ssid, quality } => format!("Wi-Fi {} · señal {} %", ssid, quality),
        };
        out.push(Entry::new("\u{E704}", "Conexión en uso", estado).key("estado"));
    }
    out.push(Entry::new("\u{E72C}", "Vaciar la caché de DNS",
                        "Olvida las resoluciones guardadas por el cliente DNS")
        .key("dns").act(&[Action::Usar]));
    out.push(Entry::new("\u{E968}", "Propiedades TCP/IP del adaptador",
                        "Abre el cuadro clásico de conexiones de red")
        .key("ncpa").act(&[Action::Abrir]));
    out
}

fn adaptadores_act(key: &str, action: Action) -> Option<String> {
    match (key, action) {
        ("dns", _) => {
            let dll = Dll::open("dnsapi.dll")?;
            let f: FnDnsFlush = unsafe { std::mem::transmute(dll.proc("DnsFlushResolverCache")?) };
            let ok = unsafe { f() != 0 };
            Some(if ok { "Caché de DNS vaciada.".into() } else { "No se ha podido vaciar la caché.".into() })
        }
        ("ncpa", _) => {
            ui::shell_open("ncpa.cpl");
            Some(String::new())
        }
        _ => None,
    }
}

// ------------------------------------------------------------ Wi-Fi
#[repr(C)]
struct Dot11Ssid {
    length: u32,
    ssid: [u8; 32],
}

#[repr(C)]
struct WlanInterfaceInfo {
    guid: GUID,
    description: [u16; 256],
    state: u32,
}

#[repr(C)]
struct WlanInterfaceInfoList {
    count: u32,
    index: u32,
}

#[repr(C)]
struct WlanAvailableNetwork {
    profile: [u16; 256],
    ssid: Dot11Ssid,
    bss_type: u32,
    bssid_count: u32,
    connectable: i32,
    not_connectable_reason: u32,
    phy_count: u32,
    phy_types: [u32; 8],
    more_phy: i32,
    signal_quality: u32,
    security: i32,
    auth_algorithm: u32,
    cipher_algorithm: u32,
    flags: u32,
    reserved: u32,
}

#[repr(C)]
struct WlanAvailableNetworkList {
    count: u32,
    index: u32,
}

#[repr(C)]
struct WlanConnectionParameters {
    mode: u32,
    profile: *const u16,
    ssid: *const Dot11Ssid,
    bssid_list: *const c_void,
    bss_type: u32,
    flags: u32,
}

const WLAN_CONNECTED: u32 = 1;
const WLAN_HAS_PROFILE: u32 = 2;

struct Wlan {
    dll: Dll,
    handle: *mut c_void,
    free: unsafe extern "system" fn(*mut c_void),
    close: unsafe extern "system" fn(*mut c_void, *mut c_void) -> u32,
}

impl Wlan {
    fn open() -> Option<Wlan> {
        let dll = Dll::open("wlanapi.dll")?;
        unsafe {
            let open: unsafe extern "system" fn(u32, *mut c_void, *mut u32, *mut *mut c_void) -> u32 =
                std::mem::transmute(dll.proc("WlanOpenHandle")?);
            let free: unsafe extern "system" fn(*mut c_void) = std::mem::transmute(dll.proc("WlanFreeMemory")?);
            let close: unsafe extern "system" fn(*mut c_void, *mut c_void) -> u32 =
                std::mem::transmute(dll.proc("WlanCloseHandle")?);
            let mut negotiated = 0u32;
            let mut handle: *mut c_void = std::ptr::null_mut();
            if open(2, std::ptr::null_mut(), &mut negotiated, &mut handle) != 0 || handle.is_null() {
                return None;
            }
            Some(Wlan { dll, handle, free, close })
        }
    }

    /// GUID de la primera interfaz inalámbrica.
    fn first_interface(&self) -> Option<GUID> {
        unsafe {
            let enum_ifaces: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut WlanInterfaceInfoList) -> u32 =
                std::mem::transmute(self.dll.proc("WlanEnumInterfaces")?);
            let mut list: *mut WlanInterfaceInfoList = std::ptr::null_mut();
            if enum_ifaces(self.handle, std::ptr::null_mut(), &mut list) != 0 || list.is_null() {
                return None;
            }
            let count = (*list).count as usize;
            let infos = (list as *const u8).add(std::mem::size_of::<WlanInterfaceInfoList>())
                as *const WlanInterfaceInfo;
            let g = if count > 0 { Some((*infos).guid) } else { None };
            (self.free)(list as *mut c_void);
            g
        }
    }
}

impl Drop for Wlan {
    fn drop(&mut self) {
        unsafe { (self.close)(self.handle, std::ptr::null_mut()) };
    }
}

fn wifi() -> Vec<Entry> {
    let Some(w) = Wlan::open() else {
        return vec![Entry::new("\u{E701}", "Wi-Fi",
                               "Este equipo no tiene adaptador inalámbrico (falta wlanapi).")];
    };
    let Some(guid) = w.first_interface() else {
        return vec![Entry::new("\u{E701}", "Wi-Fi", "Ninguna interfaz inalámbrica disponible.")];
    };
    let mut out = Vec::new();
    unsafe {
        let Some(p) = w.dll.proc("WlanGetAvailableNetworkList") else { return out };
        let get: unsafe extern "system" fn(*mut c_void, *const GUID, u32, *mut c_void, *mut *mut WlanAvailableNetworkList) -> u32 =
            std::mem::transmute(p);
        let mut list: *mut WlanAvailableNetworkList = std::ptr::null_mut();
        // 3 = incluir también las redes ocultas y las de los perfiles guardados.
        if get(w.handle, &guid, 3, std::ptr::null_mut(), &mut list) != 0 || list.is_null() {
            out.push(Entry::new("\u{E701}", "Sin resultados",
                                "El adaptador no ha devuelto ninguna red. Pulsa «Buscar»."));
        } else {
            let count = (*list).count as usize;
            let nets = (list as *const u8).add(std::mem::size_of::<WlanAvailableNetworkList>())
                as *const WlanAvailableNetwork;
            let mut seen: Vec<String> = Vec::new();
            for i in 0..count.min(64) {
                let n = &*nets.add(i);
                let len = (n.ssid.length as usize).min(32);
                let ssid = String::from_utf8_lossy(&n.ssid.ssid[..len]).to_string();
                if ssid.is_empty() || seen.iter().any(|s| s == &ssid) {
                    continue;
                }
                seen.push(ssid.clone());
                let profile = from_utf16(&n.profile);
                let connected = n.flags & WLAN_CONNECTED != 0;
                let has_profile = n.flags & WLAN_HAS_PROFILE != 0;
                let mut sub = format!("Señal {} % · {}", n.signal_quality,
                                      if n.security != 0 { "protegida" } else { "abierta" });
                if connected {
                    sub.push_str(" · conectada");
                } else if has_profile {
                    sub.push_str(" · perfil guardado");
                }
                let mut e = Entry::new("\u{E701}", ssid.clone(), sub)
                    .key(if has_profile && !profile.is_empty() { profile } else { ssid })
                    .on(connected);
                e = if connected {
                    e.act(&[Action::Quitar])
                } else if has_profile {
                    e.act(&[Action::Usar, Action::Quitar])
                } else {
                    e
                };
                out.push(e);
            }
            (w.free)(list as *mut c_void);
        }
    }
    out.push(Entry::new("\u{E72C}", "Buscar redes", "Pide al adaptador una exploración nueva")
        .key("scan").act(&[Action::Buscar]));
    out
}

fn wifi_act(key: &str, action: Action) -> Option<String> {
    let w = Wlan::open()?;
    let guid = w.first_interface()?;
    unsafe {
        match action {
            Action::Buscar => {
                let f: unsafe extern "system" fn(*mut c_void, *const GUID, *const c_void, *const c_void, *mut c_void) -> u32 =
                    std::mem::transmute(w.dll.proc("WlanScan")?);
                let r = f(w.handle, &guid, std::ptr::null(), std::ptr::null(), std::ptr::null_mut());
                Some(if r == 0 {
                    "Explorando... vuelve a abrir la lista en unos segundos.".to_string()
                } else {
                    "El adaptador ha rechazado la exploración.".to_string()
                })
            }
            Action::Usar => {
                let f: unsafe extern "system" fn(*mut c_void, *const GUID, *const WlanConnectionParameters, *mut c_void) -> u32 =
                    std::mem::transmute(w.dll.proc("WlanConnect")?);
                let profile = wide(key);
                let params = WlanConnectionParameters {
                    mode: 0, // wlan_connection_mode_profile
                    profile: profile.as_ptr(),
                    ssid: std::ptr::null(),
                    bssid_list: std::ptr::null(),
                    bss_type: 3, // dot11_BSS_type_any
                    flags: 0,
                };
                let r = f(w.handle, &guid, &params, std::ptr::null_mut());
                Some(if r == 0 {
                    format!("Conectando a «{}»...", key)
                } else {
                    "No se ha podido conectar con el perfil guardado.".to_string()
                })
            }
            Action::Quitar => {
                let f: unsafe extern "system" fn(*mut c_void, *const GUID, *mut c_void) -> u32 =
                    std::mem::transmute(w.dll.proc("WlanDisconnect")?);
                let r = f(w.handle, &guid, std::ptr::null_mut());
                Some(if r == 0 { "Wi-Fi desconectado.".into() } else { "No se ha podido desconectar.".into() })
            }
            _ => None,
        }
    }
}

// ============================================================ programas
const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

fn programas() -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let fuentes: [(HKEY, u32); 3] = [
        (HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY),
        (HKEY_CURRENT_USER, 0),
    ];
    for (root, view) in fuentes {
        for sub in reg_subkeys(root, UNINSTALL, view) {
            let path = format!("{}\\{}", UNINSTALL, sub);
            let Some(name) = reg_read_sz_view(root, &path, "DisplayName", view) else { continue };
            if reg_read_dword_view(root, &path, "SystemComponent", view).unwrap_or(0) == 1 {
                continue;
            }
            if reg_read_sz_view(root, &path, "ParentKeyName", view).is_some() {
                continue;
            }
            let version = reg_read_sz_view(root, &path, "DisplayVersion", view).unwrap_or_default();
            let publisher = reg_read_sz_view(root, &path, "Publisher", view).unwrap_or_default();
            let size_kb = reg_read_dword_view(root, &path, "EstimatedSize", view).unwrap_or(0);
            let uninstall = reg_read_sz_view(root, &path, "UninstallString", view)
                .or_else(|| reg_read_sz_view(root, &path, "QuietUninstallString", view));
            let mut sub_text = String::new();
            if !publisher.is_empty() {
                sub_text.push_str(&publisher);
            }
            if !version.is_empty() {
                if !sub_text.is_empty() {
                    sub_text.push_str(" · ");
                }
                sub_text.push_str(&format!("versión {}", version));
            }
            if size_kb > 0 {
                if !sub_text.is_empty() {
                    sub_text.push_str(" · ");
                }
                sub_text.push_str(&format!("{:.0} MB", size_kb as f64 / 1024.0));
            }
            if out.iter().any(|e| e.title.eq_ignore_ascii_case(&name)) {
                continue;
            }
            let mut e = Entry::new("\u{E71D}", name, sub_text);
            if let Some(cmd) = uninstall {
                e = e.key(cmd).act(&[Action::Desinstalar]);
            }
            out.push(e);
        }
    }
    out.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    out
}

fn programas_act(key: &str, action: Action) -> Option<String> {
    if action != Action::Desinstalar || key.is_empty() {
        return None;
    }
    // La cadena del registro viene como «"C:\...\unins.exe" /param».
    let (exe, args) = split_command(key);
    unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            wide("open").as_ptr(),
            wide(&exe).as_ptr(),
            if args.is_empty() { std::ptr::null() } else { wide(&args).as_ptr() },
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
    Some("Se ha lanzado el desinstalador del programa.".to_string())
}

/// Parte una línea de comandos en ejecutable y argumentos.
fn split_command(cmd: &str) -> (String, String) {
    let cmd = cmd.trim();
    if let Some(rest) = cmd.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return (rest[..end].to_string(), rest[end + 1..].trim().to_string());
        }
    }
    // Sin comillas: el ejecutable acaba en «.exe».
    let lower = cmd.to_lowercase();
    if let Some(pos) = lower.find(".exe") {
        let (a, b) = cmd.split_at(pos + 4);
        return (a.to_string(), b.trim().to_string());
    }
    (cmd.to_string(), String::new())
}

fn run_hidden(exe: &str, args: &str) {
    unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            wide("open").as_ptr(),
            wide(exe).as_ptr(),
            wide(args).as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

// ============================================================ predeterminadas
#[repr(C)]
struct OpenAsInfo {
    file: *const u16,
    class: *const u16,
    flags: i32,
}

type FnAssocQueryString = unsafe extern "system" fn(u32, u32, *const u16, *const u16, *mut u16, *mut u32) -> i32;
type FnSHOpenWithDialog = unsafe extern "system" fn(HWND, *const OpenAsInfo) -> i32;

const ASSOCSTR_FRIENDLYAPPNAME: u32 = 4;
const ASSOCF_IS_PROTOCOL: u32 = 0x00001000;
const OAIF_ALLOW_REGISTRATION: i32 = 0x00000001;
const OAIF_REGISTER_EXT: i32 = 0x00000002;
const OAIF_EXEC: i32 = 0x00000004;
const OAIF_HIDE_REGISTRATION: i32 = 0x00000020;

/// Tipos que de verdad se cambian a mano.
const TIPOS: &[(&str, &str, bool)] = &[
    (".txt", "Archivos de texto", false),
    (".pdf", "Documentos PDF", false),
    (".htm", "Páginas web guardadas", false),
    (".jpg", "Imágenes JPEG", false),
    (".png", "Imágenes PNG", false),
    (".mp3", "Música MP3", false),
    (".mp4", "Vídeo MP4", false),
    (".zip", "Archivos comprimidos", false),
    ("http", "Enlaces web (navegador)", true),
    ("mailto", "Correo electrónico", true),
];

fn assoc_app(assoc: &str, protocol: bool) -> String {
    let Some(dll) = Dll::open("shlwapi.dll") else { return String::new() };
    unsafe {
        let Some(p) = dll.proc("AssocQueryStringW") else { return String::new() };
        let f: FnAssocQueryString = std::mem::transmute(p);
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let flags = if protocol { ASSOCF_IS_PROTOCOL } else { 0 };
        if f(flags, ASSOCSTR_FRIENDLYAPPNAME, wide(assoc).as_ptr(), std::ptr::null(),
             buf.as_mut_ptr(), &mut len) != 0 {
            return String::new();
        }
        from_utf16(&buf)
    }
}

fn predeterminadas() -> Vec<Entry> {
    TIPOS.iter().map(|(assoc, label, protocol)| {
        let app = assoc_app(assoc, *protocol);
        Entry::new(if *protocol { "\u{E774}" } else { "\u{E7C3}" },
                   format!("{} ({})", label, assoc),
                   if app.is_empty() { "Sin aplicación asignada".to_string() } else { app })
            .key(format!("{}{}", if *protocol { "p:" } else { "e:" }, assoc))
            .act(&[Action::Cambiar])
    }).collect()
}

fn predeterminadas_act(key: &str, action: Action, owner: HWND) -> Option<String> {
    if action != Action::Cambiar {
        return None;
    }
    let dll = Dll::open("shell32.dll")?;
    let f: FnSHOpenWithDialog = unsafe { std::mem::transmute(dll.proc("SHOpenWithDialog")?) };
    let protocol = key.starts_with("p:");
    let assoc = &key[2..];
    unsafe {
        if protocol {
            let class = wide(assoc);
            let info = OpenAsInfo {
                file: std::ptr::null(),
                class: class.as_ptr(),
                flags: OAIF_ALLOW_REGISTRATION | OAIF_REGISTER_EXT | OAIF_HIDE_REGISTRATION,
            };
            f(owner, &info);
        } else {
            // El cuadro necesita un archivo de ese tipo: se crea uno vacío
            // en la carpeta temporal y se borra después.
            let tmp = std::env::temp_dir().join(format!("oshell-tipo{}", assoc));
            let _ = std::fs::write(&tmp, b"");
            let file = wide(&tmp.to_string_lossy());
            let info = OpenAsInfo {
                file: file.as_ptr(),
                class: std::ptr::null(),
                flags: OAIF_ALLOW_REGISTRATION | OAIF_REGISTER_EXT | OAIF_EXEC,
            };
            f(owner, &info);
            let _ = std::fs::remove_file(&tmp);
        }
    }
    Some(format!("Aplicación de «{}»: {}.", assoc, {
        let app = assoc_app(assoc, protocol);
        if app.is_empty() { "sin asignar".to_string() } else { app }
    }))
}

// ============================================================ accesibilidad
const HCF_ON: u32 = 1;
const SKF_ON: u32 = 1;
const FKF_ON: u32 = 1;
const TKF_ON: u32 = 1;
const MKF_ON: u32 = 1;

#[repr(C)]
struct HighContrast {
    cb: u32,
    flags: u32,
    scheme: *mut u16,
}

#[repr(C)]
struct FilterKeys {
    cb: u32,
    wait: u32,
    delay: u32,
    repeat: u32,
    bounce: u32,
    flags: u32,
}

#[repr(C)]
struct MouseKeys {
    cb: u32,
    flags: u32,
    max_speed: u32,
    time_to_max: u32,
    ctrl_speed: u32,
    r1: u32,
    r2: u32,
}

fn high_contrast() -> bool {
    let mut hc = HighContrast {
        cb: std::mem::size_of::<HighContrast>() as u32,
        flags: 0,
        scheme: std::ptr::null_mut(),
    };
    unsafe {
        SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cb,
                              &mut hc as *mut HighContrast as *mut c_void, 0);
    }
    hc.flags & HCF_ON != 0
}

fn set_high_contrast(on: bool) -> bool {
    let mut hc = HighContrast {
        cb: std::mem::size_of::<HighContrast>() as u32,
        flags: if on { HCF_ON } else { 0 },
        scheme: std::ptr::null_mut(),
    };
    unsafe {
        SystemParametersInfoW(SPI_SETHIGHCONTRAST, hc.cb,
                              &mut hc as *mut HighContrast as *mut c_void, SPIF_APPLY) != 0
    }
}

fn filter_keys() -> bool {
    let mut fk: FilterKeys = unsafe { std::mem::zeroed() };
    fk.cb = std::mem::size_of::<FilterKeys>() as u32;
    unsafe {
        SystemParametersInfoW(SPI_GETFILTERKEYS, fk.cb, &mut fk as *mut FilterKeys as *mut c_void, 0);
    }
    fk.flags & FKF_ON != 0
}

fn set_filter_keys(on: bool) -> bool {
    let mut fk: FilterKeys = unsafe { std::mem::zeroed() };
    fk.cb = std::mem::size_of::<FilterKeys>() as u32;
    unsafe {
        SystemParametersInfoW(SPI_GETFILTERKEYS, fk.cb, &mut fk as *mut FilterKeys as *mut c_void, 0);
        fk.flags = if on { fk.flags | FKF_ON | 2 } else { fk.flags & !FKF_ON };
        SystemParametersInfoW(SPI_SETFILTERKEYS, fk.cb,
                              &mut fk as *mut FilterKeys as *mut c_void, SPIF_APPLY) != 0
    }
}

fn mouse_keys() -> bool {
    let mut mk: MouseKeys = unsafe { std::mem::zeroed() };
    mk.cb = std::mem::size_of::<MouseKeys>() as u32;
    unsafe {
        SystemParametersInfoW(SPI_GETMOUSEKEYS, mk.cb, &mut mk as *mut MouseKeys as *mut c_void, 0);
    }
    mk.flags & MKF_ON != 0
}

fn set_mouse_keys(on: bool) -> bool {
    let mut mk: MouseKeys = unsafe { std::mem::zeroed() };
    mk.cb = std::mem::size_of::<MouseKeys>() as u32;
    unsafe {
        SystemParametersInfoW(SPI_GETMOUSEKEYS, mk.cb, &mut mk as *mut MouseKeys as *mut c_void, 0);
        mk.flags = if on { mk.flags | MKF_ON | 2 } else { mk.flags & !MKF_ON };
        SystemParametersInfoW(SPI_SETMOUSEKEYS, mk.cb,
                              &mut mk as *mut MouseKeys as *mut c_void, SPIF_APPLY) != 0
    }
}

fn accesibilidad() -> Vec<Entry> {
    let sticky = flags2_get(SPI_GETSTICKYKEYS) & SKF_ON != 0;
    let toggle = flags2_get(SPI_GETTOGGLEKEYS) & TKF_ON != 0;
    let sounds = spi_get_bool(SPI_GETSHOWSOUNDS);
    vec![
        Entry::new("\u{E7F8}", "Contraste alto", si(high_contrast()))
            .key("hc").on(high_contrast()).act(&[Action::Alternar]),
        Entry::new("\u{E765}", "Teclas especiales (StickyKeys)", si(sticky))
            .key("sticky").on(sticky).act(&[Action::Alternar]),
        Entry::new("\u{E765}", "Teclas filtro (FilterKeys)", si(filter_keys()))
            .key("filter").on(filter_keys()).act(&[Action::Alternar]),
        Entry::new("\u{E765}", "Teclas de alternancia (ToggleKeys)", si(toggle))
            .key("toggle").on(toggle).act(&[Action::Alternar]),
        Entry::new("\u{E962}", "Mover el puntero con el teclado", si(mouse_keys()))
            .key("mousekeys").on(mouse_keys()).act(&[Action::Alternar]),
        Entry::new("\u{E7F6}", "Avisos sonoros también en pantalla", si(sounds))
            .key("showsounds").on(sounds).act(&[Action::Alternar]),
        Entry::new("\u{E8A3}", "Lupa", "Amplía una parte de la pantalla")
            .key("magnify").act(&[Action::Abrir]),
        Entry::new("\u{E995}", "Narrador", "Lee en voz alta lo que hay en pantalla")
            .key("narrator").act(&[Action::Abrir]),
        Entry::new("\u{E765}", "Teclado en pantalla", "Teclado táctil de Windows")
            .key("osk").act(&[Action::Abrir]),
    ]
}

fn accesibilidad_act(key: &str, action: Action) -> Option<String> {
    match (key, action) {
        ("hc", _) => {
            let on = !high_contrast();
            set_high_contrast(on);
            Some(format!("Contraste alto: {}.", si(on).to_lowercase()))
        }
        ("sticky", _) => {
            let cur = flags2_get(SPI_GETSTICKYKEYS);
            let on = cur & SKF_ON == 0;
            flags2_set(SPI_SETSTICKYKEYS, if on { cur | SKF_ON | 2 } else { cur & !SKF_ON });
            Some(format!("Teclas especiales: {}.", si(on).to_lowercase()))
        }
        ("filter", _) => {
            let on = !filter_keys();
            set_filter_keys(on);
            Some(format!("Teclas filtro: {}.", si(on).to_lowercase()))
        }
        ("toggle", _) => {
            let cur = flags2_get(SPI_GETTOGGLEKEYS);
            let on = cur & TKF_ON == 0;
            flags2_set(SPI_SETTOGGLEKEYS, if on { cur | TKF_ON | 2 } else { cur & !TKF_ON });
            Some(format!("Teclas de alternancia: {}.", si(on).to_lowercase()))
        }
        ("mousekeys", _) => {
            let on = !mouse_keys();
            set_mouse_keys(on);
            Some(format!("Puntero con el teclado: {}.", si(on).to_lowercase()))
        }
        ("showsounds", _) => {
            let on = !spi_get_bool(SPI_GETSHOWSOUNDS);
            spi_set_bool(SPI_SETSHOWSOUNDS, on);
            Some(format!("Avisos en pantalla: {}.", si(on).to_lowercase()))
        }
        ("magnify", _) => {
            ui::shell_open("magnify.exe");
            Some(String::new())
        }
        ("narrator", _) => {
            ui::shell_open("narrator.exe");
            Some(String::new())
        }
        ("osk", _) => {
            ui::shell_open("osk.exe");
            Some(String::new())
        }
        _ => None,
    }
}

// ============================================================ anclados
fn anclados() -> Vec<Entry> {
    let pins = crate::apps::taskbar_pins();
    let mut out: Vec<Entry> = pins.iter().enumerate().map(|(i, p)| {
        let existe = std::path::Path::new(p).exists();
        let mut acts = vec![Action::Quitar];
        if i > 0 {
            acts.insert(0, Action::Subir);
        }
        if i + 1 < pins.len() {
            acts.insert(if i > 0 { 1 } else { 0 }, Action::Bajar);
        }
        Entry::new("\u{E718}", crate::apps::pin_label(p),
                   if existe { p.clone() } else { format!("{} (no se encuentra)", p) })
            .key(p.clone())
            .on(existe)
            .act(&acts)
    }).collect();
    if out.is_empty() {
        out.push(Entry::new("\u{E718}", "Ningún icono anclado",
                            "Ancla programas aquí o con el botón derecho en la barra de tareas"));
    }
    out.push(Entry::new("\u{E710}", "Anclar un programa",
                        "Elige un ejecutable, un acceso directo o una carpeta")
        .key("add").act(&[Action::Examinar]));
    out
}

fn anclados_act(index: usize, key: &str, action: Action, owner: HWND) -> Option<String> {
    match action {
        Action::Examinar => {
            let path = browse_program(owner)?;
            let name = crate::apps::pin_label(&path);
            if crate::apps::taskbar_pin(&path) {
                ui::broadcast_config_changed();
                Some(format!("«{}» anclado a la barra de tareas.", name))
            } else {
                Some("Ya estaba anclado (o se ha llegado al tope de 24 iconos).".to_string())
            }
        }
        Action::Quitar => {
            crate::apps::taskbar_unpin(key);
            ui::broadcast_config_changed();
            Some(format!("«{}» desanclado.", crate::apps::pin_label(key)))
        }
        Action::Subir | Action::Bajar => {
            crate::apps::taskbar_move(index, if action == Action::Subir { -1 } else { 1 });
            ui::broadcast_config_changed();
            Some(String::new())
        }
        _ => None,
    }
}

/// Diálogo del sistema para elegir el programa que se ancla.
pub fn browse_program(owner: HWND) -> Option<String> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    unsafe {
        let mut file = [0u16; 520];
        let filter: Vec<u16> =
            "Programas y accesos directos\0*.exe;*.lnk;*.url;*.bat;*.cmd\0Todos\0*.*\0\0"
                .encode_utf16()
                .collect();
        let mut ofn: OPENFILENAMEW = std::mem::zeroed();
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = owner;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrFile = file.as_mut_ptr();
        ofn.nMaxFile = file.len() as u32;
        ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST;
        if GetOpenFileNameW(&mut ofn) == 0 {
            return None;
        }
        let end = file.iter().position(|&c| c == 0).unwrap_or(file.len());
        Some(String::from_utf16_lossy(&file[..end]))
    }
}

// ============================================================ bloqueo
const CSP: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\PersonalizationCSP";

fn bloqueo() -> Vec<Entry> {
    let actual = reg_read_sz(HKEY_LOCAL_MACHINE, CSP, "LockScreenImagePath").unwrap_or_default();
    vec![
        Entry::new("\u{E770}", "Imagen de la pantalla de bloqueo de Windows",
                   if actual.is_empty() { "La predeterminada de Windows".to_string() } else { actual })
            .key("img").act(&[Action::Examinar, Action::Quitar]),
        Entry::new("\u{E72E}", "Pantalla de bloqueo de OptimizedShell",
                   "Reloj, usuario y fondo propios")
            .key("demo").act(&[Action::Abrir]),
        Entry::new("\u{E72E}", "Bloquear ahora", "Bloquea la sesión")
            .key("lock").act(&[Action::Usar]),
    ]
}

fn bloqueo_act(key: &str, action: Action, owner: HWND) -> Option<String> {
    match (key, action) {
        ("img", Action::Examinar) => {
            let path = crate::display::browse_image(owner)?;
            let ok = reg_write_sz(HKEY_LOCAL_MACHINE, CSP, "LockScreenImagePath", &path)
                && reg_write_sz(HKEY_LOCAL_MACHINE, CSP, "LockScreenImageUrl", &path)
                && reg_write_dword(HKEY_LOCAL_MACHINE, CSP, "LockScreenImageStatus", 1);
            Some(if ok {
                "Imagen de la pantalla de bloqueo cambiada.".to_string()
            } else {
                "Hace falta ejecutar OptimizedShell como administrador para cambiarla.".to_string()
            })
        }
        ("img", Action::Quitar) => {
            let ok = reg_write_dword(HKEY_LOCAL_MACHINE, CSP, "LockScreenImageStatus", 0);
            Some(if ok {
                "Se ha vuelto a la imagen de Windows.".to_string()
            } else {
                "Hace falta permiso de administrador.".to_string()
            })
        }
        ("demo", _) => {
            crate::shell::launch_own("--lock --demo");
            Some(String::new())
        }
        ("lock", _) => {
            crate::shell::launch_own("--lock");
            Some(String::new())
        }
        _ => None,
    }
}

// ============================================================ herramientas
/// Consolas clásicas que no tienen equivalente propio en OptimizedShell.
const HERRAMIENTAS: &[(&str, &str, &str, &str)] = &[
    ("\u{E950}", "Administrador de dispositivos", "Controladores y hardware", "devmgmt.msc"),
    ("\u{EDA2}", "Administración de discos", "Particiones y volúmenes", "diskmgmt.msc"),
    ("\u{E9D9}", "Visor de eventos", "Registros del sistema", "eventvwr.msc"),
    ("\u{E90F}", "Programador de tareas", "Tareas automáticas", "taskschd.msc"),
    ("\u{E7BA}", "Directiva de grupo local", "Solo en ediciones Pro", "gpedit.msc"),
    ("\u{E8A5}", "Editor del registro", "Claves y valores", "regedit.exe"),
    ("\u{EA35}", "Liberador de espacio", "Archivos temporales", "cleanmgr.exe"),
    ("\u{E946}", "Información del sistema", "Resumen del hardware", "msinfo32.exe"),
    ("\u{E72E}", "Cuentas de usuario", "Usuarios locales y grupos", "netplwiz.exe"),
    ("\u{E72C}", "Restaurar sistema", "Puntos de restauración", "rstrui.exe"),
];

fn herramientas() -> Vec<Entry> {
    HERRAMIENTAS.iter().map(|(glyph, title, sub, cmd)| {
        Entry::new(glyph, *title, *sub).key(*cmd).act(&[Action::Abrir])
    }).collect()
}

fn herramientas_act(key: &str, action: Action) -> Option<String> {
    if action != Action::Abrir {
        return None;
    }
    ui::shell_open(key);
    Some(String::new())
}
