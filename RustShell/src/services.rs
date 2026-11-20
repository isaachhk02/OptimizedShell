// Servicios de Windows para la pestaña «Servicios» del administrador.
//
// Se habla con el Administrador de control de servicios (SCM) por su API de
// advapi32: una sola llamada a EnumServicesStatusExW trae nombre, descripción,
// estado y PID de todos los servicios, y QueryServiceConfigW añade el tipo de
// inicio del seleccionado (no de todos: abrir un handle por servicio solo para
// pintar la lista sería justo el gasto que este proyecto evita).
//
// Iniciar, detener y reiniciar necesitan permisos de administrador; si el SCM
// los niega se devuelve el código de error para poder explicarlo.
#![allow(dead_code)]

use std::ffi::c_void;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Services::*;

use crate::ui::wide;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum State {
    Stopped,
    StartPending,
    StopPending,
    Running,
    ContinuePending,
    PausePending,
    Paused,
    Unknown,
}

impl State {
    fn from_code(c: u32) -> State {
        match c {
            1 => State::Stopped,
            2 => State::StartPending,
            3 => State::StopPending,
            4 => State::Running,
            5 => State::ContinuePending,
            6 => State::PausePending,
            7 => State::Paused,
            _ => State::Unknown,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            State::Stopped => "Detenido",
            State::StartPending => "Iniciando",
            State::StopPending => "Deteniéndose",
            State::Running => "En ejecución",
            State::ContinuePending => "Reanudando",
            State::PausePending => "Pausando",
            State::Paused => "En pausa",
            State::Unknown => "Desconocido",
        }
    }
    pub fn running(self) -> bool {
        matches!(self, State::Running | State::ContinuePending)
    }
}

/// Tipo de inicio configurado (lo que Windows llama «Tipo de inicio»).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StartType {
    Boot,
    System,
    Automatic,
    Manual,
    Disabled,
    Unknown,
}

impl StartType {
    fn from_code(c: u32) -> StartType {
        match c {
            0 => StartType::Boot,
            1 => StartType::System,
            2 => StartType::Automatic,
            3 => StartType::Manual,
            4 => StartType::Disabled,
            _ => StartType::Unknown,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            StartType::Boot => "Arranque",
            StartType::System => "Sistema",
            StartType::Automatic => "Automático",
            StartType::Manual => "Manual",
            StartType::Disabled => "Deshabilitado",
            StartType::Unknown => "",
        }
    }
    fn code(self) -> u32 {
        match self {
            StartType::Boot => 0,
            StartType::System => 1,
            StartType::Automatic => 2,
            StartType::Manual => 3,
            StartType::Disabled => 4,
            StartType::Unknown => 3,
        }
    }
}

#[derive(Clone)]
pub struct Service {
    /// Nombre corto (el que usa `sc`).
    pub name: String,
    /// Nombre para mostrar.
    pub display: String,
    pub state: State,
    /// PID del proceso que lo aloja (0 si no está en ejecución).
    pub pid: u32,
    pub start: StartType,
    /// El servicio comparte proceso con otros (svchost).
    pub shared: bool,
}

fn from_wide_ptr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe {
        let mut len = 0usize;
        while *p.add(len) != 0 && len < 1024 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    }
}

