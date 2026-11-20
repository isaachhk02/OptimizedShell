// Portapapeles: texto y listas de archivos (CF_HDROP).
//
// El explorador copia y pega archivos con el mismo formato que usa el
// Explorador de Windows, así que lo copiado aquí se pega allí y al revés:
// CF_HDROP con la lista de rutas y, junto a él, el formato registrado
// «Preferred DropEffect» que distingue copiar de cortar.
#![allow(dead_code)]

use windows_sys::Win32::Foundation::{HANDLE, HWND};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::UI::Shell::{DragQueryFileW, HDROP};

use super::wide;

const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;

/// Efecto preferido: copiar o mover (cortar).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Copy,
    Move,
}

impl Effect {
    fn code(self) -> u32 {
        match self {
            Effect::Copy => 1, // DROPEFFECT_COPY
            Effect::Move => 2, // DROPEFFECT_MOVE
        }
    }
    fn from_code(c: u32) -> Effect {
        if c & 2 != 0 { Effect::Move } else { Effect::Copy }
    }
}

fn preferred_effect_format() -> u32 {
    unsafe { RegisterClipboardFormatW(wide("Preferred DropEffect").as_ptr()) }
}

/// Reserva memoria global y copia dentro los bytes dados.
unsafe fn global_from(bytes: &[u8]) -> HANDLE {
    let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
    if h.is_null() {
        return std::ptr::null_mut();
    }
    let p = GlobalLock(h);
    if p.is_null() {
        return std::ptr::null_mut();
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
    GlobalUnlock(h);
    h
}

/// Copia texto al portapapeles.
pub fn set_text(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return false;
        }
        EmptyClipboard();
        let utf16 = wide(text);
        let bytes = std::slice::from_raw_parts(utf16.as_ptr() as *const u8, utf16.len() * 2);
        let h = global_from(bytes);
        let ok = !h.is_null() && !SetClipboardData(CF_UNICODETEXT, h).is_null();
        CloseClipboard();
        ok
    }
}

/// Deja una lista de archivos en el portapapeles, para copiar o para cortar.
pub fn set_files(paths: &[String], effect: Effect) -> bool {
    if paths.is_empty() {
        return false;
    }
    // DROPFILES + la lista de rutas en UTF-16, separadas por NUL y con NUL doble al final.
    #[repr(C)]
    struct DropFiles {
        p_files: u32,
        pt: [i32; 2],
        f_nc: i32,
        f_wide: i32,
    }
    let mut names: Vec<u16> = Vec::new();
    for p in paths {
        names.extend(p.encode_utf16());
        names.push(0);
    }
    names.push(0);

    let head = DropFiles {
        p_files: std::mem::size_of::<DropFiles>() as u32,
        pt: [0, 0],
        f_nc: 0,
        f_wide: 1,
    };
    let mut bytes = Vec::with_capacity(std::mem::size_of::<DropFiles>() + names.len() * 2);
    unsafe {
        bytes.extend_from_slice(std::slice::from_raw_parts(
            &head as *const _ as *const u8, std::mem::size_of::<DropFiles>()));
        bytes.extend_from_slice(std::slice::from_raw_parts(
            names.as_ptr() as *const u8, names.len() * 2));

        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return false;
        }
        EmptyClipboard();
        let h = global_from(&bytes);
        let mut ok = !h.is_null() && !SetClipboardData(CF_HDROP, h).is_null();
        // Efecto preferido: sin esto, pegar siempre copia.
        let fx = effect.code();
        let fh = global_from(&fx.to_le_bytes());
        if !fh.is_null() {
            ok &= !SetClipboardData(preferred_effect_format(), fh).is_null();
        }
        CloseClipboard();
        ok
    }
}

/// Lista de archivos del portapapeles y si venían cortados.
pub fn get_files() -> Option<(Vec<String>, Effect)> {
    unsafe {
        if IsClipboardFormatAvailable(CF_HDROP) == 0 {
            return None;
        }
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let h = GetClipboardData(CF_HDROP);
        if h.is_null() {
            CloseClipboard();
            return None;
        }
        let drop = h as HDROP;
        let n = DragQueryFileW(drop, 0xFFFF_FFFF, std::ptr::null_mut(), 0);
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            let len = DragQueryFileW(drop, i, std::ptr::null_mut(), 0);
            if len == 0 {
                continue;
            }
            let mut buf = vec![0u16; len as usize + 1];
            DragQueryFileW(drop, i, buf.as_mut_ptr(), buf.len() as u32);
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            out.push(String::from_utf16_lossy(&buf[..end]));
        }
        // Efecto preferido (si no está, se copia).
        let mut effect = Effect::Copy;
        let fmt = preferred_effect_format();
        if IsClipboardFormatAvailable(fmt) != 0 {
            let fh = GetClipboardData(fmt);
            if !fh.is_null() {
                let p = GlobalLock(fh) as *const u32;
                if !p.is_null() {
                    effect = Effect::from_code(*p);
                    GlobalUnlock(fh);
                }
            }
        }
        CloseClipboard();
        if out.is_empty() { None } else { Some((out, effect)) }
    }
}

/// ¿Hay archivos en el portapapeles? (para habilitar «Pegar»).
pub fn has_files() -> bool {
    unsafe { IsClipboardFormatAvailable(CF_HDROP) != 0 }
}

/// Texto del portapapeles, si lo hay.
pub fn get_text(_owner: HWND) -> Option<String> {
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
            return None;
        }
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let h = GetClipboardData(CF_UNICODETEXT);
        let mut out = None;
        if !h.is_null() {
            let p = GlobalLock(h) as *const u16;
            if !p.is_null() {
                let mut len = 0usize;
                while *p.add(len) != 0 && len < 1 << 20 {
                    len += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
                GlobalUnlock(h);
            }
        }
        CloseClipboard();
        out
    }
}
