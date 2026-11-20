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