/// Todos los servicios del equipo, ordenados por nombre para mostrar.
pub fn enumerate() -> Vec<Service> {
    let mut out = Vec::new();
    unsafe {
        // Solo lectura: conectar y enumerar no necesita permisos de administrador,
        // pero sí hacen falta las dos cosas (conectar por sí solo no enumera).
        let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(),
                                 SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE);
        if scm.is_null() {
            return out;
        }
        let mut needed = 0u32;
        let mut count = 0u32;
        let mut resume = 0u32;
        // Primera llamada: solo para saber cuánto espacio hace falta.
        EnumServicesStatusExW(
            scm, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_STATE_ALL,
            std::ptr::null_mut(), 0, &mut needed, &mut count, &mut resume, std::ptr::null(),
        );
        if needed == 0 {
            CloseServiceHandle(scm);
            return out;
        }
        let mut buf = vec![0u8; needed as usize + 4096];
        let mut needed2 = 0u32;
        resume = 0;
        let ok = EnumServicesStatusExW(
            scm, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_STATE_ALL,
            buf.as_mut_ptr(), buf.len() as u32, &mut needed2, &mut count, &mut resume, std::ptr::null(),
        );
        if ok != 0 {
            let items = std::slice::from_raw_parts(
                buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW, count as usize);
            for it in items {
                let st = it.ServiceStatusProcess;
                out.push(Service {
                    name: from_wide_ptr(it.lpServiceName),
                    display: from_wide_ptr(it.lpDisplayName),
                    state: State::from_code(st.dwCurrentState),
                    pid: st.dwProcessId,
                    start: StartType::Unknown,
                    shared: st.dwServiceType & SERVICE_WIN32_SHARE_PROCESS != 0,
                });
            }
        }
        CloseServiceHandle(scm);
    }
    out.sort_by(|a, b| a.display.to_lowercase().cmp(&b.display.to_lowercase()));
    out
}

/// Tipo de inicio de un servicio concreto (una consulta, no toda la lista).
pub fn start_type(name: &str) -> StartType {
    unsafe {
        let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if scm.is_null() {
            return StartType::Unknown;
        }
        let svc = OpenServiceW(scm, wide(name).as_ptr(), SERVICE_QUERY_CONFIG);
        let mut out = StartType::Unknown;
        if !svc.is_null() {
            let mut needed = 0u32;
            QueryServiceConfigW(svc, std::ptr::null_mut(), 0, &mut needed);
            if needed > 0 {
                let mut buf = vec![0u8; needed as usize + 64];
                let mut needed2 = 0u32;
                if QueryServiceConfigW(svc, buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW,
                                       buf.len() as u32, &mut needed2) != 0 {
                    let cfg = &*(buf.as_ptr() as *const QUERY_SERVICE_CONFIGW);
                    out = StartType::from_code(cfg.dwStartType);
                }
            }
            CloseServiceHandle(svc);
        }
        CloseServiceHandle(scm);
        out
    }
}

/// Rellena el tipo de inicio de los servicios indicados (los visibles).
pub fn fill_start_types(list: &mut [Service]) {
    unsafe {
        let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if scm.is_null() {
            return;
        }
        for s in list.iter_mut() {
            if s.start != StartType::Unknown {
                continue;
            }
            let svc = OpenServiceW(scm, wide(&s.name).as_ptr(), SERVICE_QUERY_CONFIG);
            if svc.is_null() {
                continue;
            }
            let mut needed = 0u32;
            QueryServiceConfigW(svc, std::ptr::null_mut(), 0, &mut needed);
            if needed > 0 {
                let mut buf = vec![0u8; needed as usize + 64];
                let mut needed2 = 0u32;
                if QueryServiceConfigW(svc, buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW,
                                       buf.len() as u32, &mut needed2) != 0 {
                    let cfg = &*(buf.as_ptr() as *const QUERY_SERVICE_CONFIGW);
                    s.start = StartType::from_code(cfg.dwStartType);
                }
            }
            CloseServiceHandle(svc);
        }
        CloseServiceHandle(scm);
    }
}

/// Resultado de una acción sobre un servicio: `Ok` o el motivo del fallo.
pub type ActionResult = Result<(), String>;

fn last_error_text(code: u32) -> String {
    match code {
        5 => "Acceso denegado: ejecuta OptimizedShell como administrador.".to_string(),
        1051 => "Otros servicios dependen de este.".to_string(),
        1052 => "El servicio no admite esa acción.".to_string(),
        1053 => "El servicio no respondió a tiempo.".to_string(),
        1056 => "El servicio ya está en ejecución.".to_string(),
        1062 => "El servicio no estaba iniciado.".to_string(),
        _ => format!("Error {} del administrador de servicios.", code),
    }
}

