// Vigilancia de una carpeta: avisa cuando algo cambia dentro.
//
// `ReadDirectoryChangesW` bloquea hasta que hay un cambio, así que vive en un
// hilo aparte que, cuando pasa algo, le manda un mensaje a la ventana del
// explorador. La ventana se limita a releer la carpeta: no hace falta saber
// qué cambió exactamente, y así los cambios en ráfaga (descomprimir un zip,
// copiar cien archivos) cuestan una sola relectura.
//
// El hilo se para solo: `CancelIoEx` sobre el handle hace que la llamada
// bloqueante vuelva con error y el bucle termine.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::ui::wide;

/// Mensaje que recibe la ventana cuando la carpeta ha cambiado.
pub const WM_DIR_CHANGED: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_USER + 0x41;

/// Un vigilante en marcha. Al soltarlo se para el hilo.
pub struct Watcher {
    handle: Arc<HandleBox>,
    stop: Arc<AtomicBool>,
}

/// El handle del directorio, compartido con el hilo para poder cancelarlo.
struct HandleBox(HANDLE);

// El handle solo se usa para ReadDirectoryChangesW y CancelIoEx, que admiten
// llamadas desde varios hilos.
unsafe impl Send for HandleBox {}
unsafe impl Sync for HandleBox {}

impl Drop for HandleBox {
    fn drop(&mut self) {
        if self.0 != INVALID_HANDLE_VALUE && !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[link(name = "kernel32")]
extern "system" {
    fn CancelIoEx(handle: HANDLE, overlapped: *mut std::ffi::c_void) -> i32;
}

impl Watcher {
    /// Empieza a vigilar `path` y avisa a `hwnd` con `WM_DIR_CHANGED`.
    /// Devuelve `None` si la carpeta no se puede abrir (unidad extraíble que
    /// ya no está, permisos...): en ese caso el explorador sigue como antes,
    /// con el botón de actualizar.
    pub fn start(path: &str, hwnd: HWND) -> Option<Watcher> {
        unsafe {
            let h = CreateFileW(
                wide(path).as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            );
            if h == INVALID_HANDLE_VALUE || h.is_null() {
                return None;
            }
            let handle = Arc::new(HandleBox(h));
            let stop = Arc::new(AtomicBool::new(false));
            let th = handle.clone();
            let ts = stop.clone();
            let target = hwnd as isize;

            std::thread::spawn(move || {
                // Suficiente para muchos cambios seguidos; si se desborda,
                // Windows avisa igual y se relee la carpeta entera.
                let mut buf = vec![0u8; 16 * 1024];
                let filter = FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_DIR_NAME
                    | FILE_NOTIFY_CHANGE_ATTRIBUTES
                    | FILE_NOTIFY_CHANGE_SIZE
                    | FILE_NOTIFY_CHANGE_LAST_WRITE;
                // Este hilo no es interactivo: que lo programe en núcleos lentos.
                crate::ui::set_thread_eco(None);
                loop {
                    if ts.load(Ordering::Relaxed) {
                        break;
                    }
                    let mut returned = 0u32;
                    let ok = ReadDirectoryChangesW(
                        th.0,
                        buf.as_mut_ptr() as *mut _,
                        buf.len() as u32,
                        0, // sin subcarpetas: la lista solo enseña este nivel
                        filter,
                        &mut returned,
                        std::ptr::null_mut(),
                        None,
                    );
                    if ok == 0 || ts.load(Ordering::Relaxed) {
                        break;
                    }
                    // No importa qué cambió: la ventana relee la carpeta.
                    PostMessageW(target as HWND, WM_DIR_CHANGED, 0, 0);
                }
            });
            Some(Watcher { handle, stop })
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Desbloquea el hilo: la llamada pendiente vuelve con error.
        unsafe { CancelIoEx(self.handle.0, std::ptr::null_mut()) };
    }
}
