// Área de notificación de la barra de tareas (bandeja del sistema).
//
// Enseña el volumen, la red (Wi-Fi con su intensidad o Ethernet), la batería,
// el Bluetooth y la campana de notificaciones, y abre un panel de
// «Configuración rápida» al estilo de Windows 11 con el deslizador de volumen
// y los accesos a los ajustes del sistema.
//
// Los iconos de batería, Wi-Fi, Ethernet y campana se dibujan con GDI+ en vez
// de usar glifos: así el nivel de carga y la intensidad de la señal se ven de
// verdad, y no dependen de que una fuente de iconos concreta esté instalada.
//
// El estado se muestrea cada pocos segundos (`sample`), no en cada repintado:
// consultar Core Audio o la tabla de interfaces en cada WM_PAINT sería
// justo el gasto que este proyecto evita.
#![allow(non_snake_case, non_upper_case_globals)]

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::notify;
use crate::sysinfo::{self, Net, Snapshot};
use crate::ui::config::{self, Id};
use crate::ui::gdip::{Align, Gfx};
use crate::ui::{self, theme, window};

pub const QUICK_CLASS: &str = "OShellRustQuick";

// ---------------------------------------------------------------- geometría
fn er() -> RECT {
    RECT { left: 0, top: 0, right: 0, bottom: 0 }
}
fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}
fn rw(r: &RECT) -> i32 {
    r.right - r.left
}
fn rh(r: &RECT) -> i32 {
    r.bottom - r.top
}
fn in_rect(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

// ---------------------------------------------------------------- elementos
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Icono publicado por una aplicación con Shell_NotifyIcon (índice en la lista).
    App(usize),
    Volume,
    Net,
    Bluetooth,
    Battery,
    Bell,
}

pub struct Item {
    pub kind: Kind,
    pub rect: RECT,
}

pub struct Tray {
    pub snap: Snapshot,
    pub items: Vec<Item>,
    /// Iconos de las aplicaciones (Shell_NotifyIcon), tal y como estaban al
    /// medir: así el dibujo y los clics ven la misma lista.
    pub apps: Vec<crate::traynotify::TrayIcon>,
    last_sample: u32,
}

impl Tray {
    pub fn new() -> Tray {
        Tray { snap: Snapshot::default(), items: Vec::new(), apps: Vec::new(), last_sample: 0 }
    }

    /// Vuelve a muestrear el estado si ha pasado el intervalo (o si `force`).
    /// Devuelve true cuando algo de lo que se ve ha cambiado.
    pub fn sample(&mut self, force: bool) -> bool {
        let now = unsafe { GetTickCount() };
        if !force && self.last_sample != 0 && now.wrapping_sub(self.last_sample) < 4000 {
            return false;
        }
        self.last_sample = now;
        let next = sysinfo::snapshot();
        let changed = next.volume != self.snap.volume
            || next.muted != self.snap.muted
            || next.has_audio != self.snap.has_audio
            || next.battery != self.snap.battery
            || next.net != self.snap.net
            || next.bluetooth != self.snap.bluetooth;
        self.snap = next;
        changed
    }

    /// Qué iconos tocan según la configuración y el equipo.
    fn kinds(&self) -> Vec<Kind> {
        let c = config::current();
        let mut v = Vec::new();
        // Primero los de las aplicaciones, como en Windows.
        if c.b(Id::BandejaIconosApps) {
            for i in 0..self.apps.len() {
                v.push(Kind::App(i));
            }
        }
        if c.b(Id::BandejaSonido) && self.snap.has_audio {
            v.push(Kind::Volume);
        }
        if c.b(Id::BandejaRed) {
            v.push(Kind::Net);
        }
        if c.b(Id::BandejaBluetooth) && self.snap.bluetooth.is_some() {
            v.push(Kind::Bluetooth);
        }
        if c.b(Id::BandejaBateria) && self.snap.battery.present {
            v.push(Kind::Battery);
        }
        if c.b(Id::BandejaNotificaciones) {
            v.push(Kind::Bell);
        }
        v
    }

