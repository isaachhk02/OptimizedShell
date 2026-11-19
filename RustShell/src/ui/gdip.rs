// GDI+ : inicialización, envoltura RAII de Graphics, caché de fuentes y
// helpers de dibujo. Port de las utilidades de dibujo de CppShell/src/ui.h.
// GDI+ se usa a través de su API plana (Gdip*), por eso se encapsula aquí.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use windows_sys::Win32::Graphics::Gdi::HDC;
use windows_sys::Win32::Graphics::GdiPlus::*;

use super::theme::Color;
use super::{s, sf, wide};

static TOKEN: OnceLock<usize> = OnceLock::new();

/// Inicializa GDI+. Equivale a GdiplusStartup.
pub fn startup() {
    unsafe {
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            DebugEventCallback: 0,
            SuppressBackgroundThread: 0,
            SuppressExternalCodecs: 0,
        };
        let mut token: usize = 0;
        GdiplusStartup(&mut token, &input, std::ptr::null_mut());
        let _ = TOKEN.set(token);
    }
}

pub fn shutdown() {
    if let Some(&token) = TOKEN.get() {
        if token != 0 {
            unsafe { GdiplusShutdown(token) };
        }
    }
}

// ------------------------------------------------------------ alineación de texto
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Align {
    Near = 0,
    Center = 1,
    Far = 2,
}

// Caché de recursos GDI+ por hilo (los punteros Gp* no son Send; la UI vive en
// un solo hilo). Fuentes por (tamaño 1/4 px, negrita, icono) y formatos por
// (alineación horizontal, vertical).
thread_local! {
    static FONTS: RefCell<HashMap<(u32, bool, bool), *mut GpFont>> = RefCell::new(HashMap::new());
    static UI_FAMILY: RefCell<*mut GpFontFamily> = RefCell::new(std::ptr::null_mut());
    static ICON_FAMILY: RefCell<*mut GpFontFamily> = RefCell::new(std::ptr::null_mut());
    static FORMATS: RefCell<HashMap<(u8, u8), *mut GpStringFormat>> = RefCell::new(HashMap::new());
}

fn make_family(cell: &'static std::thread::LocalKey<RefCell<*mut GpFontFamily>>, name: &str) -> *mut GpFontFamily {
    cell.with(|f| {
        let mut p = f.borrow_mut();
        if p.is_null() {
            let n = wide(name);
            let mut fam: *mut GpFontFamily = std::ptr::null_mut();
            unsafe { GdipCreateFontFamilyFromName(n.as_ptr(), std::ptr::null_mut(), &mut fam) };
            *p = fam;
        }
        *p
    })
}

fn format(h: Align, v: Align) -> *mut GpStringFormat {
    FORMATS.with(|c| {
        let mut map = c.borrow_mut();
        let key = (h as u8, v as u8);
        if let Some(&f) = map.get(&key) {
            return f;
        }
        let mut fmt: *mut GpStringFormat = std::ptr::null_mut();
        unsafe {
            GdipCreateStringFormat(0, 0, &mut fmt);
            GdipSetStringFormatAlign(fmt, h as i32);
            GdipSetStringFormatLineAlign(fmt, v as i32);
            // NoWrap (0x1000) | LineLimit (0x2000): texto de una línea, sin ajuste.
            GdipSetStringFormatFlags(fmt, 0x1000 | 0x2000);
        }
        map.insert(key, fmt);
        fmt
    })
}

/// Devuelve (y cachea) una fuente al tamaño lógico dado (96 DPI).
/// `icon` usa Segoe MDL2 Assets (glifos de la interfaz).
fn font(size_pt: f32, bold: bool, icon: bool) -> *mut GpFont {
    let px = sf(size_pt);
    let key = ((px * 4.0) as u32, bold, icon);
    FONTS.with(|c| {
        let mut map = c.borrow_mut();
        if let Some(&f) = map.get(&key) {
            return f;
        }
        let fam = if icon {
            make_family(&ICON_FAMILY, "Segoe MDL2 Assets")
        } else {
            make_family(&UI_FAMILY, "Segoe UI")
        };
        let mut f: *mut GpFont = std::ptr::null_mut();
        let style = if bold { 1 } else { 0 };
        unsafe { GdipCreateFont(fam, px, style, 2 /*UnitPixel*/, &mut f) };
        map.insert(key, f);
        f
    })
}

#[inline]
fn argb_u32(c: Color) -> u32 {
    c // GDI+ ARGB (0xAARRGGBB) == nuestro Color
}

/// Envoltura RAII de un GpGraphics creado desde un HDC.
pub struct Gfx {
    g: *mut GpGraphics,
}

impl Gfx {
    pub fn from_hdc(hdc: HDC) -> Gfx {
        let mut g: *mut GpGraphics = std::ptr::null_mut();
        unsafe {
            GdipCreateFromHDC(hdc, &mut g);
            GdipSetSmoothingMode(g, 4 /*AntiAlias*/);
            GdipSetTextRenderingHint(g, 5 /*ClearTypeGridFit*/);
        }
        Gfx { g }
    }

