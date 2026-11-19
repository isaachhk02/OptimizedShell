// Pantalla de bloqueo / inicio de sesión (--lock | --logon | --demo).
// Port de CppShell/src/logonui.cpp.
//
// Primera iteración: ventana a pantalla completa con fondo oscuro, reloj, tarjeta
// con avatar (inicial), nombre de usuario y campo de contraseña. Autentica con
// LogonUserW (--demo acepta cualquier contraseña). Al validar, cierra (desbloquea).
// La integración completa de sesión y el lanzamiento del shell como usuario se
// añadirán en iteraciones siguientes.
#![allow(non_snake_case, non_upper_case_globals)]

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, RECT, SYSTEMTIME, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::Security::LogonUserW;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_BACK, VK_RETURN};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::args;
use crate::ui::gdip::{Align, Gfx};
use crate::ui::{self, theme, window};

const LOGON32_LOGON_INTERACTIVE: u32 = 2;
const LOGON32_PROVIDER_DEFAULT: u32 = 0;

struct State {
    hwnd: HWND,
    username: String,
    password: String,
    message: String,
    demo: bool,
    card: RECT,
    pass_rc: RECT,
    login_rc: RECT,
}

fn er() -> RECT {
    RECT { left: 0, top: 0, right: 0, bottom: 0 }
}
fn in_rect(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}
fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}

fn initial(name: &str) -> String {
    name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default()
}

fn authenticate(st: &State) -> bool {
    if st.demo {
        return true;
    }
    unsafe {
        let (user, domain) = match st.username.split_once('\\') {
            Some((d, u)) => (u.to_string(), d.to_string()),
            None => (st.username.clone(), ".".to_string()),
        };
        let mut token: HANDLE = std::ptr::null_mut();
        let ok = LogonUserW(
            ui::wide(&user).as_ptr(),
            ui::wide(&domain).as_ptr(),
            ui::wide(&st.password).as_ptr(),
            LOGON32_LOGON_INTERACTIVE,
            LOGON32_PROVIDER_DEFAULT,
            &mut token,
        );
        if ok != 0 {
            if !token.is_null() {
                CloseHandle(token);
            }
            return true;
        }
    }
    false
}

fn layout(st: &mut State) {
    let mut rc = er();
    unsafe { GetClientRect(st.hwnd, &mut rc) };
    let (w, h) = (rc.right, rc.bottom);
    let cw = ui::s(360);
    let chh = ui::s(300);
    let cx = (w - cw) / 2;
    let cy = (h - chh) / 2 + ui::s(40);
    st.card = rect(cx, cy, cw, chh);
    st.pass_rc = rect(cx + ui::s(40), cy + ui::s(150), cw - ui::s(80), ui::s(40));
    st.login_rc = rect(cx + ui::s(40), cy + ui::s(206), cw - ui::s(80), ui::s(40));
}