    /// Coloca los iconos terminando en `right`. Devuelve el borde izquierdo
    /// que ocupa la bandeja (lo que queda libre para los botones de ventana).
    pub fn layout(&mut self, right: i32, h: i32) -> i32 {
        self.items.clear();
        if !config::current().b(Id::BandejaMostrar) {
            self.apps.clear();
            return right;
        }
        self.apps = if config::current().b(Id::BandejaIconosApps) {
            crate::traynotify::icons()
        } else {
            Vec::new()
        };
        let iw = ui::s(32);
        let pad = ui::s(4);
        let kinds = self.kinds();
        let mut total = 0;
        for k in &kinds {
            total += self.item_width(*k, iw);
        }
        let mut x = right - total - pad;
        let left = x;
        for k in kinds {
            let w = self.item_width(k, iw);
            self.items.push(Item { kind: k, rect: rect(x, 0, w, h) });
            x += w;
        }
        left - pad
    }

    fn item_width(&self, k: Kind, base: i32) -> i32 {
        match k {
            // Los iconos de aplicación son más estrechos: van pegados.
            Kind::App(_) => ui::s(26),
            // La batería con porcentaje necesita sitio para el texto.
            Kind::Battery if config::current().b(Id::BandejaPorcentajeBateria) && self.snap.battery.percent >= 0 => {
                base + ui::s(26)
            }
            _ => base,
        }
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<usize> {
        self.items.iter().position(|i| in_rect(&i.rect, x, y))
    }

    /// Texto descriptivo del icono (para el tooltip de la barra).
    pub fn tooltip(&self, i: usize) -> String {
        let Some(it) = self.items.get(i) else { return String::new() };
        match it.kind {
            Kind::App(j) => self.apps.get(j).map(|a| a.tip.clone()).unwrap_or_default(),
            Kind::Volume => {
                if self.snap.muted {
                    "Sonido silenciado".into()
                } else {
                    format!("Volumen: {} %", self.snap.volume)
                }
            }
            Kind::Net => match &self.snap.net {
                Net::Off => "Sin conexión".into(),
                Net::Ethernet { mbps } if *mbps > 0 => format!("Ethernet · {} Mbps", mbps),
                Net::Ethernet { .. } => "Ethernet conectado".into(),
                Net::Wifi { ssid, quality } => format!("{} · {} %", ssid, quality),
            },
            Kind::Bluetooth => match self.snap.bluetooth {
                Some(true) => "Bluetooth disponible".into(),
                _ => "Bluetooth no disponible".into(),
            },
            Kind::Battery => {
                let b = self.snap.battery;
                let pct = if b.percent >= 0 { format!("{} %", b.percent) } else { "desconocida".into() };
                if b.charging {
                    format!("Batería {} (cargando)", pct)
                } else {
                    format!("Batería {}", pct)
                }
            }
            Kind::Bell => {
                let n = notify::unread();
                if n == 0 {
                    "Notificaciones".into()
                } else {
                    format!("{} notificaciones nuevas", n)
                }
            }
        }
    }

    pub fn paint(&self, g: &Gfx, s: &theme::Style, hover: isize) {
        for (i, it) in self.items.iter().enumerate() {
            let r = it.rect;
            if hover == i as isize {
                g.rounded((r.left + ui::s(2)) as f32, (r.top + ui::s(5)) as f32,
                          (rw(&r) - ui::s(4)) as f32, (rh(&r) - ui::s(10)) as f32, ui::sf(5.0),
                          Some(s.task_button_hover), None);
            }
            let cx = (r.left + r.right) / 2;
            let cy = (r.top + r.bottom) / 2;
            match it.kind {
                Kind::App(j) => {
                    if let Some(a) = self.apps.get(j) {
                        let sz = ui::s(16);
                        g.draw_hicon(a.icon, cx - sz / 2, cy - sz / 2, sz);
                    }
                }
                Kind::Volume => {
                    let glyph = if self.snap.muted {
                        "\u{E74F}"
                    } else {
                        match self.snap.volume {
                            0 => "\u{E992}",
                            1..=33 => "\u{E993}",
                            34..=66 => "\u{E994}",
                            _ => "\u{E995}",
                        }
                    };
                    g.text_in(glyph, r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32, 11.0, false,
                              if self.snap.muted { s.text_faint } else { s.text }, Align::Center, Align::Center, true);
                }
                Kind::Net => match &self.snap.net {
                    Net::Wifi { quality, .. } => draw_wifi(g, s, cx, cy, *quality as i32),
                    Net::Ethernet { .. } => draw_ethernet(g, s, cx, cy, true),
                    Net::Off => draw_ethernet(g, s, cx, cy, false),
                },
                Kind::Bluetooth => {
                    let on = self.snap.bluetooth == Some(true);
                    g.text_in("\u{E702}", r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32, 11.0,
                              false, if on { s.text } else { s.text_faint }, Align::Center, Align::Center, true);
                }
                Kind::Battery => {
                    let b = self.snap.battery;
                    let show_pct = config::current().b(Id::BandejaPorcentajeBateria) && b.percent >= 0;
                    let bw = ui::s(22);
                    let bh = ui::s(11);
                    let bx = if show_pct { r.left + ui::s(6) } else { cx - bw / 2 };
                    draw_battery(g, s, rect(bx, cy - bh / 2, bw, bh), b.percent, b.charging);
                    if show_pct {
                        g.text_in(&format!("{}%", b.percent), (bx + bw + ui::s(4)) as f32, r.top as f32,
                                  ui::s(26) as f32, rh(&r) as f32, 10.0, false, s.text, Align::Near,
                                  Align::Center, false);
                    }
                }
                Kind::Bell => draw_bell(g, s, cx, cy, notify::unread()),
            }
        }
    }
}

// ---------------------------------------------------------------- iconos dibujados
fn draw_wifi(g: &Gfx, s: &theme::Style, cx: i32, cy: i32, quality: i32) {
    // Cuatro barras crecientes; las que superan la calidad quedan atenuadas.
    let bw = ui::sf(3.0);
    let gap = ui::sf(2.0);
    let base = cy as f32 + ui::sf(7.0);
    let total = bw * 4.0 + gap * 3.0;
    let x0 = cx as f32 - total / 2.0;
    let active = ((quality.clamp(0, 100) + 24) / 25).min(4);
    for i in 0..4 {
        let hgt = ui::sf(4.0) + i as f32 * ui::sf(3.2);
        let col = if i < active { s.text } else { theme::with_alpha(s.text_faint, 90) };
        g.rounded(x0 + i as f32 * (bw + gap), base - hgt, bw, hgt, bw / 2.0, Some(col), None);
    }
}

fn draw_ethernet(g: &Gfx, s: &theme::Style, cx: i32, cy: i32, connected: bool) {
    // Monitor con peana: el mismo símbolo que usa Windows para la red por cable.
    let col = if connected { s.text } else { s.text_faint };
    let w = ui::sf(15.0);
    let h = ui::sf(11.0);
    let x = cx as f32 - w / 2.0;
    let y = cy as f32 - h / 2.0 - ui::sf(2.0);
    if connected {
        g.rounded(x, y, w, h, ui::sf(1.8), Some(col), None);
    } else {
        g.rounded(x, y, w, h, ui::sf(1.8), None, Some((col, ui::sf(1.4))));
    }
    // Pie y base.
    g.fill_rect_f(cx as f32 - ui::sf(1.5), y + h, ui::sf(3.0), ui::sf(2.5), col);
    g.rounded(cx as f32 - w * 0.33, y + h + ui::sf(2.5), w * 0.66, ui::sf(1.8), ui::sf(0.9), Some(col), None);
    if !connected {
        // Aspa cuando no hay enlace.
        let bx = x + w + ui::sf(1.0);
        let by = y + ui::sf(1.0);
        g.line(bx, by, bx + ui::sf(5.0), by + ui::sf(5.0), s.danger, ui::sf(1.6));
        g.line(bx + ui::sf(5.0), by, bx, by + ui::sf(5.0), s.danger, ui::sf(1.6));
    }
}

fn draw_battery(g: &Gfx, s: &theme::Style, r: RECT, percent: i32, charging: bool) {
    let x = r.left as f32;
    let y = r.top as f32;
    let w = rw(&r) as f32;
    let h = rh(&r) as f32;
    let low = percent >= 0 && percent <= 20 && !charging;
    let col = if low { s.danger } else { s.text };
    g.rounded(x, y, w, h, ui::sf(2.5), None, Some((col, ui::sf(1.3))));
    // Borne positivo.
    g.rounded(x + w + ui::sf(1.0), y + h * 0.3, ui::sf(2.0), h * 0.4, ui::sf(1.0), Some(col), None);
    let frac = percent.clamp(0, 100) as f32 / 100.0;
    let inner = ui::sf(2.0);
    let fill_w = ((w - inner * 2.0) * frac).max(0.0);
    if fill_w > 0.5 {
        let fc = if charging { s.success } else { col };
        g.rounded(x + inner, y + inner, fill_w, h - inner * 2.0, ui::sf(1.2), Some(fc), None);
    }
    if charging {
        // Rayo: dos triángulos encontrados.
        let cx = x + w / 2.0;
        let cy = y + h / 2.0;
        let a = ui::sf(3.0);
        g.fill_poly(&[(cx + a * 0.3, cy - a), (cx - a * 0.6, cy + a * 0.2), (cx + a * 0.1, cy + a * 0.2)], s.on_accent);
        g.fill_poly(&[(cx + a * 0.6, cy - a * 0.2), (cx - a * 0.3, cy + a), (cx - a * 0.1, cy - a * 0.2)], s.on_accent);
    }
}

fn draw_bell(g: &Gfx, s: &theme::Style, cx: i32, cy: i32, unread: usize) {
    let col = if unread > 0 { s.accent } else { s.text };
    let x = cx as f32;
    let y = cy as f32;
    let r = ui::sf(7.0);
    // Silueta de campana: polígono relleno (GDI+ no dibuja arcos sueltos aquí).
    let pts: [(f32, f32); 10] = [
        (-0.52, 0.34), (-0.44, 0.10), (-0.36, -0.20), (-0.20, -0.42),
        (0.20, -0.42), (0.36, -0.20), (0.44, 0.10), (0.52, 0.34),
        (0.52, 0.44), (-0.52, 0.44),
    ];
    let poly: Vec<(f32, f32)> = pts.iter().map(|(px, py)| (x + px * r * 2.0, y + py * r * 2.0)).collect();
    g.fill_poly(&poly, col);
    // Asa arriba y badajo abajo.
    g.fill_circle(x, y - r * 0.92, ui::sf(1.5), col);
    g.fill_circle(x, y + r * 0.62, ui::sf(1.8), col);
    if unread > 0 {
        let bx = x + r * 0.9;
        let by = y - r * 0.8;
        g.fill_circle(bx, by, ui::sf(6.0), s.danger);
        let txt = if unread > 9 { "9+".to_string() } else { unread.to_string() };
        g.text_in(&txt, bx - ui::sf(8.0), by - ui::sf(8.0), ui::sf(16.0), ui::sf(16.0), 7.0, true,
                  theme::rgbc(255, 255, 255), Align::Center, Align::Center, false);
    }
}

// =============================================================== configuración rápida
#[derive(Clone, Copy, PartialEq)]
enum QHit {
    None,
    Tile(usize),
    VolSlider,
    Mute,
    Settings,
    TaskMgr,
    NetRow,
    BatteryRow,
}

/// Mosaicos del panel: etiqueta, estado y acción.
#[derive(Clone, Copy, PartialEq)]
enum Tile {
    Wifi,
    Bluetooth,
    Dnd,
    Theme,
}

struct Quick {
    snap: Snapshot,
    /// Estado de las radios (None = el equipo no tiene esa radio).
    wifi: Option<bool>,
    bluetooth: Option<bool>,
    /// Mensaje cuando una radio no se deja cambiar.
    msg: Option<String>,
    tiles: Vec<(Tile, RECT)>,
    vol_track: RECT,
    mute_rc: RECT,
    net_rc: RECT,
    bat_rc: RECT,
    settings_rc: RECT,
    taskmgr_rc: RECT,
    hover: QHit,
    dragging: bool,
}

fn quick_metrics() -> (i32, i32) {
    let bat = sysinfo::battery().present;
    (ui::s(340), ui::s(if bat { 268 } else { 232 }))
}

fn quick_layout(hwnd: HWND, q: &mut Quick) {
    let mut rc = er();
    unsafe { GetClientRect(hwnd, &mut rc) };
    let pad = ui::s(14);
    let cols = 4;
    let gap = ui::s(10);
    let tw = (rw(&rc) - pad * 2 - gap * (cols - 1)) / cols;
    let th = ui::s(62);
    let tiles = [Tile::Wifi, Tile::Bluetooth, Tile::Dnd, Tile::Theme];
    q.tiles = tiles
        .iter()
        .enumerate()
        .map(|(i, t)| (*t, rect(pad + i as i32 * (tw + gap), pad, tw, th)))
        .collect();

    let vy = pad + th + ui::s(18);
    q.mute_rc = rect(pad, vy - ui::s(10), ui::s(28), ui::s(28));
    q.vol_track = rect(pad + ui::s(38), vy, rw(&rc) - pad * 2 - ui::s(76), ui::s(6));

    let ry = vy + ui::s(26);
    q.net_rc = rect(pad, ry, rw(&rc) - pad * 2, ui::s(34));
    q.bat_rc = if sysinfo::battery().present {
        rect(pad, ry + ui::s(38), rw(&rc) - pad * 2, ui::s(34))
    } else {
        er()
    };

    let by = rc.bottom - ui::s(46);
    let bw = (rw(&rc) - pad * 2 - gap) / 2;
    q.settings_rc = rect(pad, by, bw, ui::s(34));
    q.taskmgr_rc = rect(pad + bw + gap, by, bw, ui::s(34));
}

fn tile_state(q: &Quick, t: Tile) -> (&'static str, String, bool) {
    match t {
        // El mosaico de Wi-Fi enciende y apaga la radio de verdad; si el
        // equipo no tiene Wi-Fi enseña la conexión que haya.
        Tile::Wifi => match q.wifi {
            Some(true) => match &q.snap.net {
                Net::Wifi { ssid, .. } => ("\u{E701}", ssid.clone(), true),
                _ => ("\u{E701}", "Encendido".into(), true),
            },
            Some(false) => ("\u{E701}", "Apagado".into(), false),
            None => match &q.snap.net {
                Net::Ethernet { .. } => ("\u{E701}", "Ethernet".into(), true),
                Net::Wifi { ssid, .. } => ("\u{E701}", ssid.clone(), true),
                Net::Off => ("\u{E701}", "Sin red".into(), false),
            },
        },
        Tile::Bluetooth => (
            "\u{E702}",
            match q.bluetooth {
                Some(true) => "Encendido".into(),
                Some(false) => "Apagado".into(),
                None => match q.snap.bluetooth {
                    Some(true) => "Disponible".into(),
                    _ => "No instalado".into(),
                },
            },
            q.bluetooth.unwrap_or(q.snap.bluetooth == Some(true)),
        ),
        Tile::Dnd => {
            let on = config::current().b(Id::NotifNoMolestar);
            ("\u{E7E7}", if on { "Activado".into() } else { "Desactivado".into() }, on)
        }
        Tile::Theme => {
            let dark = config::current().dark();
            ("\u{E793}", if dark { "Oscuro".into() } else { "Claro".into() }, dark)
        }
    }
}

fn tile_label(t: Tile) -> &'static str {
    match t {
        Tile::Wifi => "Red",
        Tile::Bluetooth => "Bluetooth",
        Tile::Dnd => "No molestar",
        Tile::Theme => "Tema",
    }
}

