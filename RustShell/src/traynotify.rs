// Iconos de aplicación en el área de notificación (Shell_NotifyIcon).
//
// Las aplicaciones no hablan con el shell por una API pública: `Shell_NotifyIcon`
// busca la ventana de clase `Shell_TrayWnd` y le manda un WM_COPYDATA con
// `dwData == 1` y, dentro, la estructura NOTIFYICONDATA que pasó el programa.
// Aquí se crea esa ventana y se interpreta ese bloque.
//
// Importante: esto solo recibe iconos cuando OptimizedShell *es* el shell. Si
// el Explorador de Windows está en marcha, es su `Shell_TrayWnd` la que
// encuentran las aplicaciones y los iconos van allí.
//
// El contenido de NOTIFYICONDATA se lee por desplazamientos en vez de con una
// estructura fija porque su tamaño depende de la versión con la que se compiló
// cada programa (cbSize). Todo lo que viene de fuera se trata como datos: se
// comprueban los límites antes de leer y las ventanas muertas se descartan.
#![allow(non_snake_case, non_upper_case_globals, dead_code)]

use std::cell::RefCell;

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::ui;

/// Clase que buscan las aplicaciones. El nombre no es cosa nuestra.
pub const TRAY_CLASS: &str = "Shell_TrayWnd";
/// Clase hija que algunos programas esperan encontrar dentro.
pub const NOTIFY_CLASS: &str = "TrayNotifyWnd";

/// `dwData` del WM_COPYDATA que trae un icono.
const TRAY_COPYDATA_ID: usize = 1;
/// Firma del bloque (la pone Shell_NotifyIcon).
const TRAY_SIGNATURE: u32 = 0x3475_3423;

// Mensajes de Shell_NotifyIcon.
const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIM_DELETE: u32 = 2;
const NIM_SETFOCUS: u32 = 3;
const NIM_SETVERSION: u32 = 4;

// Banderas de NOTIFYICONDATA.
const NIF_MESSAGE: u32 = 0x01;
const NIF_ICON: u32 = 0x02;
const NIF_TIP: u32 = 0x04;
const NIF_STATE: u32 = 0x08;
const NIF_INFO: u32 = 0x10;

const NIS_HIDDEN: u32 = 0x01;

/// Un icono publicado por una aplicación.
#[derive(Clone)]
pub struct TrayIcon {
    /// Ventana del programa dueño (a la que se le reenvían los clics).
    pub owner: HWND,
    pub id: u32,
    pub icon: HICON,
    pub tip: String,
    /// Mensaje que el programa quiere recibir con los clics.
    pub callback: u32,
    /// Versión pedida con NIM_SETVERSION (0, 3 o 4): cambia cómo se le avisa.
    pub version: u32,
    pub hidden: bool,
}

thread_local! {
    static ICONS: RefCell<Vec<TrayIcon>> = RefCell::new(Vec::new());
}

/// Los iconos vivos, descartando los de ventanas que ya no existen.
pub fn icons() -> Vec<TrayIcon> {
    ICONS.with(|c| {
        let mut v = c.borrow_mut();
        v.retain(|i| unsafe { IsWindow(i.owner) != 0 });
        v.iter().filter(|i| !i.hidden && !i.icon.is_null()).cloned().collect()
    })
}

pub fn count() -> usize {
    icons().len()
}

// ------------------------------------------------------- lectura del bloque
/// Desplazamientos de NOTIFYICONDATAW tal y como la compila Windows de 64 bits.
/// Se usan solo tras comprobar que el bloque recibido es al menos así de
/// grande: lo que llega es de otro proceso y no hay que fiarse de su tamaño.
#[cfg(target_pointer_width = "64")]
mod off {
    pub const CB_SIZE: usize = 0;
    pub const HWND: usize = 8;
    pub const UID: usize = 16;
    pub const FLAGS: usize = 20;
    pub const CALLBACK: usize = 24;
    pub const HICON: usize = 32;
    pub const TIP: usize = 40;
    pub const STATE: usize = 296;
    pub const STATE_MASK: usize = 300;
    pub const VERSION: usize = 816;
    /// Tamaño mínimo para que existan los campos modernos (estado y versión).
    pub const MODERN: usize = 820;
}

#[cfg(target_pointer_width = "32")]
mod off {
    pub const CB_SIZE: usize = 0;
    pub const HWND: usize = 4;
    pub const UID: usize = 8;
    pub const FLAGS: usize = 12;
    pub const CALLBACK: usize = 16;
    pub const HICON: usize = 20;
    pub const TIP: usize = 24;
    pub const STATE: usize = 280;
    pub const STATE_MASK: usize = 284;
    pub const VERSION: usize = 800;
    pub const MODERN: usize = 804;
}