fn with_service<F>(name: &str, access: u32, f: F) -> ActionResult
where
    F: FnOnce(*mut c_void) -> i32,
{
    unsafe {
        let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if scm.is_null() {
            return Err(last_error_text(
                windows_sys::Win32::Foundation::GetLastError()));
        }
        let svc = OpenServiceW(scm, wide(name).as_ptr(), access);
        if svc.is_null() {
            let e = windows_sys::Win32::Foundation::GetLastError();
            CloseServiceHandle(scm);
            return Err(last_error_text(e));
        }
        let ok = f(svc as *mut c_void);
        let e = if ok == 0 { windows_sys::Win32::Foundation::GetLastError() } else { 0 };
        CloseServiceHandle(svc);
        CloseServiceHandle(scm);
        if ok == 0 { Err(last_error_text(e)) } else { Ok(()) }
    }
}

pub fn start(name: &str) -> ActionResult {
    with_service(name, SERVICE_START, |svc| unsafe {
        StartServiceW(svc as SC_HANDLE, 0, std::ptr::null())
    })
}

pub fn stop(name: &str) -> ActionResult {
    with_service(name, SERVICE_STOP, |svc| unsafe {
        let mut st: SERVICE_STATUS = std::mem::zeroed();
        ControlService(svc as SC_HANDLE, SERVICE_CONTROL_STOP, &mut st)
    })
}

/// Detiene y vuelve a iniciar, esperando a que el SCM confirme la parada.
pub fn restart(name: &str) -> ActionResult {
    stop(name)?;
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if !query_state(name).running() {
            break;
        }
    }
    start(name)
}

/// Estado actual de un servicio (para refrescar tras una acción).
pub fn query_state(name: &str) -> State {
    unsafe {
        let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if scm.is_null() {
            return State::Unknown;
        }
        let svc = OpenServiceW(scm, wide(name).as_ptr(), SERVICE_QUERY_STATUS);
        let mut out = State::Unknown;
        if !svc.is_null() {
            let mut st: SERVICE_STATUS_PROCESS = std::mem::zeroed();
            let mut needed = 0u32;
            if QueryServiceStatusEx(svc, SC_STATUS_PROCESS_INFO, &mut st as *mut _ as *mut u8,
                                    std::mem::size_of::<SERVICE_STATUS_PROCESS>() as u32, &mut needed) != 0 {
                out = State::from_code(st.dwCurrentState);
            }
            CloseServiceHandle(svc);
        }
        CloseServiceHandle(scm);
        out
    }
}

/// Cambia el tipo de inicio (necesita permisos de administrador).
pub fn set_start_type(name: &str, t: StartType) -> ActionResult {
    with_service(name, SERVICE_CHANGE_CONFIG, |svc| unsafe {
        ChangeServiceConfigW(
            svc as SC_HANDLE,
            SERVICE_NO_CHANGE,
            t.code(),
            SERVICE_NO_CHANGE,
            std::ptr::null(), std::ptr::null(), std::ptr::null_mut(),
            std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(),
        )
    })
}

/// ¿El proceso actual tiene permisos de administrador? Sirve para avisar antes
/// de intentar una acción que el SCM va a rechazar.
pub fn is_elevated() -> bool {
    use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut el: TOKEN_ELEVATION = std::mem::zeroed();
        let mut len = 0u32;
        let ok = GetTokenInformation(token, TokenElevation, &mut el as *mut _ as *mut c_void,
                                     std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut len);
        CloseHandle(token);
        ok != 0 && el.TokenIsElevated != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El SCM debe contestar sin permisos de administrador: si esta consulta
    /// deja de funcionar, la pestaña Servicios se queda vacía.
    #[test]
    fn se_enumeran_servicios() {
        let list = enumerate();
        assert!(!list.is_empty(), "el SCM no devolvio ningun servicio");
        assert!(list.iter().any(|s| !s.name.is_empty() && !s.display.is_empty()),
                "los servicios vienen sin nombre");
    }
}
