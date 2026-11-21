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
use windows_sys::Win32::Security::{
    GetLengthSid, GetTokenInformation, TokenUser, TOKEN_QUERY,
};
use windows_sys::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Services::{
    ChangeServiceConfig2W, CloseServiceHandle, ControlService, CreateServiceW, DeleteService, OpenSCManagerW, OpenServiceW,
    RegisterServiceCtrlHandlerW, SetServiceStatus, StartServiceCtrlDispatcherW, StartServiceW,
    SERVICE_STATUS, SERVICE_TABLE_ENTRYW,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
};

const PROCESS_QUERY_INFORMATION: u32 = 0x0400;

// --- servicio de Windows ---
const SERVICE_NAME: &str = "OptimizedOptimizer";
const SERVICE_DISPLAY: &str = "OptimizedOptimizer (optimizador de RAM)";
const SERVICE_WIN32_OWN_PROCESS: u32 = 0x00000010;
const SERVICE_AUTO_START: u32 = 0x00000002;
const SERVICE_ERROR_NORMAL: u32 = 0x00000001;
const SERVICE_START_PENDING: u32 = 0x00000002;
const SERVICE_RUNNING: u32 = 0x00000004;
const SERVICE_STOP_PENDING: u32 = 0x00000003;
const SERVICE_STOPPED: u32 = 0x00000001;
const SERVICE_ACCEPT_STOP: u32 = 0x00000001;
const SERVICE_CONTROL_STOP: u32 = 0x00000001;
const SC_MANAGER_CREATE_SERVICE: u32 = 0x00000002;
const SC_MANAGER_CONNECT: u32 = 0x00000001;
const SERVICE_ALL_ACCESS: u32 = 0x000F01FF;
const SERVICE_STOP: u32 = 0x00000020;
const SERVICE_QUERY_STATUS: u32 = 0x00000004;
const DELETE_SERVICE: u32 = 0x00010000;
const SERVICE_CONFIG_DESCRIPTION: u32 = 1;

static mut SERVICE_STATUS_HANDLE: windows_sys::Win32::System::Services::SERVICE_STATUS_HANDLE = std::ptr::null_mut();
static SERVICE_STOP_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

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

/// Obtiene el SID del usuario actual.
fn current_user_sid() -> Option<Vec<u8>> {
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let sid = token_user_sid(token);
        CloseHandle(token);
        sid
    }
}

/// Extrae los bytes del SID de un token.
fn token_user_sid(token: *mut c_void) -> Option<Vec<u8>> {
    unsafe {
        let mut size = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        if GetTokenInformation(token, TokenUser, buf.as_mut_ptr() as *mut c_void, size, &mut size) == 0 {
            return None;
        }
        let tu = &*(buf.as_ptr() as *const windows_sys::Win32::Security::TOKEN_USER);
        let sid = tu.User.Sid;
        let len = GetLengthSid(sid) as usize;
        if len == 0 {
            return None;
        }
        Some(std::slice::from_raw_parts(sid as *const u8, len).to_vec())
    }
}

/// ¿El proceso pertenece al usuario actual? (evita tocar procesos del sistema)
fn process_owned_by_current_user(pid: u32, current: &[u8]) -> bool {
    if pid == 0 || pid == 4 {
        return false;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let mut token = std::ptr::null_mut();
        let ok = OpenProcessToken(h, TOKEN_QUERY, &mut token);
        CloseHandle(h);
        if ok == 0 || token.is_null() {
            return false;
        }
        let sid = token_user_sid(token);
        CloseHandle(token);
        sid.map_or(false, |s| s == current)
    }
}