/// Lee un u32 del bloque si cabe.
fn read_u32(buf: &[u8], at: usize) -> Option<u32> {
    if at + 4 > buf.len() {
        return None;
    }
    Some(u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]))
}

/// Lee un puntero (HWND/HICON) del bloque si cabe.
fn read_ptr(buf: &[u8], at: usize) -> Option<usize> {
    let n = std::mem::size_of::<usize>();
    if at + n > buf.len() {
        return None;
    }
    let mut v = [0u8; std::mem::size_of::<usize>()];
    v.copy_from_slice(&buf[at..at + n]);
    Some(usize::from_le_bytes(v))
}

/// Lee una cadena UTF-16 de longitud máxima `max` caracteres.
fn read_wstr(buf: &[u8], at: usize, max: usize) -> String {
    let mut out = Vec::new();
    let mut i = at;
    for _ in 0..max {
        if i + 2 > buf.len() {
            break;
        }
        let c = u16::from_le_bytes([buf[i], buf[i + 1]]);
        if c == 0 {
            break;
        }
        out.push(c);
        i += 2;
    }
    String::from_utf16_lossy(&out)
}

/// Lo que viene dentro del WM_COPYDATA.
struct Incoming {
    message: u32,
    owner: HWND,
    id: u32,
    flags: u32,
    callback: u32,
    icon: HICON,
    tip: String,
    state: u32,
    state_mask: u32,
    version: u32,
}

fn parse(cds: &COPYDATASTRUCT) -> Option<Incoming> {
    if cds.dwData != TRAY_COPYDATA_ID || cds.lpData.is_null() {
        return None;
    }
    let len = cds.cbData as usize;
    // dwSignature + dwMessage + lo mínimo de NOTIFYICONDATA.
    if len < 8 + off::HICON + std::mem::size_of::<usize>() {
        return None;
    }
    let buf = unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, len) };
    if read_u32(buf, 0)? != TRAY_SIGNATURE {
        return None;
    }
    let message = read_u32(buf, 4)?;
    // La NOTIFYICONDATA empieza tras la firma y el mensaje.
    let nid = &buf[8..];
    let cb = read_u32(nid, off::CB_SIZE).unwrap_or(0) as usize;
    let flags = read_u32(nid, off::FLAGS).unwrap_or(0);
    let modern = cb >= off::MODERN && nid.len() >= off::MODERN;

    Some(Incoming {
        message,
        owner: read_ptr(nid, off::HWND)? as HWND,
        id: read_u32(nid, off::UID)?,
        flags,
        callback: read_u32(nid, off::CALLBACK).unwrap_or(0),
        icon: read_ptr(nid, off::HICON).unwrap_or(0) as HICON,
        // szTip son 128 caracteres en las versiones modernas y 64 en la vieja.
        tip: if flags & NIF_TIP != 0 {
            read_wstr(nid, off::TIP, if modern { 128 } else { 64 })
        } else {
            String::new()
        },
        state: if modern { read_u32(nid, off::STATE).unwrap_or(0) } else { 0 },
        state_mask: if modern { read_u32(nid, off::STATE_MASK).unwrap_or(0) } else { 0 },
        version: if modern { read_u32(nid, off::VERSION).unwrap_or(0) } else { 0 },
    })
}

/// Aplica un mensaje de Shell_NotifyIcon. Devuelve true si la lista cambió.
fn apply(inc: Incoming) -> bool {
    ICONS.with(|c| {
        let mut v = c.borrow_mut();
        let pos = v.iter().position(|i| i.owner == inc.owner && i.id == inc.id);
        match inc.message {
            NIM_ADD => {
                if pos.is_some() {
                    return false;
                }
                v.push(TrayIcon {
                    owner: inc.owner,
                    id: inc.id,
                    icon: if inc.flags & NIF_ICON != 0 { inc.icon } else { std::ptr::null_mut() },
                    tip: inc.tip,
                    callback: if inc.flags & NIF_MESSAGE != 0 { inc.callback } else { 0 },
                    version: 0,
                    hidden: inc.flags & NIF_STATE != 0
                        && inc.state_mask & NIS_HIDDEN != 0
                        && inc.state & NIS_HIDDEN != 0,
                });
                true
            }
            NIM_MODIFY => {
                let Some(i) = pos else { return false };
                let it = &mut v[i];
                if inc.flags & NIF_ICON != 0 {
                    it.icon = inc.icon;
                }
                if inc.flags & NIF_TIP != 0 {
                    it.tip = inc.tip;
                }
                if inc.flags & NIF_MESSAGE != 0 {
                    it.callback = inc.callback;
                }
                if inc.flags & NIF_STATE != 0 && inc.state_mask & NIS_HIDDEN != 0 {
                    it.hidden = inc.state & NIS_HIDDEN != 0;
                }
                true
            }
            NIM_DELETE => {
                if let Some(i) = pos {
                    v.remove(i);
                    return true;
                }
                false
            }
            NIM_SETVERSION => {
                if let Some(i) = pos {
                    v[i].version = inc.version;
                }
                false
            }
            // NIM_SETFOCUS y lo que no conocemos: no cambian la lista.
            _ => false,
        }
    })
}

