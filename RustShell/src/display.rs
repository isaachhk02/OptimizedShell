// Ajustes de pantalla: monitores, resoluciones y frecuencias de actualización.
//
// Se usa la API clásica de GDI (EnumDisplayDevicesW / EnumDisplaySettingsExW /
// ChangeDisplaySettingsExW), que es la que permite enumerar y aplicar modos sin
// pasar por WMI ni por la configuración de Windows.
//
// Al aplicar un modo se prueba primero (CDS_TEST): si el monitor no lo admite,
// no se toca nada y se dice por qué.
#![allow(dead_code)]

use windows_sys::Win32::Graphics::Gdi::*;

use crate::ui::wide;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub hz: u32,
    pub bpp: u32,
}

impl Mode {
    pub fn label(&self) -> String {
        format!("{} × {}", self.width, self.height)
    }
    pub fn full_label(&self) -> String {
        format!("{} × {} · {} Hz", self.width, self.height, self.hz)
    }
}

#[derive(Clone)]
pub struct Display {
    /// Nombre interno (\\.\DISPLAY1), que es lo que piden las APIs.
    pub device: String,
    /// Nombre del monitor para mostrar.
    pub name: String,
    pub primary: bool,
    pub current: Mode,
    /// Posición en el escritorio virtual.
    pub x: i32,
    pub y: i32,
    /// Orientación (0, 90, 180, 270 grados).
    pub rotation: u32,
    /// Modos admitidos, sin repetidos y ordenados de mayor a menor.
    pub modes: Vec<Mode>,
}

impl Display {
    /// Resoluciones distintas, de mayor a menor.
    pub fn resolutions(&self) -> Vec<(u32, u32)> {
        let mut v: Vec<(u32, u32)> = self.modes.iter().map(|m| (m.width, m.height)).collect();
        v.sort_unstable();
        v.dedup();
        v.reverse();
        v
    }

    /// Frecuencias posibles para una resolución.
    pub fn rates_for(&self, w: u32, h: u32) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .modes
            .iter()
            .filter(|m| m.width == w && m.height == h)
            .map(|m| m.hz)
            .collect();
        v.sort_unstable();
        v.dedup();
        v.reverse();
        v
    }
}

