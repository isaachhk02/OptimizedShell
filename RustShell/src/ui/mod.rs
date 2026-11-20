// ui - utilidades compartidas de UI (Win32 + GDI+) para OptimizedShell.
// Port de CppShell/src/ui.h. Ventanas con doble buffer, blur acrílico DWM,
// dibujo con GDI+ (recursos cacheados), tema compartido y utilidades para
// reducir el consumo del propio proceso.
#![allow(dead_code)]

pub mod config;
pub mod theme;
pub mod gdip;
pub mod window;
pub mod menu;
pub mod clipboard;

use std::sync::OnceLock;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, ReleaseDC, LOGPIXELSX};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, SetProcessInformation, SetThreadInformation,
    SetThreadPriority, ProcessPowerThrottling, ThreadPowerThrottling,
    PROCESS_POWER_THROTTLING_STATE, THREAD_POWER_THROTTLING_STATE,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    THREAD_POWER_THROTTLING_EXECUTION_SPEED, THREAD_POWER_THROTTLING_CURRENT_VERSION,
    THREAD_PRIORITY_BELOW_NORMAL,
};
use windows_sys::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows_sys::Win32::Foundation::HINSTANCE;

// ------------------------------------------------------------ estado global
// El C++ usa variables globales (proceso de una sola hilo de UI). Aquí se
// guarda el DPI y la instancia tras InitUi.
static G_DPI: OnceLock<i32> = OnceLock::new();
static G_HINST: OnceLock<isize> = OnceLock::new();

pub fn dpi() -> i32 {
    *G_DPI.get().unwrap_or(&96)
}

pub fn hinst() -> HINSTANCE {
    (*G_HINST.get().unwrap_or(&0)) as HINSTANCE
}

/// Escala un valor de 96 DPI al DPI actual y a la escala elegida por el
/// usuario (ajuste «Escala de la interfaz»). Equivale a ui::S.
pub fn s(v: i32) -> i32 {
    (v as i64 * dpi() as i64 * config::scale_pct() as i64 / (96 * 100)) as i32
}

/// Escala un valor en coma flotante (equivale a ui::SF).
pub fn sf(v: f32) -> f32 {
    v * dpi() as f32 / 96.0 * config::scale_pct() as f32 / 100.0
}

/// Alto de la barra de título de las ventanas con marco propio (configurable).
pub fn caption_h() -> i32 {
    s(config::current().get(config::Id::AltoTitulo))
}

/// Mensaje de difusión que avisa de un cambio de configuración.
pub fn config_changed_msg() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    *MSG.get_or_init(|| unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW(
            wide("OptimizedShell.ThemeChanged").as_ptr(),
        )
    })
}

/// Avisa a todas las ventanas de OptimizedShell de que la configuración cambió.
pub fn broadcast_config_changed() {
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SendNotifyMessageW(
            windows_sys::Win32::UI::WindowsAndMessaging::HWND_BROADCAST,
            config_changed_msg(),
            0,
            0,
        );
    }
}

/// Publica una configuración: la guarda, recalcula tema y fuentes y avisa.
pub fn apply_config(c: &config::Config, persist: bool) {
    if persist {
        config::save(c);
    }
    config::set_current(*c);
    theme::apply(c);
    gdip::invalidate_cache();
}

/// Relee la configuración del disco y recalcula tema y fuentes. Para usar al
/// recibir el mensaje de difusión.
pub fn reload_config() -> config::Config {
    let c = config::load();
    config::set_current(c);
    theme::apply(&c);
    gdip::invalidate_cache();
    c
}

/// Aplica los ajustes de rendimiento de la configuración al proceso actual.
pub fn apply_perf() {
    let c = config::current();
    set_process_eco(c.b(config::Id::Eco));
    if c.b(config::Id::LiberarMemoria) {
        trim_working_set();
    }
}