fn paint(st: &State) {
    window::paint_buffered(st.hwnd, |g: &Gfx, w, h| {
        let s = theme::style();
        // fondo
        g.clear(theme::rgbc(12, 14, 20));
        g.fill_rect(0, 0, w, h, theme::argb(255, 12, 14, 20));

        // reloj
        let mut now: SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut now) };
        let clock = format!("{:02}:{:02}", now.wHour, now.wMinute);
        g.text_in(&clock, 0.0, (h / 2 - ui::s(220)) as f32, w as f32, ui::s(90) as f32, 64.0, false, theme::rgbc(240, 243, 248), Align::Center, Align::Center, false);
        let days = ["domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado"];
        let date = format!("{}, {} de {}", days.get(now.wDayOfWeek as usize).unwrap_or(&""), now.wDay, month(now.wMonth));
        g.text_in(&date, 0.0, (h / 2 - ui::s(130)) as f32, w as f32, ui::s(30) as f32, 18.0, false, theme::rgbc(200, 206, 216), Align::Center, Align::Center, false);

        // tarjeta
        let c = st.card;
        g.rounded(c.left as f32, c.top as f32, (c.right - c.left) as f32, (c.bottom - c.top) as f32, ui::sf(14.0), Some(theme::argb(235, 24, 27, 36)), Some((theme::argb(40, 255, 255, 255), 1.0)));
        // avatar
        let acx = ((c.left + c.right) / 2) as f32;
        let ay = (c.top + ui::s(58)) as f32;
        g.fill_circle(acx, ay, ui::s(40) as f32, s.accent_solid);
        g.text_in(&initial(&st.username), (acx - ui::s(40) as f32), (ay - ui::s(40) as f32), ui::s(80) as f32, ui::s(80) as f32, 34.0, true, s.on_accent, Align::Center, Align::Center, false);
        g.text_in(&st.username, c.left as f32, (c.top + ui::s(104)) as f32, (c.right - c.left) as f32, ui::s(30) as f32, 19.0, true, theme::rgbc(240, 243, 248), Align::Center, Align::Center, false);

        // campo contraseña
        let p = st.pass_rc;
        g.rounded(p.left as f32, p.top as f32, (p.right - p.left) as f32, (p.bottom - p.top) as f32, ui::sf(8.0), Some(theme::argb(255, 35, 40, 52)), Some((theme::argb(60, 255, 255, 255), 1.0)));
        if st.password.is_empty() {
            g.text_in("Contraseña", (p.left + ui::s(14)) as f32, p.top as f32, (p.right - p.left - ui::s(28)) as f32, (p.bottom - p.top) as f32, 13.0, false, theme::rgbc(120, 128, 142), Align::Near, Align::Center, false);
        } else {
            let dots: String = std::iter::repeat('\u{25CF}').take(st.password.chars().count()).collect();
            g.text_in(&dots, (p.left + ui::s(14)) as f32, p.top as f32, (p.right - p.left - ui::s(28)) as f32, (p.bottom - p.top) as f32, 13.0, false, theme::rgbc(232, 236, 244), Align::Near, Align::Center, false);
        }
        // botón iniciar
        let b = st.login_rc;
        g.rounded(b.left as f32, b.top as f32, (b.right - b.left) as f32, (b.bottom - b.top) as f32, ui::sf(8.0), Some(s.accent_solid), None);
        g.text_in("Iniciar sesión", b.left as f32, b.top as f32, (b.right - b.left) as f32, (b.bottom - b.top) as f32, 13.0, true, s.on_accent, Align::Center, Align::Center, false);

        if !st.message.is_empty() {
            g.text_in(&st.message, c.left as f32, (b.bottom + ui::s(8)) as f32, (c.right - c.left) as f32, ui::s(24) as f32, 12.0, false, s.danger, Align::Center, Align::Center, false);
        }
    });
}

fn month(m: u16) -> &'static str {
    const M: [&str; 12] = ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"];
    M.get((m as usize).saturating_sub(1)).copied().unwrap_or("")
}

fn try_login(st: &mut State) {
    if authenticate(st) {
        unsafe { PostMessageW(st.hwnd, WM_CLOSE, 0, 0) };
    } else {
        st.message = "Contraseña incorrecta. Inténtalo de nuevo.".to_string();
        st.password.clear();
        unsafe { InvalidateRect(st.hwnd, std::ptr::null(), 0) };
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let st_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    match msg {
        WM_SIZE => {
            if !st_ptr.is_null() {
                layout(&mut *st_ptr);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_TIMER => {
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            if !st_ptr.is_null() {
                paint(&*st_ptr);
            }
            0
        }
        WM_CHAR => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let ch = wp as u32;
                if ch == VK_BACK as u32 {
                    st.password.pop();
                } else if ch == 13 {
                    try_login(st);
                    return 0;
                } else if ch >= 0x20 {
                    if let Some(c) = char::from_u32(ch) {
                        st.password.push(c);
                        st.message.clear();
                    }
                }
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_KEYDOWN => {
            if wp as u32 == VK_RETURN as u32 && !st_ptr.is_null() {
                try_login(&mut *st_ptr);
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                if in_rect(&st.login_rc, x, y) {
                    try_login(st);
                }
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            if !st_ptr.is_null() {
                drop(Box::from_raw(st_ptr));
            }
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub fn run(lock: bool, demo: bool) -> i32 {
    let _ = lock;
    unsafe {
        let hinst = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null());
        ui::init(hinst);

        let username = std::env::var("USERNAME").unwrap_or_else(|_| "Usuario".to_string());

        let class_name = ui::wide("OShellRustLogon");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        RegisterClassW(&wc);

        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST,
            class_name.as_ptr(),
            ui::wide("Bloqueado").as_ptr(),
            WS_POPUP,
            0, 0, sw, sh,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return 1;
        }
        let st = Box::new(State {
            hwnd,
            username,
            password: String::new(),
            message: String::new(),
            demo,
            card: er(),
            pass_rc: er(),
            login_rc: er(),
        });
        let raw = Box::into_raw(st);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
        layout(&mut *raw);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        SetTimer(hwnd, 1, 1000, None); // reloj
        ui::set_process_eco(true);

        let code = window::message_loop();
        ui::shutdown();
        code
    }
}