/// Reenvía un clic al programa dueño del icono, con el formato que pidió.
pub fn forward_click(icon: &TrayIcon, mouse_msg: u32, screen_x: i32, screen_y: i32) {
    if icon.callback == 0 || unsafe { IsWindow(icon.owner) } == 0 {
        return;
    }
    unsafe {
        // Windows exige esto antes de enseñar un menú contextual, o el menú
        // del programa se queda abierto cuando pierde el foco.
        SetForegroundWindow(icon.owner);
        if icon.version >= 4 {
            // Versión 4: posición en wParam, mensaje e identificador en lParam.
            let wp = ((screen_y as u32 as usize) << 16) | (screen_x as u16 as usize);
            let lp = (((icon.id as u32) << 16) | (mouse_msg & 0xFFFF)) as LPARAM;
            PostMessageW(icon.owner, icon.callback, wp, lp);
        } else {
            PostMessageW(icon.owner, icon.callback, icon.id as WPARAM, mouse_msg as LPARAM);
        }
    }
}

// ---------------------------------------------------------------- ventana
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COPYDATA => {
            if lp != 0 {
                let cds = &*(lp as *const COPYDATASTRUCT);
                if let Some(inc) = parse(cds) {
                    if apply(inc) {
                        // La barra repinta su bandeja.
                        if let Some(bar) = find_taskbar() {
                            windows_sys::Win32::Graphics::Gdi::InvalidateRect(
                                bar, std::ptr::null(), 0);
                            PostMessageW(bar, WM_TRAY_CHANGED, 0, 0);
                        }
                    }
                    return 1;
                }
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Mensaje con el que esta ventana avisa a la barra de que algo cambió.
pub const WM_TRAY_CHANGED: u32 = WM_USER + 0x50;

fn find_taskbar() -> Option<HWND> {
    unsafe {
        let h = FindWindowW(ui::wide(crate::shell::CLASS).as_ptr(), std::ptr::null());
        if h.is_null() { None } else { Some(h) }
    }
}

/// Crea la ventana `Shell_TrayWnd` que recibe los iconos.
///
/// Si ya existe una (el Explorador está en marcha) se crea igualmente, pero
/// las aplicaciones seguirán mandando sus iconos a la del Explorador: los
/// iconos propios solo aparecen cuando OptimizedShell es el shell.
pub fn create(hinst: HINSTANCE) -> HWND {
    unsafe {
        let name = ui::wide(TRAY_CLASS);
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: name.as_ptr(),
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            name.as_ptr(),
            ui::wide("").as_ptr(),
            WS_POPUP,
            0, 0, 1, 1,
            std::ptr::null_mut(), std::ptr::null_mut(), hinst, std::ptr::null(),
        );
        if hwnd.is_null() {
            return hwnd;
        }
        // Ventana hija con el nombre que algunos programas buscan dentro.
        let child = ui::wide(NOTIFY_CLASS);
        let wc2 = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(DefWindowProcW),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: child.as_ptr(),
        };
        RegisterClassW(&wc2);
        CreateWindowExW(0, child.as_ptr(), ui::wide("").as_ptr(), WS_CHILD,
                        0, 0, 1, 1, hwnd, std::ptr::null_mut(), hinst, std::ptr::null());

        // Las aplicaciones que arrancaron antes vuelven a mandar sus iconos
        // cuando ven este mensaje de difusión.
        let taskbar_created = RegisterWindowMessageW(ui::wide("TaskbarCreated").as_ptr());
        if taskbar_created != 0 {
            SendNotifyMessageW(HWND_BROADCAST, taskbar_created, 0, 0);
        }
        hwnd
    }
}