fn paint_quick(hwnd: HWND, q: &Quick) {
    window::paint_buffered(hwnd, |g: &Gfx, w, h| {
        let s = theme::style();
        g.clear(theme::argb(0, 0, 0, 0));
        g.rounded(0.0, 0.0, w as f32, h as f32, ui::sf(10.0), Some(s.flyout_bg), Some((s.flyout_border, 1.0)));

        for (i, (t, r)) in q.tiles.iter().enumerate() {
            let (glyph, state, on) = tile_state(q, *t);
            let hov = q.hover == QHit::Tile(i);
            let fill = if on {
                if hov { s.accent_hover } else { s.accent_solid }
            } else if hov {
                s.control_hover
            } else {
                s.control_bg
            };
            let fg = if on { s.on_accent } else { s.text };
            g.rounded(r.left as f32, r.top as f32, rw(r) as f32, rh(r) as f32, ui::sf(8.0), Some(fill),
                      Some((s.control_border, 1.0)));
            g.text_in(glyph, r.left as f32, (r.top + ui::s(8)) as f32, rw(r) as f32, ui::s(20) as f32, 12.0,
                      false, fg, Align::Center, Align::Center, true);
            g.text_in(tile_label(*t), r.left as f32, (r.top + ui::s(28)) as f32, rw(r) as f32,
                      ui::s(14) as f32, 9.5, false, fg, Align::Center, Align::Center, false);
            g.text_in(&state, r.left as f32, (r.top + ui::s(42)) as f32, rw(r) as f32, ui::s(14) as f32, 8.5,
                      false, if on { s.on_accent } else { s.text_faint }, Align::Center, Align::Center, false);
        }

        // volumen
        let muted = q.snap.muted;
        let glyph = if muted {
            "\u{E74F}"
        } else {
            match q.snap.volume {
                0 => "\u{E992}",
                1..=33 => "\u{E993}",
                34..=66 => "\u{E994}",
                _ => "\u{E995}",
            }
        };
        if q.hover == QHit::Mute {
            g.rounded(q.mute_rc.left as f32, q.mute_rc.top as f32, rw(&q.mute_rc) as f32,
                      rh(&q.mute_rc) as f32, ui::sf(5.0), Some(s.control_hover), None);
        }
        g.text_in(glyph, q.mute_rc.left as f32, q.mute_rc.top as f32, rw(&q.mute_rc) as f32,
                  rh(&q.mute_rc) as f32, 11.0, false, if muted { s.text_faint } else { s.text },
                  Align::Center, Align::Center, true);
        let t = q.vol_track;
        let tw = rw(&t) as f32;
        let th = rh(&t) as f32;
        g.rounded(t.left as f32, t.top as f32, tw, th, th / 2.0, Some(s.track), None);
        let frac = q.snap.volume.clamp(0, 100) as f32 / 100.0;
        let fill = if muted { s.text_faint } else { s.accent_solid };
        g.rounded(t.left as f32, t.top as f32, tw * frac, th, th / 2.0, Some(fill), None);
        let kx = t.left as f32 + tw * frac;
        g.fill_circle(kx, t.top as f32 + th / 2.0, ui::sf(8.0), fill);
        g.fill_circle(kx, t.top as f32 + th / 2.0, ui::sf(4.0), s.on_accent);
        g.text_in(&format!("{}", q.snap.volume), (t.right + ui::s(8)) as f32, (t.top - ui::s(12)) as f32,
                  ui::s(34) as f32, ui::s(28) as f32, 11.0, false, s.text_dim, Align::Far, Align::Center, false);

        // filas de estado
        let net_text = match &q.snap.net {
            Net::Off => "Sin conexión a la red".to_string(),
            Net::Ethernet { mbps } if *mbps > 0 => format!("Ethernet · {} Mbps", mbps),
            Net::Ethernet { .. } => "Ethernet conectado".to_string(),
            Net::Wifi { ssid, quality } => format!("{} · señal {} %", ssid, quality),
        };
        info_row(g, &s, q.net_rc, "\u{E704}", "Red", &net_text, q.hover == QHit::NetRow);
        if rw(&q.bat_rc) > 0 {
            let b = q.snap.battery;
            let txt = match (b.percent, b.charging, b.seconds_left) {
                (p, true, _) if p >= 0 => format!("{} %, cargando", p),
                (p, false, sec) if p >= 0 && sec > 0 => {
                    format!("{} %, quedan {} h {:02} min", p, sec / 3600, (sec % 3600) / 60)
                }
                (p, _, _) if p >= 0 => format!("{} %", p),
                _ => "Estado desconocido".into(),
            };
            info_row(g, &s, q.bat_rc, "\u{E83E}", "Batería", &txt, q.hover == QHit::BatteryRow);
        }

        // aviso cuando una radio no se deja cambiar
        if let Some(m) = &q.msg {
            g.text_in(m, ui::s(14) as f32, (q.settings_rc.top - ui::s(20)) as f32,
                      (w - ui::s(28)) as f32, ui::s(18) as f32, 10.5, false, s.danger,
                      Align::Near, Align::Center, false);
        }
        // pie
        quick_btn(g, &s, q.settings_rc, "\u{E713}", "Configuración", q.hover == QHit::Settings);
        quick_btn(g, &s, q.taskmgr_rc, "\u{E9D9}", "Administrador", q.hover == QHit::TaskMgr);
    });
}