// ------------------------------------------------------------ cadenas
/// Convierte &str a un vector UTF-16 terminado en null, para las APIs *W.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ------------------------------------------------------------ inicialización
/// Inicializa DPI, GDI+, fuentes y el tema. Equivale a ui::InitUi.
pub fn init(inst: HINSTANCE) {
    let _ = G_HINST.set(inst as isize);
    // La conciencia de DPI (PerMonitorV2) la fija el manifiesto embebido.
    unsafe {
        let dc = GetDC(std::ptr::null_mut());
        let mut dpi = 96;
        if !dc.is_null() {
            dpi = GetDeviceCaps(dc, LOGPIXELSX as i32);
            ReleaseDC(std::ptr::null_mut(), dc);
        }
        if dpi <= 0 {
            dpi = 96;
        }
        let _ = G_DPI.set(dpi);
    }
    gdip::startup();
    let c = config::load();
    config::set_current(c);
    theme::apply(&c);
}

/// Libera GDI+ y recursos. Equivale a ui::ShutdownUi.
pub fn shutdown() {
    gdip::shutdown();
}

// ------------------------------------------------------------ eficiencia del proceso
/// EcoQoS: Windows programa el proceso en núcleos/frecuencias de bajo consumo.
pub fn set_process_eco(on: bool) {
    unsafe {
        let st = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: if on { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
        };
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &st as *const _ as *const _,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
    }
}

/// Marca un hilo como de bajo consumo (EcoQoS + prioridad por debajo de lo normal).
pub fn set_thread_eco(thread: Option<HANDLE>) {
    unsafe {
        let t = thread.unwrap_or_else(|| GetCurrentThread());
        let st = THREAD_POWER_THROTTLING_STATE {
            Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
        };
        SetThreadInformation(
            t,
            ThreadPowerThrottling,
            &st as *const _ as *const _,
            std::mem::size_of::<THREAD_POWER_THROTTLING_STATE>() as u32,
        );
        SetThreadPriority(t, THREAD_PRIORITY_BELOW_NORMAL);
    }
}

/// Devuelve al sistema las páginas que el proceso no usa ahora mismo.
pub fn trim_working_set() {
    unsafe {
        EmptyWorkingSet(GetCurrentProcess());
    }
}

/// Atajo: deja texto en el portapapeles (equivale a `clipboard::set_text`).
pub fn set_clipboard_text(text: &str) -> bool {
    clipboard::set_text(text)
}

// ------------------------------------------------------------ abrir con el shell
/// Abre algo con el shell de Windows: un protocolo (`ms-settings:display`),
/// un ejecutable (`control.exe`), una carpeta o un archivo.
///
/// La Configuración de Windows 11 (`ms-settings:`) es una aplicación
/// empaquetada (UWP), y abrirla tiene tres requisitos que no se cumplen solos
/// cuando OptimizedShell es el shell del usuario:
///
/// 1. La activación pasa por COM, así que el hilo que llama tiene que estar
///    inicializado. OptimizedShell dibuja con Win32 puro y no inicializa COM
///    en sus hilos de interfaz, de modo que `ShellExecuteW` fallaba en
///    silencio.
/// 2. `ShellExecuteW` resuelve los protocolos de las aplicaciones empaquetadas
///    apoyándose en el shell de la sesión: sin un `explorer.exe` en ejecución
///    no hay quien atienda la activación y la llamada no abre nada, que es
///    justo el caso que nos ocupa. Por eso aquí se activa la aplicación
///    directamente con `IApplicationActivationManager` (CLSID 45BA127D-...),
///    que vive en un servidor COM propio del sistema y funciona sin
///    Explorador.
/// 3. Si tampoco se puede, queda lanzar `SystemSettings.exe` de
///    `%WinDir%\ImmersiveControlPanel`, y como último recurso reintentar vía
///    `explorer.exe`.
///
/// Devuelve `true` si algo se ha llegado a lanzar.
pub fn shell_open(target: &str) -> bool {
    com_init();

    // Las aplicaciones empaquetadas (ms-settings:...) van por la vía de
    // activación, que no necesita Explorador.
    if let Some(aumid) = packaged_app_for(target) {
        if activate_app(aumid, target) {
            return true;
        }
    }

    if shell_execute(target, None) {
        return true;
    }

    // La Configuración, a pelo: el ejecutable de la propia aplicación.
    if target.to_ascii_lowercase().starts_with("ms-settings:") {
        let win = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        let exe = format!("{}\\ImmersiveControlPanel\\SystemSettings.exe",
                          win.trim_end_matches('\\'));
        if std::path::Path::new(&exe).exists() && shell_execute(&exe, None) {
            return true;
        }
    }

    // Último recurso: que lo abra explorer.exe (protocolos y shell:AppsFolder\...).
    shell_execute("explorer.exe", Some(target))
}