    pub fn raw(&self) -> *mut GpGraphics {
        self.g
    }

    pub fn clear(&self, c: Color) {
        unsafe { GdipGraphicsClear(self.g, argb_u32(c)) };
    }

    pub fn fill_rect(&self, x: i32, y: i32, w: i32, h: i32, c: Color) {
        unsafe {
            let mut br: *mut GpSolidFill = std::ptr::null_mut();
            GdipCreateSolidFill(argb_u32(c), &mut br);
            GdipFillRectangleI(self.g, br as *mut GpBrush, x, y, w, h);
            GdipDeleteBrush(br as *mut GpBrush);
        }
    }

    pub fn fill_rect_f(&self, x: f32, y: f32, w: f32, h: f32, c: Color) {
        unsafe {
            let mut br: *mut GpSolidFill = std::ptr::null_mut();
            GdipCreateSolidFill(argb_u32(c), &mut br);
            GdipFillRectangle(self.g, br as *mut GpBrush, x, y, w, h);
            GdipDeleteBrush(br as *mut GpBrush);
        }
    }

    /// Línea horizontal fina de 1px.
    pub fn hline(&self, x1: i32, x2: i32, y: i32, c: Color) {
        unsafe {
            let mut pen: *mut GpPen = std::ptr::null_mut();
            GdipCreatePen1(argb_u32(c), 1.0, 2 /*UnitPixel*/, &mut pen);
            GdipDrawLineI(self.g, pen, x1, y, x2, y);
            GdipDeletePen(pen);
        }
    }

    /// Línea entre dos puntos con grosor dado.
    pub fn line(&self, x1: f32, y1: f32, x2: f32, y2: f32, c: Color, width: f32) {
        unsafe {
            let mut pen: *mut GpPen = std::ptr::null_mut();
            GdipCreatePen1(argb_u32(c), width, 2, &mut pen);
            GdipDrawLine(self.g, pen, x1, y1, x2, y2);
            GdipDeletePen(pen);
        }
    }

    /// Circunferencia (solo borde).
    pub fn stroke_circle(&self, cx: f32, cy: f32, r: f32, c: Color, width: f32) {
        unsafe {
            let mut pen: *mut GpPen = std::ptr::null_mut();
            GdipCreatePen1(argb_u32(c), width, 2, &mut pen);
            GdipDrawEllipse(self.g, pen, cx - r, cy - r, r * 2.0, r * 2.0);
            GdipDeletePen(pen);
        }
    }

    /// Marca de verificación (dos trazos) centrada en (cx,cy).
    pub fn check(&self, cx: f32, cy: f32, size: f32, c: Color, width: f32) {
        self.line(cx - size * 0.5, cy, cx - size * 0.1, cy + size * 0.4, c, width);
        self.line(cx - size * 0.1, cy + size * 0.4, cx + size * 0.5, cy - size * 0.4, c, width);
    }

    /// Interruptor estilo Windows.
    pub fn switch(&self, x: f32, y: f32, w: f32, h: f32, on: bool, on_color: Color, off_color: Color, knob: Color) {
        self.rounded(x, y, w, h, h / 2.0, Some(if on { on_color } else { off_color }), None);
        let kr = (h - super::sf(6.0)) / 2.0;
        let kx = if on { x + w - kr - super::sf(3.0) } else { x + kr + super::sf(3.0) };
        self.fill_circle(kx, y + h / 2.0, kr, knob);
    }

    pub fn fill_circle(&self, cx: f32, cy: f32, r: f32, c: Color) {
        unsafe {
            let mut br: *mut GpSolidFill = std::ptr::null_mut();
            GdipCreateSolidFill(argb_u32(c), &mut br);
            GdipFillEllipse(self.g, br as *mut GpBrush, cx - r, cy - r, r * 2.0, r * 2.0);
            GdipDeleteBrush(br as *mut GpBrush);
        }
    }

