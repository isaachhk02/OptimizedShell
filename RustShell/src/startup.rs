// Programas de inicio de Windows (claves Run y carpetas Inicio).
// Port de CppShell/src/startup.h. Compartido por el administrador de tareas
// (pestaña Inicio) y, más adelante, por el shell (ejecutarlos al iniciar sesión).
// El estado habilitado/deshabilitado usa las mismas claves StartupApproved que
// Windows, por lo que es compatible con su administrador de tareas.
#![allow(dead_code)]

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, FILETIME};
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows_sys::Win32::UI::Shell::{SHGetFolderPathW, CSIDL_COMMON_STARTUP, CSIDL_STARTUP};
use windows_sys::Win32::Storage::FileSystem::{
    FindClose, FindFirstFileW, FindNextFileW, FILE_ATTRIBUTE_DIRECTORY, WIN32_FIND_DATAW,
};

use crate::ui::wide;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const APPROVED_BASE: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\";

#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub command: String,
    pub location: String,
    pub from_folder: bool,
    pub enabled: bool,
    pub approved_root: HKEY,
    pub approved_key: String,
    pub approved_value: String,
}

fn from_utf16(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn read_approved(root: HKEY, key: &str, value: &str) -> bool {
    let mut data = [0u8; 16];
    let mut size = data.len() as u32;
    let st = unsafe {
        RegGetValueW(
            root,
            wide(key).as_ptr(),
            wide(value).as_ptr(),
            RRF_RT_REG_BINARY,
            std::ptr::null_mut(),
            data.as_mut_ptr() as *mut c_void,
            &mut size,
        )
    };
    if st != ERROR_SUCCESS || size == 0 {
        return true; // sin entrada = habilitado
    }
    data[0] & 1 == 0
}

fn enum_registry(out: &mut Vec<Entry>, root: HKEY, view: u32, approved_sub: &str, location: &str) {
    unsafe {
        let mut k: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(root, wide(RUN_KEY).as_ptr(), 0, KEY_READ | view, &mut k) != ERROR_SUCCESS {
            return;
        }
        let mut i = 0u32;
        loop {
            let mut name = [0u16; 512];
            let mut name_len = name.len() as u32;
            let mut ty = 0u32;
            let mut data_len = 0u32;
            let st = RegEnumValueW(k, i, name.as_mut_ptr(), &mut name_len, std::ptr::null_mut(),
                                   &mut ty, std::ptr::null_mut(), &mut data_len);
            if st == ERROR_NO_MORE_ITEMS {
                break;
            }
            i += 1;
            if st != ERROR_SUCCESS || (ty != REG_SZ && ty != REG_EXPAND_SZ) || name_len == 0 {
                continue;
            }
            let mut data = vec![0u16; (data_len as usize) / 2 + 2];
            let mut cb = (data.len() * 2) as u32;
            name_len = name.len() as u32;
            if RegEnumValueW(k, i - 1, name.as_mut_ptr(), &mut name_len, std::ptr::null_mut(),
                             &mut ty, data.as_mut_ptr() as *mut u8, &mut cb) != ERROR_SUCCESS {
                continue;
            }
            let nm = from_utf16(&name);
            out.push(Entry {
                name: nm.clone(),
                command: from_utf16(&data),
                location: location.to_string(),
                from_folder: false,
                enabled: read_approved(root, &format!("{}{}", APPROVED_BASE, approved_sub), &nm),
                approved_root: root,
                approved_key: format!("{}{}", APPROVED_BASE, approved_sub),
                approved_value: nm,
            });
        }
        RegCloseKey(k);
    }
}

fn folder_path(csidl: i32) -> Option<String> {
    let mut buf = [0u16; 260];
    let ok = unsafe { SHGetFolderPathW(std::ptr::null_mut(), csidl, std::ptr::null_mut(), 0, buf.as_mut_ptr()) };
    if ok == 0 {
        Some(from_utf16(&buf))
    } else {
        None
    }
}

fn enum_folder(out: &mut Vec<Entry>, csidl: i32, approved_root: HKEY, location: &str) {
    let dir = match folder_path(csidl) {
        Some(d) => d,
        None => return,
    };
    unsafe {
        let mut fd: WIN32_FIND_DATAW = std::mem::zeroed();
        let pat = wide(&format!("{}\\*", dir));
        let h = FindFirstFileW(pat.as_ptr(), &mut fd);
        if h == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return;
        }
        loop {
            let fname = from_utf16(&fd.cFileName);
            if fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 && !fname.eq_ignore_ascii_case("desktop.ini") {
                let mut name = fname.clone();
                if name.to_lowercase().ends_with(".lnk") {
                    name.truncate(name.len() - 4);
                }
                out.push(Entry {
                    name,
                    command: format!("{}\\{}", dir, fname),
                    location: location.to_string(),
                    from_folder: true,
                    enabled: read_approved(approved_root, &format!("{}StartupFolder", APPROVED_BASE), &fname),
                    approved_root,
                    approved_key: format!("{}StartupFolder", APPROVED_BASE),
                    approved_value: fname,
                });
            }
            if FindNextFileW(h, &mut fd) == 0 {
                break;
            }
        }
        FindClose(h);
    }
}

pub fn enumerate() -> Vec<Entry> {
    let mut out = Vec::new();
    enum_registry(&mut out, HKEY_CURRENT_USER, 0, "Run", "Registro (usuario)");
    enum_registry(&mut out, HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY, "Run", "Registro (equipo)");
    enum_registry(&mut out, HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY, "Run32", "Registro (equipo, 32 bits)");
    enum_folder(&mut out, CSIDL_STARTUP as i32, HKEY_CURRENT_USER, "Carpeta Inicio (usuario)");
    enum_folder(&mut out, CSIDL_COMMON_STARTUP as i32, HKEY_LOCAL_MACHINE, "Carpeta Inicio (todos)");
    out
}

/// Devuelve 0 (ERROR_SUCCESS) o el código de error (5 = acceso denegado).
pub fn set_enabled(e: &Entry, enable: bool) -> u32 {
    let mut data = [0u8; 12];
    data[0] = if enable { 0x02 } else { 0x03 };
    if !enable {
        let mut ft: FILETIME = unsafe { std::mem::zeroed() };
        unsafe { GetSystemTimeAsFileTime(&mut ft) };
        let bytes = unsafe { std::slice::from_raw_parts(&ft as *const _ as *const u8, 8) };
        data[4..12].copy_from_slice(bytes);
    }
    unsafe {
        let mut k: HKEY = std::ptr::null_mut();
        let st = RegCreateKeyExW(e.approved_root, wide(&e.approved_key).as_ptr(), 0, std::ptr::null(),
                                 0, KEY_SET_VALUE, std::ptr::null(), &mut k, std::ptr::null_mut());
        if st != ERROR_SUCCESS {
            return st;
        }
        let st = RegSetValueExW(k, wide(&e.approved_value).as_ptr(), 0, REG_BINARY, data.as_ptr(), data.len() as u32);
        RegCloseKey(k);
        st
    }
}