fn from_utf16(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Monitores conectados y activos.
pub fn displays() -> Vec<Display> {
    let mut out = Vec::new();
    unsafe {
        let mut i = 0u32;
        loop {
            let mut dd: DISPLAY_DEVICEW = std::mem::zeroed();
            dd.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
            if EnumDisplayDevicesW(std::ptr::null(), i, &mut dd, 0) == 0 {
                break;
            }
            i += 1;
            // Solo los que están conectados al escritorio.
            if dd.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0 {
                continue;
            }
            let device = from_utf16(&dd.DeviceName);
            let primary = dd.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0;

            // Nombre del monitor (el hijo del adaptador).
            let mut mon: DISPLAY_DEVICEW = std::mem::zeroed();
            mon.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
            let name = if EnumDisplayDevicesW(wide(&device).as_ptr(), 0, &mut mon, 0) != 0 {
                from_utf16(&mon.DeviceString)
            } else {
                from_utf16(&dd.DeviceString)
            };

            // Modo actual.
            let mut dm: DEVMODEW = std::mem::zeroed();
            dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
            let cur = if EnumDisplaySettingsExW(wide(&device).as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm, 0) != 0 {
                Mode {
                    width: dm.dmPelsWidth,
                    height: dm.dmPelsHeight,
                    hz: dm.dmDisplayFrequency,
                    bpp: dm.dmBitsPerPel,
                }
            } else {
                Mode { width: 0, height: 0, hz: 0, bpp: 32 }
            };
            let (x, y) = (dm.Anonymous1.Anonymous2.dmPosition.x, dm.Anonymous1.Anonymous2.dmPosition.y);
            let rotation = match dm.Anonymous1.Anonymous2.dmDisplayOrientation {
                1 => 90,
                2 => 180,
                3 => 270,
                _ => 0,
            };

            // Modos admitidos.
            let mut modes = Vec::new();
            let mut n = 0u32;
            loop {
                let mut m: DEVMODEW = std::mem::zeroed();
                m.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
                if EnumDisplaySettingsExW(wide(&device).as_ptr(), n, &mut m, 0) == 0 {
                    break;
                }
                n += 1;
                if n > 4096 {
                    break;
                }
                // Solo color verdadero: los modos de 8 y 16 bits no interesan.
                if m.dmBitsPerPel < 32 || m.dmPelsWidth < 640 {
                    continue;
                }
                modes.push(Mode {
                    width: m.dmPelsWidth,
                    height: m.dmPelsHeight,
                    hz: m.dmDisplayFrequency,
                    bpp: m.dmBitsPerPel,
                });
            }
            modes.sort_unstable();
            modes.dedup();
            modes.reverse();

            out.push(Display { device, name, primary, current: cur, x, y, rotation, modes });
        }
    }
    // El principal primero.
    out.sort_by_key(|d| !d.primary);
    out
}

fn change_result(code: i32) -> Result<(), String> {
    match code {
        0 => Ok(()), // DISP_CHANGE_SUCCESSFUL
        1 => Err("Hay que reiniciar para aplicar este modo.".into()),
        -1 => Err("El controlador de vídeo rechazó el cambio.".into()),
        -2 => Err("El monitor no admite ese modo.".into()),
        -3 => Err("No se pudo escribir la configuración en el registro.".into()),
        -4 => Err("Hay otro cambio de modo en curso.".into()),
        -5 => Err("Parámetros no válidos.".into()),
        _ => Err(format!("Error {} al cambiar el modo de pantalla.", code)),
    }
}

/// Aplica una resolución y frecuencia a un monitor. Primero se prueba el modo
/// y solo si Windows lo acepta se aplica de verdad.
pub fn set_mode(device: &str, width: u32, height: u32, hz: u32) -> Result<(), String> {
    unsafe {
        let mut dm: DEVMODEW = std::mem::zeroed();
        dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
        if EnumDisplaySettingsExW(wide(device).as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm, 0) == 0 {
            return Err("No se pudo leer el modo actual.".into());
        }
        dm.dmPelsWidth = width;
        dm.dmPelsHeight = height;
        if hz > 0 {
            dm.dmDisplayFrequency = hz;
        }
        dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | if hz > 0 { DM_DISPLAYFREQUENCY } else { 0 };

        let test = ChangeDisplaySettingsExW(wide(device).as_ptr(), &dm, std::ptr::null_mut(),
                                            CDS_TEST, std::ptr::null());
        change_result(test)?;
        let r = ChangeDisplaySettingsExW(wide(device).as_ptr(), &dm, std::ptr::null_mut(),
                                         CDS_UPDATEREGISTRY, std::ptr::null());
        change_result(r)
    }
}

/// Gira la pantalla. Al girar 90 o 270 grados hay que intercambiar ancho y alto.
pub fn set_rotation(device: &str, degrees: u32) -> Result<(), String> {
    unsafe {
        let mut dm: DEVMODEW = std::mem::zeroed();
        dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
        if EnumDisplaySettingsExW(wide(device).as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm, 0) == 0 {
            return Err("No se pudo leer el modo actual.".into());
        }
        let now = dm.Anonymous1.Anonymous2.dmDisplayOrientation;
        let want = match degrees {
            90 => 1,
            180 => 2,
            270 => 3,
            _ => 0,
        };
        // Si se pasa de horizontal a vertical (o al revés) se cambian las medidas.
        let swap = (now % 2) != (want % 2);
        if swap {
            std::mem::swap(&mut dm.dmPelsWidth, &mut dm.dmPelsHeight);
        }
        dm.Anonymous1.Anonymous2.dmDisplayOrientation = want;
        dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYORIENTATION;
        let test = ChangeDisplaySettingsExW(wide(device).as_ptr(), &dm, std::ptr::null_mut(),
                                            CDS_TEST, std::ptr::null());
        change_result(test)?;
        let r = ChangeDisplaySettingsExW(wide(device).as_ptr(), &dm, std::ptr::null_mut(),
                                         CDS_UPDATEREGISTRY, std::ptr::null());
        change_result(r)
    }
}

// ------------------------------------------------------------ fondos
/// Carpetas donde Windows y el usuario guardan imágenes de fondo.
fn wallpaper_dirs() -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    if let Ok(win) = std::env::var("WINDIR") {
        v.push(std::path::PathBuf::from(&win).join(r"Web\Wallpaper"));
        v.push(std::path::PathBuf::from(&win).join(r"Web\4K\Wallpaper"));
        v.push(std::path::PathBuf::from(&win).join(r"Web\Screen"));
    }
    if let Ok(up) = std::env::var("USERPROFILE") {
        v.push(std::path::PathBuf::from(&up).join("Pictures"));
        v.push(std::path::PathBuf::from(&up).join(r"Pictures\Wallpapers"));
    }
    v
}

fn is_image(p: &std::path::Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "bmp" | "gif"
    )
}

fn walk_images(dir: &std::path::Path, depth: u32, out: &mut Vec<String>, limit: usize) {
    if depth > 2 || out.len() >= limit {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if out.len() >= limit {
            return;
        }
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => walk_images(&p, depth + 1, out, limit),
            Ok(t) if t.is_file() && is_image(&p) => out.push(p.to_string_lossy().to_string()),
            _ => {}
        }
    }
}

/// Imágenes candidatas a fondo de escritorio. El fondo actual va primero.
pub fn wallpapers() -> Vec<String> {
    let mut out = Vec::new();
    let current = crate::desktop::wallpaper_path();
    if !current.is_empty() && std::path::Path::new(&current).exists() {
        out.push(current);
    }
    for d in wallpaper_dirs() {
        walk_images(&d, 0, &mut out, 60);
    }
    // Sin repetidos, conservando el orden.
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.to_lowercase()));
    out.truncate(60);
    out
}

/// Diálogo «Examinar...» del sistema para elegir una imagen.
pub fn browse_image(owner: windows_sys::Win32::Foundation::HWND) -> Option<String> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    unsafe {
        let mut file = [0u16; 520];
        // Pares de cadenas terminadas en NUL, con NUL doble al final.
        let filter: Vec<u16> = "Imágenes\0*.jpg;*.jpeg;*.png;*.bmp;*.gif\0Todos\0*.*\0\0"
            .encode_utf16()
            .collect();
        let mut ofn: OPENFILENAMEW = std::mem::zeroed();
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = owner;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrFile = file.as_mut_ptr();
        ofn.nMaxFile = file.len() as u32;
        ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST;
        if GetOpenFileNameW(&mut ofn) == 0 {
            return None;
        }
        let end = file.iter().position(|&c| c == 0).unwrap_or(file.len());
        Some(String::from_utf16_lossy(&file[..end]))
    }
}