fn info_row(g: &Gfx, s: &theme::Style, r: RECT, glyph: &str, label: &str, value: &str, hov: bool) {
    if hov {
        g.rounded(r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32, ui::sf(6.0), Some(s.row_hover), None);
    }
    g.text_in(glyph, (r.left + ui::s(4)) as f32, r.top as f32, ui::s(22) as f32, rh(&r) as f32, 10.5, false,
              s.accent, Align::Center, Align::Center, true);
    g.text_in(label, (r.left + ui::s(32)) as f32, r.top as f32, ui::s(90) as f32, rh(&r) as f32, 11.5, false,
              s.text, Align::Near, Align::Center, false);
    g.text_in(value, (r.left + ui::s(120)) as f32, r.top as f32, (rw(&r) - ui::s(140)) as f32, rh(&r) as f32,
              11.0, false, s.text_dim, Align::Far, Align::Center, false);
    g.text_in("\u{E76C}", (r.right - ui::s(18)) as f32, r.top as f32, ui::s(16) as f32, rh(&r) as f32, 8.0,
              false, s.text_faint, Align::Center, Align::Center, true);
}

fn quick_btn(g: &Gfx, s: &theme::Style, r: RECT, glyph: &str, label: &str, hov: bool) {
    g.rounded(r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32, ui::sf(6.0),
              Some(if hov { s.control_hover } else { s.control_bg }), Some((s.control_border, 1.0)));
    g.text_in(glyph, (r.left + ui::s(10)) as f32, r.top as f32, ui::s(18) as f32, rh(&r) as f32, 9.5, false,
              s.text, Align::Center, Align::Center, true);
    g.text_in(label, (r.left + ui::s(32)) as f32, r.top as f32, (rw(&r) - ui::s(40)) as f32, rh(&r) as f32,
              11.5, false, s.text, Align::Near, Align::Center, false);
}