    /// Construye un camino de rectángulo redondeado.
    unsafe fn rounded_path(&self, x: f32, y: f32, w: f32, h: f32, radius: f32) -> *mut GpPath {
        let mut path: *mut GpPath = std::ptr::null_mut();
        GdipCreatePath(0 /*FillModeAlternate*/, &mut path);
        let r = radius.min(w / 2.0).min(h / 2.0).max(0.0);
        if r <= 0.5 {
            // Rectángulo recto.
            GdipAddPathArc(path, x, y, 0.01, 0.01, 180.0, 90.0);
            GdipAddPathArc(path, x + w, y, 0.01, 0.01, 270.0, 90.0);
            GdipAddPathArc(path, x + w, y + h, 0.01, 0.01, 0.0, 90.0);
            GdipAddPathArc(path, x, y + h, 0.01, 0.01, 90.0, 90.0);
        } else {
            let d = r * 2.0;
            GdipAddPathArc(path, x, y, d, d, 180.0, 90.0);
            GdipAddPathArc(path, x + w - d, y, d, d, 270.0, 90.0);
            GdipAddPathArc(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
            GdipAddPathArc(path, x, y + h - d, d, d, 90.0, 90.0);
        }
        GdipClosePathFigure(path);
        path
    }

    /// Rectángulo redondeado con relleno y/o borde opcionales.
    pub fn rounded(&self, x: f32, y: f32, w: f32, h: f32, radius: f32,
                   fill: Option<Color>, border: Option<(Color, f32)>) {
        unsafe {
            let path = self.rounded_path(x, y, w, h, radius);
            if let Some(c) = fill {
                let mut br: *mut GpSolidFill = std::ptr::null_mut();
                GdipCreateSolidFill(argb_u32(c), &mut br);
                GdipFillPath(self.g, br as *mut GpBrush, path);
                GdipDeleteBrush(br as *mut GpBrush);
            }
            if let Some((c, width)) = border {
                let mut pen: *mut GpPen = std::ptr::null_mut();
                GdipCreatePen1(argb_u32(c), width, 2 /*UnitPixel*/, &mut pen);
                GdipDrawPath(self.g, pen, path);
                GdipDeletePen(pen);
            }
            GdipDeletePath(path);
        }
    }

    /// Dibuja texto con alineación dentro de un rectángulo.
    pub fn text_in(&self, s: &str, x: f32, y: f32, w: f32, h: f32, size_pt: f32, bold: bool,
                   c: Color, halign: Align, valign: Align, icon: bool) {
        if s.is_empty() {
            return;
        }
        let utf16 = wide(s);
        let rect = RectF { X: x, Y: y, Width: w, Height: h };
        unsafe {
            let mut br: *mut GpSolidFill = std::ptr::null_mut();
            GdipCreateSolidFill(argb_u32(c), &mut br);
            GdipDrawString(
                self.g,
                utf16.as_ptr(),
                (utf16.len() - 1) as i32,
                font(size_pt, bold, icon),
                &rect,
                format(halign, valign),
                br as *mut GpBrush,
            );
            GdipDeleteBrush(br as *mut GpBrush);
        }
    }

    /// Texto alineado a la izquierda, centrado verticalmente (lo más común).
    pub fn text(&self, s: &str, x: i32, y: i32, w: i32, h: i32, size_pt: f32, bold: bool, c: Color) {
        self.text_in(s, x as f32, y as f32, w as f32, h as f32, size_pt, bold, c,
                     Align::Near, Align::Center, false);
    }
}

impl Drop for Gfx {
    fn drop(&mut self) {
        if !self.g.is_null() {
            unsafe { GdipDeleteGraphics(self.g) };
        }
    }
}

/// Imagen GDI+ cargada desde archivo (RAII).
pub struct Image {
    img: *mut GpImage,
}

impl Image {
    pub fn load(path: &str) -> Option<Image> {
        let w = wide(path);
        let mut img: *mut GpImage = std::ptr::null_mut();
        let st = unsafe { GdipLoadImageFromFile(w.as_ptr(), &mut img) };
        if st == 0 && !img.is_null() {
            Some(Image { img })
        } else {
            None
        }
    }
    pub fn size(&self) -> (f32, f32) {
        unsafe {
            let mut w = 0.0f32;
            let mut h = 0.0f32;
            GdipGetImageDimension(self.img, &mut w, &mut h);
            (w, h)
        }
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        if !self.img.is_null() {
            unsafe { GdipDisposeImage(self.img) };
        }
    }
}

impl Gfx {
    /// Dibuja una imagen cubriendo el rectángulo (recortando para mantener relación).
    pub fn draw_image_cover(&self, img: &Image, x: i32, y: i32, w: i32, h: i32) {
        let (iw, ih) = img.size();
        if iw <= 0.0 || ih <= 0.0 {
            return;
        }
        let scale = (w as f32 / iw).max(h as f32 / ih);
        let (dw, dh) = (iw * scale, ih * scale);
        let (dx, dy) = (x as f32 + (w as f32 - dw) / 2.0, y as f32 + (h as f32 - dh) / 2.0);
        unsafe {
            GdipDrawImageRect(self.g, img.img, dx, dy, dw, dh);
        }
    }
}

// ------------------------------------------------------------ chrome / controles
// Port de style::ChromeRects / DrawChrome / DrawToolButton de ui.h.

/// Rectángulos de los botones de la barra de título (min, max, close), en el
/// orden en que aparecen a la derecha. Devuelve (x, ancho) de cada uno.
pub struct Chrome {
    pub min: (i32, i32, i32, i32),
    pub max: (i32, i32, i32, i32),
    pub close: (i32, i32, i32, i32),
    pub title_h: i32,
}

pub fn chrome_rects(w: i32) -> Chrome {
    let title_h = s(42);
    let bw = s(46);
    Chrome {
        close: (w - bw, 0, bw, title_h),
        max: (w - bw * 2, 0, bw, title_h),
        min: (w - bw * 3, 0, bw, title_h),
        title_h,
    }
}
