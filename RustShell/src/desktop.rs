// Escritorio: la ventana de fondo que dibuja el papel tapiz.
//
// Cuando OptimizedShell sustituye al Explorador no hay nadie que pinte el
// escritorio: la pantalla se queda negra. Esta ventana ocupa toda la pantalla,
// se queda siempre al fondo (HWND_BOTTOM, sin activarse nunca) y dibuja la
// imagen de fondo del usuario con el ajuste elegido, más un menú contextual
// con lo que suele hacer falta allí.
//
// La imagen sale de donde la deja Windows (HKCU\Control Panel\Desktop), así
// que cambiarla desde la Configuración de Windows o desde la galería de
// OptimizedShell se ve igual.
#![allow(non_snake_case, non_upper_case_globals)]

use std::cell::RefCell;

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::ui::config::{self, Id};
use crate::ui::gdip::{Align, Gfx, Image};
use crate::ui::menu;
use crate::ui::{self, theme, window};

pub const CLASS: &str = "OShellRustDesktop";

const DESKTOP_KEY: &str = "Control Panel\\Desktop";

thread_local! {
    /// Imagen cargada, junto a la ruta de la que salió (para no recargarla).
    static WALLPAPER: RefCell<Option<(String, Image)>> = RefCell::new(None);
}

/// Ruta del papel tapiz actual según Windows.
pub fn wallpaper_path() -> String {
    reg_str(DESKTOP_KEY, "Wallpaper")
}

fn reg_str(sub: &str, name: &str) -> String {
    unsafe {
        let mut k: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, ui::wide(sub).as_ptr(), 0, KEY_QUERY_VALUE, &mut k) != 0 {
            return String::new();
        }
        let mut buf = [0u16; 520];
        let mut len = (buf.len() * 2) as u32;
        let mut kind = 0u32;
        let ok = RegQueryValueExW(k, ui::wide(name).as_ptr(), std::ptr::null_mut(), &mut kind,
                                  buf.as_mut_ptr() as *mut u8, &mut len) == 0;
        RegCloseKey(k);
        if !ok {
            return String::new();
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n])
    }
}

/// Cambia el papel tapiz del sistema. Windows lo guarda en el registro y
/// avisa al resto del escritorio; aquí además se recarga la imagen.
pub fn set_wallpaper(path: &str) -> bool {
    unsafe {
        let ok = SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            ui::wide(path).as_ptr() as *mut _,
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        ) != 0;
        if ok {
            WALLPAPER.with(|c| *c.borrow_mut() = None);
        }
        ok
    }
}

/// Carga (y cachea) la imagen de fondo.
fn with_wallpaper<F: FnOnce(Option<&Image>)>(f: F) {
    let path = wallpaper_path();
    WALLPAPER.with(|c| {
        let mut cell = c.borrow_mut();
        let stale = match cell.as_ref() {
            Some((p, _)) => *p != path,
            None => true,
        };
        if stale {
            *cell = if path.is_empty() {
                None
            } else {
                Image::load(&path).map(|img| (path.clone(), img))
            };
        }
        f(cell.as_ref().map(|(_, img)| img));
    });
}

fn paint(hwnd: HWND) {
    let c = config::current();
    let fit = c.get(Id::EscritorioAjuste);
    let bg = theme::from_colorref({
        // El ajuste guarda 0xRRGGBB; el helper espera COLORREF (0xBBGGRR).
        let v = c.get(Id::EscritorioColor) as u32;
        (v >> 16 & 0xFF) | (v & 0xFF00) | ((v & 0xFF) << 16)
    });
    window::paint_buffered(hwnd, |g: &Gfx, w, h| {
        g.clear(bg);
        g.fill_rect(0, 0, w, h, bg);
        with_wallpaper(|img| {
            let Some(img) = img else {
                // Sin imagen: color liso y una marca discreta.
                let s = theme::style();
                g.text_in("OptimizedShell", 0.0, (h / 2 - ui::s(16)) as f32, w as f32, ui::s(32) as f32,
                          16.0, true, theme::with_alpha(s.text, 40), Align::Center, Align::Center, false);
                return;
            };
            match fit {
                1 => g.draw_image_contain(img, 0, 0, w, h),
                2 => g.draw_image(img, 0, 0, w, h),
                3 => {
                    // Mosaico.
                    let iw = img.width as i32;
                    let ih = img.height as i32;
                    if iw > 0 && ih > 0 {
                        let mut y = 0;
                        while y < h {
                            let mut x = 0;
                            while x < w {
                                g.draw_image(img, x, y, iw, ih);
                                x += iw;
                            }
                            y += ih;
                        }
                    }
                }
                4 => {
                    let iw = img.width as i32;
                    let ih = img.height as i32;
                    g.draw_image(img, (w - iw) / 2, (h - ih) / 2, iw, ih);
                }
                _ => g.draw_image_cover(img, 0, 0, w, h),
            }
        });
    });
}

