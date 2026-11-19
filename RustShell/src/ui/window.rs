// Infraestructura de ventana: bucle de mensajes y pintado con doble buffer.
// El doble buffer evita parpadeo: se dibuja todo en un bitmap en memoria y se
// vuelca de una vez a la pantalla (equivale al DIB section de ui.h).
#![allow(dead_code)]

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    EndPaint, SelectObject, PAINTSTRUCT, SRCCOPY,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetClientRect, GetMessageW, TranslateMessage, MSG,
};

use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;

use super::gdip::Gfx;

/// Marco opaco con el tema actual: modo oscuro inmersivo + esquinas redondeadas.
/// Equivale a ui::ApplyFrame(FrameOpaque) del C++.
pub fn apply_frame_opaque(hwnd: HWND, dark: bool) {
    unsafe {
        let d: i32 = if dark { 1 } else { 0 };
        DwmSetWindowAttribute(hwnd, 20, &d as *const _ as *const _, 4); // USE_IMMERSIVE_DARK_MODE
        // Las esquinas del marco siguen el ajuste «Radio de las esquinas»:
        // 1 = DONOTROUND, 3 = ROUNDSMALL, 2 = ROUND.
        let r = super::config::current().get(super::config::Id::Radio);
        let pref: i32 = if r == 0 { 1 } else if r <= 6 { 3 } else { 2 };
        DwmSetWindowAttribute(hwnd, 33, &pref as *const _ as *const _, 4); // WINDOW_CORNER_PREFERENCE
    }
}

/// Reaplica el marco con la configuración actual (tras un cambio de tema).
pub fn refresh_frame(hwnd: HWND) {
    apply_frame_opaque(hwnd, super::config::current().dark());
}

/// Ejecuta un pintado con doble buffer en respuesta a WM_PAINT. El cierre recibe
/// un Gfx ya listo sobre el buffer en memoria y el tamaño del área cliente.
pub fn paint_buffered<F: FnOnce(&Gfx, i32, i32)>(hwnd: HWND, draw: F) {
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let w = rc.right - rc.left;
        let h = rc.bottom - rc.top;
        if w > 0 && h > 0 {
            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old = SelectObject(mem, bmp as _);
            {
                let g = Gfx::from_hdc(mem);
                draw(&g, w, h);
            }
            BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            DeleteObject(bmp as _);
            DeleteDC(mem);
        }
        EndPaint(hwnd, &ps);
    }
}

/// Bucle de mensajes estándar hasta WM_QUIT.
pub fn message_loop() -> i32 {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        msg.wParam as i32
    }
}