/// Inicializa COM en este hilo una sola vez (si ya lo estaba, se ignora).
pub fn com_init() {
    use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    thread_local! {
        static COM: () = {
            unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        };
    }
    COM.with(|_| ());
}

fn shell_execute(file: &str, params: Option<&str>) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let open = wide("open");
    let f = wide(file);
    let p = params.map(wide);
    let pp = p.as_ref().map(|v| v.as_ptr()).unwrap_or(std::ptr::null());
    // ShellExecuteW devuelve un "pseudo-HINSTANCE": <= 32 significa error.
    let r = unsafe {
        ShellExecuteW(std::ptr::null_mut(), open.as_ptr(), f.as_ptr(), pp,
                      std::ptr::null(), SW_SHOWNORMAL)
    };
    r as isize > 32
}

/// Identificador (AUMID) de la aplicación empaquetada que atiende el
/// protocolo, para los protocolos del sistema que nos interesan.
fn packaged_app_for(target: &str) -> Option<&'static str> {
    const SETTINGS: &str =
        "windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel";
    let t = target.to_ascii_lowercase();
    if t.starts_with("ms-settings:") || t.starts_with("ms-availablenetworks:") {
        Some(SETTINGS)
    } else {
        None
    }
}

// IApplicationActivationManager, llamado por el vtable a mano: windows-sys no
// trae envoltorios de COM y solo hace falta un método.
type ComFn = unsafe extern "system" fn();
const V_RELEASE: usize = 2;
const V_ACTIVATE_APPLICATION: usize = 3;

#[repr(C)]
#[derive(Clone, Copy)]
struct Guid {
    a: u32,
    b: u16,
    c: u16,
    d: [u8; 8],
}

// {45BA127D-10A8-46EA-8AB7-56EA9078943C} CLSID_ApplicationActivationManager
const CLSID_AAM: Guid = Guid {
    a: 0x45BA_127D, b: 0x10A8, c: 0x46EA,
    d: [0x8A, 0xB7, 0x56, 0xEA, 0x90, 0x78, 0x94, 0x3C],
};
// {2E941141-7F97-4756-BA1D-9DECDE894A3D} IID_IApplicationActivationManager
const IID_AAM: Guid = Guid {
    a: 0x2E94_1141, b: 0x7F97, c: 0x4756,
    d: [0xBA, 0x1D, 0x9D, 0xEC, 0xDE, 0x89, 0x4A, 0x3D],
};

/// Activa una aplicación empaquetada pasándole la URI completa como argumento,
/// que es lo que la Configuración espera para abrir una página concreta.
fn activate_app(aumid: &str, uri: &str) -> bool {
    use windows_sys::Win32::System::Com::{CoCreateInstance, CLSCTX_LOCAL_SERVER};
    unsafe {
        let mut obj: *mut std::ffi::c_void = std::ptr::null_mut();
        let hr = CoCreateInstance(&CLSID_AAM as *const _ as *const _, std::ptr::null_mut(),
                                  CLSCTX_LOCAL_SERVER, &IID_AAM as *const _ as *const _,
                                  &mut obj);
        if hr < 0 || obj.is_null() {
            return false;
        }
        let vt = *(obj as *mut *const *const ComFn);
        let app = wide(aumid);
        let arg = wide(uri);
        let mut pid: u32 = 0;

        // ActivateApplication(appUserModelId, arguments, options, &pid)
        let activate: unsafe extern "system" fn(
            *mut std::ffi::c_void, *const u16, *const u16, u32, *mut u32) -> i32 =
            std::mem::transmute(*vt.add(V_ACTIVATE_APPLICATION));
        let mut ok = activate(obj, app.as_ptr(), arg.as_ptr(), 0, &mut pid) >= 0;
        if !ok {
            // Sin argumentos se abre al menos la portada de la Configuración.
            ok = activate(obj, app.as_ptr(), std::ptr::null(), 0, &mut pid) >= 0;
        }

        let release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32 =
            std::mem::transmute(*vt.add(V_RELEASE));
        release(obj);
        ok
    }
}