fn quick_hit(q: &Quick, x: i32, y: i32) -> QHit {
    for (i, (_, r)) in q.tiles.iter().enumerate() {
        if in_rect(r, x, y) {
            return QHit::Tile(i);
        }
    }
    if in_rect(&q.mute_rc, x, y) {
        return QHit::Mute;
    }
    let t = q.vol_track;
    if in_rect(&rect(t.left - ui::s(10), t.top - ui::s(14), rw(&t) + ui::s(20), ui::s(32)), x, y) {
        return QHit::VolSlider;
    }
    if in_rect(&q.net_rc, x, y) {
        return QHit::NetRow;
    }
    if rw(&q.bat_rc) > 0 && in_rect(&q.bat_rc, x, y) {
        return QHit::BatteryRow;
    }
    if in_rect(&q.settings_rc, x, y) {
        return QHit::Settings;
    }
    if in_rect(&q.taskmgr_rc, x, y) {
        return QHit::TaskMgr;
    }
    QHit::None
}

fn set_volume_at(q: &mut Quick, x: i32) {
    let t = q.vol_track;
    let w = rw(&t).max(1);
    let v = ((x - t.left) as i64 * 100 / w as i64).clamp(0, 100) as i32;
    q.snap.volume = v;
    sysinfo::set_volume(v);
}