/// Menú del botón derecho sobre el escritorio.
fn context_menu(hwnd: HWND, x: i32, y: i32) {
    let items = vec![
        menu::Item::new(1, "Actualizar").glyph("\u{E72C}"),
        menu::Item::sep(),
        menu::Item::new(2, "Explorador de archivos").glyph("\u{EC50}"),
        menu::Item::new(3, "Administrador de tareas").glyph("\u{E9D9}"),
        menu::Item::new(4, "Ejecutar...").glyph("\u{E756}"),
        menu::Item::sep(),
        menu::Item::new(5, "Personalizar el fondo").glyph("\u{E91B}"),
        menu::Item::new(6, "Configuración").glyph("\u{E713}"),
    ];
    match menu::popup(hwnd, x, y, items) {
        1 => {
            WALLPAPER.with(|c| *c.borrow_mut() = None);
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
        }
        2 => crate::shell::launch_own(&format!(
            "--explorer {}",
            std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into())
        )),
        3 => crate::shell::launch_own("--taskmgr"),
        4 => unsafe {
            ShellExecuteW(std::ptr::null_mut(), ui::wide("open").as_ptr(),
                          ui::wide("explorer.exe").as_ptr(),
                          ui::wide("shell:::{2559a1f3-21d7-11d4-bdaf-00c04f60b9f0}").as_ptr(),
                          std::ptr::null(), SW_SHOWNORMAL);
        },
        5 => crate::shell::launch_own("--settings fondo"),
        6 => crate::shell::launch_own("--settings"),
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == ui::config_changed_msg() {
        ui::reload_config();
        apply_placement(hwnd);
        InvalidateRect(hwnd, std::ptr::null(), 0);
        return 0;
    }
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
            // Cambió la resolución o los ajustes del sistema (puede ser el fondo).
            WALLPAPER.with(|c| *c.borrow_mut() = None);
            apply_placement(hwnd);
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_RBUTTONUP => {
            let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
            let mut pt = windows_sys::Win32::Foundation::POINT { x, y };
            windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut pt);
            context_menu(hwnd, pt.x, pt.y);
            0
        }
        WM_LBUTTONDOWN => {
            // Pulsar el escritorio cierra los menús abiertos del shell.
            0
        }
        // Nunca se pone delante: es el fondo.
        WM_WINDOWPOSCHANGING => {
            let p = &mut *(lp as *mut WINDOWPOS);
            p.hwndInsertAfter = HWND_BOTTOM;
            p.flags &= !SWP_NOZORDER;
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_DESTROY => {
            WALLPAPER.with(|c| *c.borrow_mut() = None);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Coloca la ventana cubriendo toda la pantalla virtual, al fondo del todo.
fn apply_placement(hwnd: HWND) {
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        SetWindowPos(hwnd, HWND_BOTTOM, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
}

/// ¿Está el Explorador de Windows haciendo de shell? Si lo está, él ya dibuja
/// el escritorio (y sus iconos), y taparlo sería un estorbo.
fn explorer_is_shell() -> bool {
    unsafe {
        !FindWindowW(ui::wide("Progman").as_ptr(), std::ptr::null()).is_null()
    }
}

/// Crea la ventana del escritorio. Devuelve null si está desactivada o si el
/// Explorador de Windows ya está dibujando el escritorio.
pub fn create(hinst: HINSTANCE) -> HWND {
    if !config::current().b(Id::EscritorioActivo) || explorer_is_shell() {
        return std::ptr::null_mut();
    }
    unsafe {
        let name = ui::wide(CLASS);
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: name.as_ptr(),
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            name.as_ptr(),
            ui::wide("Escritorio").as_ptr(),
            WS_POPUP,
            0, 0, 10, 10,
            std::ptr::null_mut(), std::ptr::null_mut(), hinst, std::ptr::null(),
        );
        if hwnd.is_null() {
            return hwnd;
        }
        apply_placement(hwnd);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        hwnd
    }
}

/// Minimiza todas las ventanas para enseñar el escritorio (Win+D).
pub fn show_desktop() {
    unsafe {
        // El shell real usa el mensaje 419 de la bandeja; aquí se hace a mano:
        // minimizar todas las ventanas de nivel superior visibles.
        EnumWindows(Some(minimize_proc), 0);
    }
}

unsafe extern "system" fn minimize_proc(hwnd: HWND, _lp: LPARAM) -> i32 {
    if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
        return 1;
    }
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex & WS_EX_TOOLWINDOW != 0 {
        return 1;
    }
    if GetWindow(hwnd, GW_OWNER) != std::ptr::null_mut() {
        return 1;
    }
    if GetWindowTextLengthW(hwnd) == 0 {
        return 1;
    }
    ShowWindow(hwnd, SW_MINIMIZE);
    1
}
