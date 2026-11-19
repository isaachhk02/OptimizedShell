// ui - utilidades compartidas de UI (Win32 + GDI+) para OptimizedShell.
// Port de CppShell/src/ui.h. Ventanas con doble buffer, blur acrílico DWM,
// dibujo con GDI+ (recursos cacheados), tema compartido y utilidades para
// reducir el consumo del propio proceso.
#![allow(dead_code)]

pub mod theme;
pub mod gdip;
pub mod window;

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

/// Escala un valor de 96 DPI al DPI actual (equivale a ui::S).
pub fn s(v: i32) -> i32 {
    (v as i64 * dpi() as i64 / 96) as i32
}

/// Escala un valor en coma flotante (equivale a ui::SF).
pub fn sf(v: f32) -> f32 {
    v * dpi() as f32 / 96.0
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
    theme::apply(theme::load());
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