/// Recorta los procesos del usuario actual (menos agresivo, más compatible con AV).
/// Devuelve (total_procesos, recortados).
pub fn trim_all_processes() -> (usize, usize) {
    let pids = process_ids();
    let total = pids.len();
    let current = current_user_sid();
    let mut trimmed = 0usize;
    for pid in pids {
        if let Some(ref sid) = current {
            if !process_owned_by_current_user(pid, sid) {
                continue;
            }
        }
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

/// Convierte un &str a Vec<u16> terminado en null (para APIs wide).
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn control_handler(ctrl: u32) {
    if ctrl == SERVICE_CONTROL_STOP {
        SERVICE_STOP_REQUESTED.store(true, std::sync::atomic::Ordering::Relaxed);
        let status = SERVICE_STATUS {
            dwServiceType: SERVICE_WIN32_OWN_PROCESS,
            dwCurrentState: SERVICE_STOP_PENDING,
            dwControlsAccepted: 0,
            dwWin32ExitCode: 0,
            dwServiceSpecificExitCode: 0,
            dwCheckPoint: 0,
            dwWaitHint: 3000,
        };
        if SERVICE_STATUS_HANDLE != std::ptr::null_mut() {
            SetServiceStatus(SERVICE_STATUS_HANDLE, &status);
        }
    }
}

unsafe fn set_service_status(state: u32, controls: u32, wait_hint: u32) {
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: controls,
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: wait_hint,
    };
    if SERVICE_STATUS_HANDLE != std::ptr::null_mut() {
        SetServiceStatus(SERVICE_STATUS_HANDLE, &status);
    }
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
    let mut name = wide(SERVICE_NAME);
    SERVICE_STATUS_HANDLE = RegisterServiceCtrlHandlerW(name.as_ptr(), Some(control_handler));
    SERVICE_STOP_REQUESTED.store(false, std::sync::atomic::Ordering::Relaxed);

    set_service_status(SERVICE_START_PENDING, 0, 3000);
    set_service_status(SERVICE_RUNNING, SERVICE_ACCEPT_STOP, 0);

    while !SERVICE_STOP_REQUESTED.load(std::sync::atomic::Ordering::Relaxed) {
        // Optimiza los procesos del usuario.
        let _ = trim_all_processes();
        // Espera 5 minutos, comprobando la parada cada segundo.
        for _ in 0..300 {
            if SERVICE_STOP_REQUESTED.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    set_service_status(SERVICE_STOPPED, 0, 0);
}

/// Intenta ejecutarse como servicio de Windows. Devuelve true si el SCM
/// arrancó el proceso como servicio.
pub fn try_run_as_service() -> bool {
    unsafe {
        let mut name = wide(SERVICE_NAME);
        let mut table = [
            SERVICE_TABLE_ENTRYW {
                lpServiceName: name.as_mut_ptr(),
                lpServiceProc: Some(service_main),
            },
            SERVICE_TABLE_ENTRYW {
                lpServiceName: std::ptr::null_mut(),
                lpServiceProc: None,
            },
        ];
        StartServiceCtrlDispatcherW(table.as_mut_ptr()) != 0
    }
}

/// Instala el servicio (auto-inicio). Requiere elevación.
pub fn install_service() -> i32 {
    unsafe {
        let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CREATE_SERVICE);
        if scm.is_null() {
            return 1;
        }
        let path = match std::env::current_exe() {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => {
                // fallback
                String::new()
            }
        };
        let pathw = wide(&path);
        let name = wide(SERVICE_NAME);
        let display = wide(SERVICE_DISPLAY);
        let svc = CreateServiceW(
            scm,
            name.as_ptr(),
            display.as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            pathw.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        );
        if svc.is_null() {
            CloseServiceHandle(scm);
            return 1;
        }
        // Descripción.
        #[repr(C)]
        struct ServiceDescription {
            lpDescription: *mut u16,
        }
        let desc_text = wide("Optimiza periódicamente los procesos del usuario recortando su working set.");
        let mut desc = ServiceDescription {
            lpDescription: desc_text.as_ptr() as *mut u16,
        };
        let _ = ChangeServiceConfig2W(svc, SERVICE_CONFIG_DESCRIPTION, &mut desc as *mut _ as *mut _);
        // Intentar arrancar ahora.
        let _ = StartServiceW(svc, 0, std::ptr::null());
        CloseServiceHandle(svc);
        CloseServiceHandle(scm);
        0
    }
}

/// Desinstala el servicio. Requiere elevación.
pub fn uninstall_service() -> i32 {
    unsafe {
        let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if scm.is_null() {
            return 1;
        }
        let name = wide(SERVICE_NAME);
        let svc = OpenServiceW(scm, name.as_ptr(), SERVICE_STOP | SERVICE_QUERY_STATUS | DELETE_SERVICE);
        if svc.is_null() {
            CloseServiceHandle(scm);
            return 1;
        }
        let mut status: SERVICE_STATUS = std::mem::zeroed();
        let _ = ControlService(svc, SERVICE_CONTROL_STOP, &mut status);
        std::thread::sleep(std::time::Duration::from_millis(500));
        let ok = DeleteService(svc) != 0;
        CloseServiceHandle(svc);
        CloseServiceHandle(scm);
        if ok {
            0
        } else {
            1
        }
    }
}

/// Punto de entrada del optimizador. Devuelve código de salida.
pub fn run() -> i32 {
    if args::has_arg("--install-service") {
        return install_service();
    }
    if args::has_arg("--uninstall-service") {
        return uninstall_service();
    }
    let dry = args::has_arg("--dry-run");

    let before = physical_in_use();

    let (total, trimmed) = if dry {
        (process_ids().len(), 0usize)
    } else {
        trim_all_processes()
    };

    // Por defecto NO se purgan las listas del sistema (menos agresivo para AV).
    // Solo si se pide explícitamente con --purge y hay elevación.
    let mut purged = false;
    if !dry && args::has_arg("--purge") && crate::services::is_elevated() {
        purged = purge_system_lists();
    }

    if !dry {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let after = physical_in_use();
    let freed = before.saturating_sub(after);
    let elevated = crate::services::is_elevated();

    let report = format!(
        "OptimizedOptimizer: procesos={}, recortados_usuario={}, liberados={} (antes {} MB, despues {} MB), listas_sistema={}, elevado={}",
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