unsafe extern "system" fn quick_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let q_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Quick;
    if msg == ui::config_changed_msg() {
        if !q_ptr.is_null() {
            quick_layout(hwnd, &mut *q_ptr);
            InvalidateRect(hwnd, std::ptr::null(), 0);
        }
        return 0;
    }
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            if !q_ptr.is_null() {
                paint_quick(hwnd, &*q_ptr);
            }
            0
        }
        WM_SIZE => {
            if !q_ptr.is_null() {
                quick_layout(hwnd, &mut *q_ptr);
            }
            0
        }
        WM_MOUSEMOVE => {
            if !q_ptr.is_null() {
                let q = &mut *q_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                if q.dragging {
                    set_volume_at(q, x);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                } else {
                    let h = quick_hit(q, x, y);
                    if h != q.hover {
                        q.hover = h;
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !q_ptr.is_null() {
                let q = &mut *q_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                match quick_hit(q, x, y) {
                    QHit::VolSlider => {
                        q.dragging = true;
                        SetCapture(hwnd);
                        set_volume_at(q, x);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                    QHit::Mute => {
                        q.snap.muted = sysinfo::toggle_mute();
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                    QHit::Tile(i) => {
                        let t = q.tiles[i].0;
                        match t {
                            Tile::Wifi => {
                                // Si hay radio Wi-Fi se enciende o se apaga;
                                // si no, se abre la página de red de Windows.
                                match q.wifi {
                                    Some(on) => {
                                        if crate::radio::set_wifi(!on) {
                                            q.wifi = Some(!on);
                                            q.msg = None;
                                        } else {
                                            q.msg = Some("No se pudo cambiar la radio Wi-Fi.".into());
                                        }
                                        InvalidateRect(hwnd, std::ptr::null(), 0);
                                    }
                                    None => {
                                        ShowWindow(hwnd, SW_HIDE);
                                        crate::shell::launch_own("--settings red");
                                    }
                                }
                            }
                            Tile::Bluetooth => {
                                match q.bluetooth {
                                    Some(on) => {
                                        if crate::radio::set_bluetooth(!on) {
                                            q.bluetooth = Some(!on);
                                            q.msg = None;
                                        } else {
                                            q.msg = Some(
                                                "Windows no permite cambiar la radio Bluetooth.".into());
                                        }
                                        InvalidateRect(hwnd, std::ptr::null(), 0);
                                    }
                                    None => {
                                        ShowWindow(hwnd, SW_HIDE);
                                        crate::shell::launch_own("--settings bluetooth");
                                    }
                                }
                            }
                            Tile::Dnd => {
                                let mut c = config::current();
                                c.toggle(Id::NotifNoMolestar);
                                ui::apply_config(&c, true);
                                ui::broadcast_config_changed();
                                InvalidateRect(hwnd, std::ptr::null(), 0);
                            }
                            Tile::Theme => {
                                let mut c = config::current();
                                c.set(Id::Modo, if c.dark() { 1 } else { 0 });
                                ui::apply_config(&c, true);
                                ui::broadcast_config_changed();
                                InvalidateRect(hwnd, std::ptr::null(), 0);
                            }
                        }
                    }
                    QHit::NetRow => {
                        ShowWindow(hwnd, SW_HIDE);
                        crate::shell::launch_own("--settings red");
                    }
                    QHit::BatteryRow => {
                        ShowWindow(hwnd, SW_HIDE);
                        crate::shell::launch_own("--settings sistema");
                    }
                    QHit::Settings => {
                        ShowWindow(hwnd, SW_HIDE);
                        crate::shell::launch_own("--settings sistema");
                    }
                    QHit::TaskMgr => {
                        ShowWindow(hwnd, SW_HIDE);
                        crate::shell::launch_own("--taskmgr");
                    }
                    QHit::None => {}
                }
            }
            0
        }
        WM_LBUTTONUP => {
            if !q_ptr.is_null() {
                let q = &mut *q_ptr;
                if q.dragging {
                    q.dragging = false;
                    ReleaseCapture();
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            if !q_ptr.is_null() {
                let q = &mut *q_ptr;
                let delta = ((wp >> 16) & 0xFFFF) as i16 as i32;
                let v = (q.snap.volume + (delta / 120) * 2).clamp(0, 100);
                q.snap.volume = v;
                sysinfo::set_volume(v);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_ACTIVATE => {
            if (wp & 0xFFFF) == WA_INACTIVE as usize {
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_DESTROY => {
            if !q_ptr.is_null() {
                drop(Box::from_raw(q_ptr));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Crea (oculta) la ventana de Configuración rápida.
pub fn create_quick(hinst: HINSTANCE) -> HWND {
    unsafe {
        let name = ui::wide(QUICK_CLASS);
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(quick_proc),
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
        let (w, h) = quick_metrics();
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            name.as_ptr(),
            ui::wide("Configuración rápida").as_ptr(),
            WS_POPUP,
            0, 0, w, h,
            std::ptr::null_mut(), std::ptr::null_mut(), hinst, std::ptr::null(),
        );
        if hwnd.is_null() {
            return hwnd;
        }
        let q = Box::new(Quick {
            snap: Snapshot::default(),
            wifi: None,
            bluetooth: None,
            msg: None,
            tiles: Vec::new(),
            vol_track: er(),
            mute_rc: er(),
            net_rc: er(),
            bat_rc: er(),
            settings_rc: er(),
            taskmgr_rc: er(),
            hover: QHit::None,
            dragging: false,
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(q) as isize);
        window::apply_frame_opaque(hwnd, config::current().dark());
        quick_layout(hwnd, &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Quick));
        hwnd
    }
}

/// Abre o cierra el panel, anclado al icono pulsado y con el estado recién
/// muestreado.
pub fn toggle_quick(hwnd: HWND, anchor_x: i32, snap: &Snapshot) {
    unsafe {
        if hwnd.is_null() {
            return;
        }
        if IsWindowVisible(hwnd) != 0 {
            ShowWindow(hwnd, SW_HIDE);
            return;
        }
        let q_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Quick;
        if !q_ptr.is_null() {
            (*q_ptr).snap = snap.clone();
            (*q_ptr).hover = QHit::None;
            (*q_ptr).msg = None;
            // El estado de las radios se consulta al abrir el panel, no antes:
            // WinRT tarda unos milisegundos y no merece pagarlos de fondo.
            (*q_ptr).wifi = crate::radio::wifi_on();
            (*q_ptr).bluetooth = crate::radio::bluetooth_on();
        }
        let (w, h) = quick_metrics();
        let bar = crate::shell::bar_rect();
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let x = (anchor_x - w / 2).clamp(ui::s(8), (sw - w - ui::s(8)).max(0));
        let y = if config::current().get(Id::BarraPosicion) == 1 {
            bar.bottom + ui::s(8)
        } else {
            bar.top - h - ui::s(8)
        };
        SetWindowPos(hwnd, HWND_TOPMOST, x, y.max(0), w, h, SWP_SHOWWINDOW);
        if !q_ptr.is_null() {
            quick_layout(hwnd, &mut *q_ptr);
        }
        SetForegroundWindow(hwnd);
        InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}
