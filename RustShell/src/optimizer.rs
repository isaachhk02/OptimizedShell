// Optimizador ligero para Windows 11.
//
// Un único ejecutable, sin UI ni hilos, que recorta el conjunto de trabajo de
// todos los procesos accesibles (EmptyWorkingSet) y, con elevación, vacía
// también las listas de memoria en espera y modificada del sistema. Pensado
// para consumir solo unos pocos MB de RAM y poder lanzarse a mano o desde el
// Programador de tareas.
//
// Modos:
//   (sin argumentos)  optimiza (recorte + purgado si hay elevación)
//   --dry-run         solo cuenta procesos y memoria, no toca nada
//   --trim-only       solo recorte por proceso, sin purgar listas del sistema
//   --no-purge        no purga aunque haya elevación
//   --purge           fuerza el purgado (si hay elevación)
//   --log             escribe el informe en %APPDATA%\OptimizedShell\optimizer.log

use std::ffi::c_void;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
};

use crate::args;

#[link(name = "ntdll")]
extern "system" {
    fn NtSetSystemInformation(class: u32, info: *mut c_void, len: u32) -> i32;
}

#[link(name = "ntdll")]
extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
}

const SYSTEM_PROCESS_INFORMATION_CLASS: u32 = 5;
const SYSTEM_MEMORY_LIST_CLASS: u32 = 80;
const MEMORY_FLUSH_MODIFIED_LIST: u32 = 3;
const MEMORY_PURGE_STANDBY_LIST: u32 = 4;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

#[repr(C)]
struct Spi {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: UnicodeString,
    base_priority: i32,
    unique_process_id: isize,
    inherited_from_unique_process_id: isize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
}

/// Memoria física en uso ahora mismo, en bytes.
fn physical_in_use() -> u64 {
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut m) == 0 {
            return 0;
        }
        m.ullTotalPhys - m.ullAvailPhys
    }
}

/// Recorre el sistema una sola vez y devuelve los PIDs reales (sin el inactivo).
fn process_ids() -> Vec<u32> {
    let mut buf: Vec<u8> = vec![0u8; 256 * 1024];
    loop {
        let mut needed: u32 = 0;
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_PROCESS_INFORMATION_CLASS,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
                &mut needed,
            )
        };
        if status == 0xC000_0004u32 as i32 {
            buf.resize((needed as usize).max(buf.len() * 2), 0);
            continue;
        }
        if status < 0 {
            return Vec::new();
        }
        break;
    }

    let mut out = Vec::with_capacity(256);
    let mut offset = 0usize;
    loop {
        let p = unsafe { &*(buf.as_ptr().add(offset) as *const Spi) };
        let pid = p.unique_process_id as u32;
        if pid != 0 {
            out.push(pid);
        }
        if p.next_entry_offset == 0 {
            break;
        }
        offset += p.next_entry_offset as usize;
    }
    out
}

/// Recorta el conjunto de trabajo de un proceso. Devuelve true si se pudo.
fn trim_process(pid: u32) -> bool {
    if pid == 0 || pid == 4 {
        return false;
    }
    unsafe {
        let h = OpenProcess(PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let ok = EmptyWorkingSet(h) != 0;
        CloseHandle(h);
        ok
    }
}

/// Vacía las listas de memoria en espera y modificada. Necesita elevación.
fn purge_system_lists() -> bool {
    unsafe {
        let mut cmd: u32 = MEMORY_FLUSH_MODIFIED_LIST;
        let flushed = NtSetSystemInformation(
            SYSTEM_MEMORY_LIST_CLASS,
            &mut cmd as *mut _ as *mut c_void,
            4,
        ) >= 0;
        let mut cmd: u32 = MEMORY_PURGE_STANDBY_LIST;
        let purged = NtSetSystemInformation(
            SYSTEM_MEMORY_LIST_CLASS,
            &mut cmd as *mut _ as *mut c_void,
            4,
        ) >= 0;
        flushed || purged
    }
}

/// Recorta todos los procesos accesibles. Devuelve (total, recortados).
pub fn trim_all_processes() -> (usize, usize) {
    let pids = process_ids();
    let total = pids.len();
    let mut trimmed = 0usize;
    for pid in pids {
        if trim_process(pid) {
            trimmed += 1;
        }
    }
    (total, trimmed)
}

fn format_bytes(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", bytes, U[0])
    } else {
        format!("{:.1} {}", v, U[i])
    }
}

fn optimizer_log_path() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(std::env::var("APPDATA").unwrap_or_default());
    p.push("OptimizedShell");
    let _ = std::fs::create_dir_all(&p);
    p.push("optimizer.log");
    p
}

/// Punto de entrada del optimizador. Devuelve código de salida.
pub fn run() -> i32 {
    let dry = args::has_arg("--dry-run");
    let trim_only = args::has_arg("--trim-only");

    let before = physical_in_use();

    let (total, trimmed) = if dry {
        (process_ids().len(), 0usize)
    } else {
        trim_all_processes()
    };

    let mut purged = false;
    if !dry && !trim_only {
        let no_purge = args::has_arg("--no-purge");
        let force_purge = args::has_arg("--purge");
        if !no_purge && (force_purge || crate::services::is_elevated()) {
            if crate::services::is_elevated() {
                purged = purge_system_lists();
            }
        }
    }

    if !dry {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let after = physical_in_use();
    let freed = before.saturating_sub(after);
    let elevated = crate::services::is_elevated();

    let report = format!(
        "OptimizedOptimizer: procesos={}, recortados={}, liberados={} (antes {} MB, despues {} MB), listas_sistema={}, elevado={}",
        total,
        trimmed,
        format_bytes(freed),
        before / 1024 / 1024,
        after / 1024 / 1024,
        purged,
        elevated
    );
    println!("{}", report);

    if args::has_arg("--log") {
        let _ = std::fs::write(optimizer_log_path(), &report);
    }

    // Modo de medida: mantiene el proceso vivo unos segundos para poder
    // comprobar su consumo real de RAM (solo diagnostico).
    if args::has_arg("--stay") {
        let secs = args::value("--stay")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(10)
            .clamp(1, 60);
        std::thread::sleep(std::time::Duration::from_secs(secs));
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn se_enumeran_procesos() {
        assert!(process_ids().len() > 0, "no se enumeraron procesos");
    }

    #[test]
    fn memoria_fisica_reportada() {
        assert!(physical_in_use() > 0, "memoria física en uso es 0");
    }
}