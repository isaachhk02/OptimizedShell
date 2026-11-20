// Configuración del sistema (--settings), al estilo de la Configuración de
// Windows 11.
//
// Estructura: barra lateral con buscador y categorías (Sistema, Bluetooth y
// dispositivos, Red e Internet, Personalización, Aplicaciones, Accesibilidad,
// Acerca de), página de inicio con la tarjeta del equipo y accesos a las
// categorías, y páginas formadas por grupos de tarjetas.
//
// Los controles no están escritos a mano: cada ajuste de `ui::config` se
// dibuja según su tipo (interruptor, deslizador, control segmentado, tarjetas
// de modo o selector de color), así que añadir una fila a `config::ITEMS` y
// citarla en un grupo basta para que aparezca aquí. Junto a los ajustes
// propios hay filas de información del equipo y accesos a las páginas de
// Windows (ms-settings:) que OptimizedShell no reimplementa.
//
// Cada cambio se aplica al momento, se guarda en %APPDATA%\OptimizedShell\
// config.ini y se difunde al resto de ventanas de OptimizedShell.
#![allow(non_snake_case, non_upper_case_globals)]

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, ScreenToClient};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, VK_BACK, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RIGHT, VK_UP,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::args;
use crate::display;
use crate::perf;
use crate::sysinfo::{self, Net};
use crate::ui::config::{self, Config, Id, Item, Kind, Page};
use crate::ui::gdip::{Align, Gfx};
use crate::ui::theme::{self, ACCENT_PRESETS};
use crate::ui::{self, window};

// ============================================================ modelo de páginas
/// Acción de una fila de enlace.
#[derive(Clone, Copy, PartialEq)]
enum Act {
    /// Abre algo con el shell de Windows (ms-settings:, control.exe...).
    Url(&'static str),
    /// Lanza OptimizedShell con estos argumentos.
    Own(&'static str),
}

/// Dato del equipo que se enseña en una fila de solo lectura.
#[derive(Clone, Copy, PartialEq)]
enum InfoKind {
    Equipo,
    Usuario,
    Windows,
    Cpu,
    Ram,
    Red,
    Bateria,
    Bluetooth,
    Audio,
    Archivo,
}

#[derive(Clone, Copy)]
enum Row {
    /// Un ajuste de `config::ITEMS`.
    S(Id),
    Link { glyph: &'static str, title: &'static str, sub: &'static str, act: Act },
    Info(InfoKind),
    /// Galería de fondos de escritorio con miniaturas.
    Gallery,
    /// Monitores, resoluciones y frecuencias.
    Displays,
}

struct Group {
    title: &'static str,
    rows: &'static [Row],
}

struct NavPage {
    glyph: &'static str,
    title: &'static str,
    sub: &'static str,
    /// Nombre admitido como argumento posicional (`--settings <arg>`).
    arg: &'static str,
    /// Página de `config::Page` que restablece el botón «Restablecer».
    reset: Option<Page>,
    groups: &'static [Group],
}

// ------------------------------------------------------------ Sistema
static SISTEMA: &[Group] = &[
    Group { title: "Notificaciones", rows: &[
        Row::S(Id::NotifActivadas),
        Row::S(Id::NotifNoMolestar),
        Row::S(Id::NotifSonido),
        Row::S(Id::NotifDuracion),
        Row::S(Id::NotifMax),
        Row::S(Id::NotifAncho),
        Row::S(Id::NotifPosicion),
        Row::S(Id::NotifHistorial),
    ]},
    Group { title: "Energía y rendimiento", rows: &[
        Row::S(Id::Eco),
        Row::S(Id::LiberarMemoria),
        Row::Info(InfoKind::Bateria),
        Row::Link { glyph: "\u{E945}", title: "Energía y suspensión",
                    sub: "Pantalla, suspensión y plan de energía de Windows",
                    act: Act::Url("ms-settings:powersleep") },
    ]},
    Group { title: "Herramientas", rows: &[
        Row::Link { glyph: "\u{E9D9}", title: "Administrador de tareas",
                    sub: "Procesos, rendimiento, inicio y servicios", act: Act::Own("--taskmgr") },
        Row::Link { glyph: "\u{EC50}", title: "Explorador de archivos",
                    sub: "Abre el explorador de OptimizedShell", act: Act::Own("--explorer C:\\") },
        Row::Link { glyph: "\u{E7F8}", title: "Pantalla",
                    sub: "Resolución, escala y varios monitores", act: Act::Url("ms-settings:display") },
        Row::Link { glyph: "\u{E8D6}", title: "Sonido",
                    sub: "Dispositivos de salida y entrada", act: Act::Url("ms-settings:sound") },
    ]},
];

// ------------------------------------------------------------ dispositivos
static DISPOSITIVOS: &[Group] = &[
    Group { title: "Estado", rows: &[
        Row::Info(InfoKind::Bluetooth),
        Row::Info(InfoKind::Audio),
    ]},
    Group { title: "Área de notificación", rows: &[
        Row::S(Id::BandejaBluetooth),
        Row::S(Id::BandejaSonido),
    ]},
    Group { title: "Ajustes de Windows", rows: &[
        Row::Link { glyph: "\u{E702}", title: "Bluetooth",
                    sub: "Emparejar y administrar dispositivos", act: Act::Url("ms-settings:bluetooth") },
        Row::Link { glyph: "\u{E749}", title: "Impresoras y escáneres",
                    sub: "", act: Act::Url("ms-settings:printers") },
        Row::Link { glyph: "\u{E962}", title: "Mouse y panel táctil",
                    sub: "", act: Act::Url("ms-settings:mousetouchpad") },
        Row::Link { glyph: "\u{E765}", title: "Teclado e idioma",
                    sub: "", act: Act::Url("ms-settings:keyboard") },
    ]},
];

// ------------------------------------------------------------ red
static RED: &[Group] = &[
    Group { title: "Estado", rows: &[
        Row::Info(InfoKind::Red),
    ]},
    Group { title: "Área de notificación", rows: &[
        Row::S(Id::BandejaRed),
    ]},
    Group { title: "Ajustes de Windows", rows: &[
        Row::Link { glyph: "\u{E701}", title: "Wi-Fi",
                    sub: "Redes disponibles y redes conocidas", act: Act::Url("ms-settings:network-wifi") },
        Row::Link { glyph: "\u{E839}", title: "Ethernet",
                    sub: "", act: Act::Url("ms-settings:network-ethernet") },
        Row::Link { glyph: "\u{E968}", title: "Configuración de red avanzada",
                    sub: "Adaptadores, uso de datos y restablecimiento",
                    act: Act::Url("ms-settings:network-advancedsettings") },
        Row::Link { glyph: "\u{E72E}", title: "Conexiones de red (clásico)",
                    sub: "Panel de control", act: Act::Url("ncpa.cpl") },
    ]},
];

// ------------------------------------------------------------ personalización
static PERSONALIZACION: &[Group] = &[
    Group { title: "Modo y color", rows: &[
        Row::S(Id::Modo),
        Row::S(Id::Acento),
    ]},
    Group { title: "Efectos", rows: &[
        Row::S(Id::Desenfoque),
        Row::S(Id::TinteFondo),
        Row::S(Id::OpacidadBarra),
        Row::S(Id::OpacidadVentanas),
        Row::S(Id::Radio),
    ]},
    Group { title: "Texto y tamaño", rows: &[
        Row::S(Id::Fuente),
        Row::S(Id::EscalaTexto),
        Row::S(Id::EscalaInterfaz),
        Row::S(Id::ContrasteTexto),
    ]},
    Group { title: "Ventanas", rows: &[
        Row::S(Id::AltoTitulo),
        Row::S(Id::AnchoBotonTitulo),
    ]},
    Group { title: "Fondo de escritorio", rows: &[
        Row::Gallery,
        Row::S(Id::EscritorioAjuste),
        Row::S(Id::EscritorioColor),
        Row::S(Id::EscritorioActivo),
    ]},
    Group { title: "Ajustes de Windows", rows: &[
        Row::Link { glyph: "\u{E91B}", title: "Más opciones de fondo",
                    sub: "Presentación, color sólido y temas de Windows",
                    act: Act::Url("ms-settings:personalization-background") },
        Row::Link { glyph: "\u{E770}", title: "Pantalla de bloqueo",
                    sub: "", act: Act::Url("ms-settings:lockscreen") },
    ]},
];

// ------------------------------------------------------------ pantalla
static PANTALLA: &[Group] = &[
    Group { title: "Monitores", rows: &[
        Row::Displays,
    ]},
    Group { title: "Escritorio", rows: &[
        Row::S(Id::EscritorioActivo),
        Row::S(Id::HotkeysActivas),
    ]},
    Group { title: "Ajustes de Windows", rows: &[
        Row::Link { glyph: "\u{E7F8}", title: "Opciones avanzadas de pantalla",
                    sub: "HDR, escala de Windows y varios monitores",
                    act: Act::Url("ms-settings:display") },
        Row::Link { glyph: "\u{E713}", title: "Configuración de gráficos",
                    sub: "", act: Act::Url("ms-settings:display-advancedgraphics") },
    ]},
];

// ------------------------------------------------------------ barra de tareas
static BARRA: &[Group] = &[
    Group { title: "Comportamiento de la barra", rows: &[
        Row::S(Id::BarraAlto),
        Row::S(Id::BarraPosicion),
        Row::S(Id::BarraReservarArea),
        Row::S(Id::BarraSiempreVisible),
        Row::S(Id::BarraRefresco),
    ]},
    Group { title: "Botón Inicio", rows: &[
        Row::S(Id::BarraMostrarInicio),
        Row::S(Id::BarraInicioCentro),
        Row::S(Id::BarraIconoInicio),
        Row::S(Id::BarraIconoIndice),
    ]},
    Group { title: "Botones de las ventanas", rows: &[
        Row::S(Id::BarraAlineacion),
        Row::S(Id::BarraIconosApps),
        Row::S(Id::BarraTamanoIcono),
        Row::S(Id::BarraMostrarTitulos),
        Row::S(Id::BarraAnchoBoton),
    ]},
    Group { title: "Área de notificación", rows: &[
        Row::S(Id::BandejaMostrar),
        Row::S(Id::BandejaIconosApps),
        Row::S(Id::BandejaSonido),
        Row::S(Id::BandejaRed),
        Row::S(Id::BandejaBateria),
        Row::S(Id::BandejaPorcentajeBateria),
        Row::S(Id::BandejaBluetooth),
        Row::S(Id::BandejaNotificaciones),
        Row::S(Id::BandejaSeparador),
    ]},
    Group { title: "Reloj", rows: &[
        Row::S(Id::BarraMostrarReloj),
        Row::S(Id::BarraReloj24h),
        Row::S(Id::BarraSegundos),
        Row::S(Id::BarraFecha),
    ]},
];

// ------------------------------------------------------------ menú Inicio
static MENU: &[Group] = &[
    Group { title: "Tamaño", rows: &[
        Row::S(Id::MenuAncho),
        Row::S(Id::MenuAlto),
        Row::S(Id::MenuAltoElemento),
    ]},
    Group { title: "Contenido", rows: &[
        Row::S(Id::MenuBusqueda),
        Row::S(Id::MenuFrecuentes),
        Row::S(Id::MenuColumnaDerecha),
        Row::S(Id::MenuIconos),
        Row::S(Id::MenuIconosApps),
    ]},
    Group { title: "Acciones", rows: &[
        Row::S(Id::MenuEjecutar),
        Row::S(Id::MenuBloquear),
        Row::S(Id::MenuApagado),
    ]},
];

// ------------------------------------------------------------ aplicaciones
static APLICACIONES: &[Group] = &[
    Group { title: "Listas", rows: &[
        Row::S(Id::ListaAltoFila),
    ]},
    Group { title: "Explorador de archivos", rows: &[
        Row::S(Id::ExplPanelLateral),
        Row::S(Id::ExplIconos),
        Row::S(Id::ExplAutoRefresco),
        Row::S(Id::ExplConfirmarBorrado),
        Row::S(Id::ExplOcultos),
        Row::S(Id::ExplCarpetasPrimero),
        Row::S(Id::ExplColumnaFecha),
        Row::S(Id::ExplColumnaTipo),
        Row::S(Id::ExplColumnaTamano),
    ]},
    Group { title: "Administrador de tareas", rows: &[
        Row::S(Id::TaskmgrRefresco),
        Row::S(Id::TaskmgrIconos),
        Row::S(Id::PerfPorNucleo),
        Row::S(Id::PerfHistorial),
    ]},
    Group { title: "Ajustes de Windows", rows: &[
        Row::Link { glyph: "\u{E71D}", title: "Aplicaciones instaladas",
                    sub: "", act: Act::Url("ms-settings:appsfeatures") },
        Row::Link { glyph: "\u{E8FC}", title: "Aplicaciones predeterminadas",
                    sub: "", act: Act::Url("ms-settings:defaultapps") },
        Row::Link { glyph: "\u{E713}", title: "Panel de control",
                    sub: "Herramientas clásicas de Windows", act: Act::Url("control.exe") },
    ]},
];

// ------------------------------------------------------------ accesibilidad
static ACCESIBILIDAD: &[Group] = &[
    Group { title: "Tamaño y legibilidad", rows: &[
        Row::S(Id::EscalaTexto),
        Row::S(Id::EscalaInterfaz),
        Row::S(Id::ContrasteTexto),
        Row::S(Id::ListaAltoFila),
    ]},
    Group { title: "Efectos visuales", rows: &[
        Row::S(Id::Desenfoque),
        Row::S(Id::OpacidadVentanas),
    ]},
    Group { title: "Ajustes de Windows", rows: &[
        Row::Link { glyph: "\u{E7F8}", title: "Accesibilidad de Windows",
                    sub: "Lupa, narrador, contraste alto", act: Act::Url("ms-settings:easeofaccess") },
    ]},
];

// ------------------------------------------------------------ acerca de
static ACERCA: &[Group] = &[
    Group { title: "Especificaciones del dispositivo", rows: &[
        Row::Info(InfoKind::Equipo),
        Row::Info(InfoKind::Usuario),
        Row::Info(InfoKind::Cpu),
        Row::Info(InfoKind::Ram),
    ]},
    Group { title: "Especificaciones de Windows", rows: &[
        Row::Info(InfoKind::Windows),
    ]},
    Group { title: "OptimizedShell", rows: &[
        Row::Info(InfoKind::Archivo),
        Row::Link { glyph: "\u{E8A5}", title: "Abrir config.ini",
                    sub: "Editar los ajustes a mano", act: Act::Own("--abrir-ini") },
        Row::Link { glyph: "\u{E7E8}", title: "Usar OptimizedShell como shell",
                    sub: "Sustituye al Explorador al iniciar sesión", act: Act::Own("--install") },
        Row::Link { glyph: "\u{E7A7}", title: "Restaurar el Explorador de Windows",
                    sub: "Deshace el cambio anterior", act: Act::Own("--uninstall") },
    ]},
];

static NAV: &[NavPage] = &[
    NavPage { glyph: "\u{E80F}", title: "Inicio", sub: "", arg: "inicio", reset: None, groups: &[] },
    NavPage { glyph: "\u{E770}", title: "Sistema",
        sub: "Notificaciones, energía, rendimiento y herramientas",
        arg: "sistema", reset: Some(Page::Notificaciones), groups: SISTEMA },
    NavPage { glyph: "\u{E7F8}", title: "Pantalla",
        sub: "Resolución, frecuencia y orientación de los monitores",
        arg: "pantalla", reset: Some(Page::Escritorio), groups: PANTALLA },
    NavPage { glyph: "\u{E702}", title: "Bluetooth y dispositivos",
        sub: "Bluetooth, sonido, impresoras y periféricos",
        arg: "bluetooth", reset: None, groups: DISPOSITIVOS },
    NavPage { glyph: "\u{E701}", title: "Red e Internet",
        sub: "Wi-Fi, Ethernet y estado de la conexión",
        arg: "red", reset: None, groups: RED },
    NavPage { glyph: "\u{E771}", title: "Personalización",
        sub: "Color, tema, texto y efectos de toda la interfaz",
        arg: "personalizacion", reset: Some(Page::Apariencia), groups: PERSONALIZACION },
    NavPage { glyph: "\u{E7F4}", title: "Barra de tareas",
        sub: "Botón Inicio, ventanas, área de notificación y reloj",
        arg: "barra", reset: Some(Page::Barra), groups: BARRA },
    NavPage { glyph: "\u{E700}", title: "Menú Inicio",
        sub: "Tamaño, búsqueda y contenido del menú",
        arg: "menu", reset: Some(Page::Menu), groups: MENU },
    NavPage { glyph: "\u{E71D}", title: "Aplicaciones",
        sub: "Explorador, administrador de tareas y listas",
        arg: "aplicaciones", reset: Some(Page::Listas), groups: APLICACIONES },
    NavPage { glyph: "\u{E776}", title: "Accesibilidad",
        sub: "Tamaño del texto, contraste y efectos",
        arg: "accesibilidad", reset: None, groups: ACCESIBILIDAD },
    NavPage { glyph: "\u{E946}", title: "Acerca de",
        sub: "Información del equipo y de OptimizedShell",
        arg: "acerca", reset: None, groups: ACERCA },
];

/// Alias admitidos por compatibilidad con los argumentos antiguos.
fn page_for_arg(arg: &str) -> usize {
    let a = arg.to_lowercase();
    if let Some(i) = NAV.iter().position(|n| n.arg == a) {
        return i;
    }
    match a.as_str() {
        "notificaciones" | "rendimiento" => 1,
        "listas" | "explorador" => index_of("aplicaciones"),
        "fondo" | "apariencia" | "tema" => index_of("personalizacion"),
        "bandeja" | "barra de tareas" => index_of("barra"),
        "escritorio" => index_of("pantalla"),
        _ => 0,
    }
}

fn index_of(arg: &str) -> usize {
    NAV.iter().position(|n| n.arg == arg).unwrap_or(0)
}

// ============================================================ zonas sensibles
#[derive(Clone, Copy, PartialEq)]
enum Hit {
    None,
    Min,
    Max,
    Close,
    Nav(usize),
    Search,
    ClearSearch,
    Tile(usize),
    /// Fila de enlace (índice dentro de `links`).
    Link(usize),
    /// Miniatura de la galería de fondos.
    Wall(usize),
    WallBrowse,
    WallNone,
    /// Monitor, resolución y frecuencia.
    Display(usize),
    Resolution(usize),
    Rate(usize),
    Rotate(u32),
    Toggle(Id),
    Choice(Id, i32),
    Slider(Id, u8),
    Step(Id, i32),
    Swatch(usize),
    WinAccent,
    ResetPage,
    ResetAll,
    OpenIni,
}

enum BlockKind {
    Setting(&'static Item),
    Header(&'static str),
    Link(usize),
    Info(InfoKind),
    Gallery,
    Displays,
}

// ------------------------------------------------------- galería de fondos
// Miniaturas cacheadas por ruta: generarlas cuesta, y la página se repinta
// con cada movimiento del ratón.
thread_local! {
    static THUMBS: std::cell::RefCell<std::collections::HashMap<String, Option<ui::gdip::Image>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

fn thumb_size() -> (i32, i32) {
    (ui::s(132), ui::s(78))
}

/// Cuántas miniaturas caben por fila y cuántas filas hacen falta.
fn gallery_grid(cw: i32, count: usize) -> (i32, i32, i32) {
    let (tw, th) = thumb_size();
    let gap = ui::s(10);
    let cols = ((cw + gap) / (tw + gap)).max(1);
    let rows = ((count as i32 + cols - 1) / cols).max(1);
    (cols, rows, th + gap)
}

fn gallery_height(cw: i32, count: usize) -> i32 {
    let (_, rows, row_h) = gallery_grid(cw, count);
    // Las miniaturas más la fila de botones de abajo.
    rows * row_h + ui::s(46)
}

fn gallery_rect(cw: i32, origin: &RECT, i: usize) -> RECT {
    let (tw, th) = thumb_size();
    let gap = ui::s(10);
    let (cols, _, _) = gallery_grid(cw, i + 1);
    let c = i as i32 % cols;
    let r = i as i32 / cols;
    rect(origin.left + c * (tw + gap), origin.top + r * (th + gap), tw, th)
}

struct Block {
    kind: BlockKind,
    rect: RECT,
}

/// Datos del equipo, refrescados de vez en cuando (no en cada repintado).
#[derive(Default, Clone)]
struct SysInfo {
    equipo: String,
    usuario: String,
    windows: String,
    cpu: String,
    ram: String,
    red: String,
    bateria: String,
    bluetooth: String,
    audio: String,
    archivo: String,
}

struct State {
    hwnd: HWND,
    page: usize,
    hover: Hit,
    scroll: i32,
    content_h: i32,
    drag: Option<(Id, u8)>,
    search: String,
    searching: bool,
    info: SysInfo,
    /// Fondos de la galería y monitores: se leen una vez al abrir la página.
    walls: Vec<String>,
    displays: Vec<display::Display>,
    /// Monitor que se está ajustando y mensaje del último cambio.
    display_sel: usize,
    display_msg: Option<String>,
    // layout
    x0: i32,
    cw: i32,
    content_rc: RECT,
    nav: Vec<RECT>,
    search_rc: RECT,
    tiles: Vec<RECT>,
    blocks: Vec<Block>,
    links: Vec<(&'static str, &'static str, &'static str, Act)>,
    hits: Vec<(RECT, Hit)>,
    min_rc: RECT,
    max_rc: RECT,
    close_rc: RECT,
    reset_page_rc: RECT,
    reset_all_rc: RECT,
    open_ini_rc: RECT,
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
fn rw(r: &RECT) -> i32 {
    r.right - r.left
}
fn rh(r: &RECT) -> i32 {
    r.bottom - r.top
}

// ============================================ geometría de los controles
// Las mismas funciones se usan al medir (layout) y al dibujar (paint), para que
// lo que se ve y lo que se puede pulsar no se separen nunca.

fn has_help(it: &Item) -> bool {
    !it.help.is_empty()
}

fn body_top(r: &RECT, it: &Item) -> i32 {
    r.top + if has_help(it) { ui::s(48) } else { ui::s(34) }
}

fn switch_rect(r: &RECT) -> RECT {
    let w = ui::s(44);
    let h = ui::s(22);
    rect(r.right - ui::s(16) - w, r.top + (rh(r) - h) / 2, w, h)
}

fn track_rect(r: &RECT) -> RECT {
    rect(r.left + ui::s(16), r.bottom - ui::s(20), rw(r) - ui::s(32), ui::s(6))
}

fn stepper_rects(r: &RECT) -> (RECT, RECT) {
    let b = ui::s(24);
    let y = r.top + ui::s(10);
    (rect(r.right - ui::s(16) - b * 2 - ui::s(6), y, b, b), rect(r.right - ui::s(16) - b, y, b, b))
}

fn chip_rects(r: &RECT, opts: &[&str], it: &Item) -> Vec<RECT> {
    let x0 = r.left + ui::s(16);
    let max_x = r.right - ui::s(16);
    let h = ui::s(28);
    let gap = ui::s(8);
    let mut x = x0;
    let mut y = body_top(r, it);
    let mut out = Vec::with_capacity(opts.len());
    for o in opts {
        let w = ui::s(22) + o.chars().count() as i32 * ui::s(7);
        if x + w > max_x && x > x0 {
            x = x0;
            y += h + gap;
        }
        out.push(rect(x, y, w, h));
        x += w + gap;
    }
    out
}

fn card_rects(r: &RECT) -> (RECT, RECT) {
    let gap = ui::s(16);
    let w = (rw(r) - gap) / 2;
    let h = ui::s(150);
    let y = r.top + ui::s(10);
    (rect(r.left, y, w, h), rect(r.left + w + gap, y, w, h))
}

fn swatch_rects(r: &RECT) -> Vec<RECT> {
    let sw = ui::s(34);
    let gap = ui::s(10);
    let x0 = r.left + ui::s(16);
    let max_x = r.right - ui::s(16);
    let mut x = x0;
    let mut y = r.top + ui::s(38);
    let mut out = Vec::new();
    for _ in 0..ACCENT_PRESETS.len() {
        if x + sw > max_x && x > x0 {
            x = x0;
            y += sw + gap;
        }
        out.push(rect(x, y, sw, sw));
        x += sw + gap;
    }
    out
}

fn color_tracks(r: &RECT) -> [RECT; 3] {
    let y0 = swatch_rects(r).last().map(|s| s.bottom).unwrap_or(r.top) + ui::s(26);
    let w = rw(r) - ui::s(32) - ui::s(60);
    let step = ui::s(34);
    [
        rect(r.left + ui::s(16) + ui::s(28), y0, w, ui::s(6)),
        rect(r.left + ui::s(16) + ui::s(28), y0 + step, w, ui::s(6)),
        rect(r.left + ui::s(16) + ui::s(28), y0 + step * 2, w, ui::s(6)),
    ]
}

fn winaccent_rect(r: &RECT) -> RECT {
    let t = color_tracks(r)[2];
    rect(r.left + ui::s(16), t.bottom + ui::s(18), ui::s(236), ui::s(32))
}

/// Alto que ocupa el bloque de un ajuste (el ancho se toma de `probe`).
fn block_height(it: &Item, cw: i32) -> i32 {
    let probe = rect(0, 0, cw, 0);
    match it.kind {
        Kind::Bool => ui::s(if has_help(it) { 62 } else { 50 }),
        Kind::Int { .. } => ui::s(if has_help(it) { 92 } else { 76 }),
        Kind::Choice(opts) => {
            let rs = chip_rects(&probe, opts, it);
            rs.last().map(|r| r.bottom).unwrap_or(0) + ui::s(14)
        }
        Kind::Cards(_) => ui::s(172),
        Kind::Color => winaccent_rect(&probe).bottom + ui::s(14),
    }
}

fn link_height() -> i32 {
    ui::s(60)
}
fn header_height() -> i32 {
    ui::s(38)
}

/// Botones «Examinar...» y «Sin fondo», bajo las miniaturas.
fn gallery_buttons(st: &State, r: &RECT) -> (RECT, RECT) {
    let (_, rows, row_h) = gallery_grid(st.cw, st.walls.len());
    let y = r.top + rows * row_h + ui::s(4);
    (rect(r.left, y, ui::s(150), ui::s(32)),
     rect(r.left + ui::s(158), y, ui::s(130), ui::s(32)))
}

// ------------------------------------------------------------ pantalla
fn display_row_h() -> i32 {
    ui::s(34)
}

/// Alto del bloque de monitores: tarjeta del monitor + resoluciones +
/// frecuencias + orientación.
fn displays_height(st: &State) -> i32 {
    if st.displays.is_empty() {
        return ui::s(60);
    }
    let sel = st.display_sel.min(st.displays.len() - 1);
    let d = &st.displays[sel];
    let chips_rows = |n: usize, per: usize| ((n + per - 1) / per).max(1) as i32;
    let res_rows = chips_rows(d.resolutions().len().min(24), 4);
    let rate_rows = chips_rows(d.rates_for(d.current.width, d.current.height).len().min(12), 6);
    ui::s(74) // tarjeta del monitor
        + if st.displays.len() > 1 { display_row_h() + ui::s(8) } else { 0 }
        + ui::s(26) + res_rows * (ui::s(30) + ui::s(8))   // resoluciones
        + ui::s(26) + rate_rows * (ui::s(30) + ui::s(8))  // frecuencias
        + ui::s(26) + ui::s(30)                            // orientación
        + ui::s(26)                                        // mensaje
}

/// Rectángulos sensibles del bloque de monitores, en el mismo orden en que se
/// dibujan (medir y pintar comparten esta función).
fn displays_hits(st: &State, r: &RECT) -> Vec<(Hit, RECT)> {
    let mut out = Vec::new();
    if st.displays.is_empty() {
        return out;
    }
    let sel = st.display_sel.min(st.displays.len() - 1);
    let d = &st.displays[sel];
    let mut y = r.top + ui::s(74);

    if st.displays.len() > 1 {
        let w = (rw(r) / st.displays.len() as i32).min(ui::s(220));
        for i in 0..st.displays.len() {
            out.push((Hit::Display(i), rect(r.left + i as i32 * (w + ui::s(8)), y, w, display_row_h())));
        }
        y += display_row_h() + ui::s(8);
    }

    // Resoluciones (4 por fila).
    y += ui::s(26);
    let res = d.resolutions();
    let cw = (rw(r) - ui::s(24)) / 4;
    for (i, _) in res.iter().take(24).enumerate() {
        let c = i as i32 % 4;
        let row = i as i32 / 4;
        out.push((Hit::Resolution(i),
                  rect(r.left + c * (cw + ui::s(8)), y + row * (ui::s(30) + ui::s(8)), cw, ui::s(30))));
    }
    let res_rows = ((res.len().min(24) + 3) / 4).max(1) as i32;
    y += res_rows * (ui::s(30) + ui::s(8));

    // Frecuencias (6 por fila).
    y += ui::s(26);
    let rates = d.rates_for(d.current.width, d.current.height);
    let rwth = (rw(r) - ui::s(40)) / 6;
    for (i, _) in rates.iter().take(12).enumerate() {
        let c = i as i32 % 6;
        let row = i as i32 / 6;
        out.push((Hit::Rate(i),
                  rect(r.left + c * (rwth + ui::s(8)), y + row * (ui::s(30) + ui::s(8)), rwth, ui::s(30))));
    }
    let rate_rows = ((rates.len().min(12) + 5) / 6).max(1) as i32;
    y += rate_rows * (ui::s(30) + ui::s(8));

    // Orientación.
    y += ui::s(26);
    for (i, deg) in [0u32, 90, 180, 270].iter().enumerate() {
        out.push((Hit::Rotate(*deg),
                  rect(r.left + i as i32 * (ui::s(104) + ui::s(8)), y, ui::s(104), ui::s(30))));
    }
    out
}

// ============================================================ información
fn computer_name() -> String {
    unsafe {
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        if windows_sys::Win32::System::SystemInformation::GetComputerNameExW(
            windows_sys::Win32::System::SystemInformation::ComputerNameDnsHostname,
            buf.as_mut_ptr(), &mut len) != 0
        {
            return String::from_utf16_lossy(&buf[..len as usize]);
        }
    }
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Equipo".into())
}

fn reg_string(sub: &str, name: &str) -> String {
    use windows_sys::Win32::System::Registry::*;
    unsafe {
        let mut k: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, ui::wide(sub).as_ptr(), 0, KEY_QUERY_VALUE, &mut k) != 0 {
            return String::new();
        }
        let mut buf = [0u16; 256];
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

/// Valor numérico del registro (UBR y compañía se guardan como REG_DWORD).
fn reg_dword(sub: &str, name: &str) -> Option<u32> {
    use windows_sys::Win32::System::Registry::*;
    unsafe {
        let mut k: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, ui::wide(sub).as_ptr(), 0, KEY_QUERY_VALUE, &mut k) != 0 {
            return None;
        }
        let mut v = 0u32;
        let mut len = 4u32;
        let mut kind = 0u32;
        let ok = RegQueryValueExW(k, ui::wide(name).as_ptr(), std::ptr::null_mut(), &mut kind,
                                  &mut v as *mut u32 as *mut u8, &mut len) == 0;
        RegCloseKey(k);
        if ok { Some(v) } else { None }
    }
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

fn collect_info() -> SysInfo {
    const WINVER: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
    let cpu = perf::cpu_info();
    let mut mem: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    mem.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    unsafe { GlobalMemoryStatusEx(&mut mem) };

    let product = reg_string(WINVER, "ProductName");
    let display = reg_string(WINVER, "DisplayVersion");
    let build = reg_string(WINVER, "CurrentBuild");
    let ubr = reg_dword(WINVER, "UBR").map(|v| v.to_string()).unwrap_or_default();
    let windows = {
        let mut s = if product.is_empty() { "Windows".to_string() } else { product };
        if !display.is_empty() {
            s.push_str(&format!(" · versión {}", display));
        }
        if !build.is_empty() {
            s.push_str(&format!(" · compilación {}", build));
            if !ubr.is_empty() {
                s.push_str(&format!(".{}", ubr));
            }
        }
        s
    };

    let net = match sysinfo::network() {
        Net::Off => "Sin conexión".to_string(),
        Net::Ethernet { mbps } if mbps > 0 => format!("Ethernet conectado · {} Mbps", mbps),
        Net::Ethernet { .. } => "Ethernet conectado".to_string(),
        Net::Wifi { ssid, quality } => format!("Wi-Fi: {} · señal {} %", ssid, quality),
    };
    let b = sysinfo::battery();
    let bateria = if !b.present {
        "Sin batería (equipo de escritorio)".to_string()
    } else if b.charging {
        format!("{} %, conectado a la corriente", b.percent.max(0))
    } else if b.seconds_left > 0 {
        format!("{} %, quedan {} h {:02} min", b.percent.max(0), b.seconds_left / 3600,
                (b.seconds_left % 3600) / 60)
    } else {
        format!("{} %, con batería", b.percent.max(0))
    };
    let bluetooth = match sysinfo::bluetooth() {
        Some(true) => "Radio Bluetooth disponible".to_string(),
        Some(false) => "Sin radio Bluetooth activa".to_string(),
        None => "Este equipo no tiene Bluetooth".to_string(),
    };
    let audio = match sysinfo::volume() {
        Some((v, true)) => format!("Silenciado (volumen {} %)", v),
        Some((v, false)) => format!("Volumen al {} %", v),
        None => "Sin dispositivo de salida".to_string(),
    };

    SysInfo {
        equipo: computer_name(),
        usuario: std::env::var("USERNAME").unwrap_or_else(|_| "Usuario".into()),
        windows,
        cpu: if cpu.name.is_empty() {
            format!("{} núcleos lógicos", cpu.logical)
        } else {
            format!("{} · {} núcleos ({} físicos)", cpu.name, cpu.logical, cpu.physical)
        },
        ram: format!("{} instalados · {} en uso", gb(mem.ullTotalPhys),
                     gb(mem.ullTotalPhys - mem.ullAvailPhys)),
        red: net,
        bateria,
        bluetooth,
        audio,
        archivo: config::config_file().to_string_lossy().to_string(),
    }
}

fn info_row(info: &SysInfo, kind: InfoKind) -> (&'static str, &'static str, String) {
    match kind {
        InfoKind::Equipo => ("\u{E977}", "Nombre del equipo", info.equipo.clone()),
        InfoKind::Usuario => ("\u{E77B}", "Usuario", info.usuario.clone()),
        InfoKind::Windows => ("\u{E770}", "Edición de Windows", info.windows.clone()),
        InfoKind::Cpu => ("\u{E950}", "Procesador", info.cpu.clone()),
        InfoKind::Ram => ("\u{EEA0}", "Memoria RAM", info.ram.clone()),
        InfoKind::Red => ("\u{E704}", "Conexión actual", info.red.clone()),
        InfoKind::Bateria => ("\u{E83E}", "Batería", info.bateria.clone()),
        InfoKind::Bluetooth => ("\u{E702}", "Bluetooth", info.bluetooth.clone()),
        InfoKind::Audio => ("\u{E767}", "Salida de audio", info.audio.clone()),
        InfoKind::Archivo => ("\u{E8A5}", "Archivo de configuración", info.archivo.clone()),
    }
}

// ============================================================ layout
/// Ajustes que coinciden con el texto buscado, en todas las páginas.
fn search_results(q: &str) -> Vec<&'static Item> {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    config::ITEMS
        .iter()
        .filter(|it| {
            it.label.to_lowercase().contains(&q) || it.help.to_lowercase().contains(&q)
        })
        .collect()
}

fn layout(st: &mut State) {
    let mut rc = er();
    unsafe { GetClientRect(st.hwnd, &mut rc) };
    let w = rc.right;
    let h = rc.bottom;
    let cap = ui::caption_h();
    let ch = ui::gdip::chrome_rects(w);
    st.min_rc = rect(ch.min.0, ch.min.1, ch.min.2, ch.min.3);
    st.max_rc = rect(ch.max.0, ch.max.1, ch.max.2, ch.max.3);
    st.close_rc = rect(ch.close.0, ch.close.1, ch.close.2, ch.close.3);

    let nav_w = ui::s(244);
    st.search_rc = rect(ui::s(12), cap + ui::s(12), nav_w - ui::s(24), ui::s(34));
    let nav_top = st.search_rc.bottom + ui::s(12);
    st.nav = (0..NAV.len())
        .map(|i| rect(ui::s(10), nav_top + i as i32 * ui::s(40), nav_w - ui::s(20), ui::s(36)))
        .collect();

    st.x0 = nav_w + ui::s(28);
    st.cw = (w - st.x0 - ui::s(28)).max(ui::s(240));
    let footer_h = ui::s(58);
    let top = cap + ui::s(78);
    st.content_rc = rect(st.x0, top, st.cw, (h - top - footer_h).max(ui::s(80)));

    let by = h - ui::s(46);
    let bw = ui::s(170);
    st.reset_page_rc = rect(st.x0, by, bw, ui::s(32));
    st.reset_all_rc = rect(st.x0 + bw + ui::s(10), by, ui::s(140), ui::s(32));
    st.open_ini_rc = rect(st.x0 + bw + ui::s(160), by, ui::s(150), ui::s(32));

    st.blocks.clear();
    st.links.clear();
    st.tiles.clear();
    st.hits.clear();
    st.hits.push((st.close_rc, Hit::Close));
    st.hits.push((st.max_rc, Hit::Max));
    st.hits.push((st.min_rc, Hit::Min));
    st.hits.push((st.search_rc, Hit::Search));
    for (i, r) in st.nav.iter().enumerate() {
        st.hits.push((*r, Hit::Nav(i)));
    }
    st.hits.push((st.reset_page_rc, Hit::ResetPage));
    st.hits.push((st.reset_all_rc, Hit::ResetAll));
    st.hits.push((st.open_ini_rc, Hit::OpenIni));

    let mut y = 0;
    let gap = ui::s(10);

    if !st.search.trim().is_empty() {
        // Resultados de la búsqueda: una lista plana de ajustes.
        st.blocks.push(Block { kind: BlockKind::Header("Resultados"), rect: rect(st.x0, y, st.cw, header_height()) });
        y += header_height();
        for it in search_results(&st.search) {
            let hgt = block_height(it, st.cw);
            st.blocks.push(Block { kind: BlockKind::Setting(it), rect: rect(st.x0, y, st.cw, hgt) });
            y += hgt + gap;
        }
    } else if st.page == 0 {
        // Página de inicio: tarjeta del equipo y accesos a las categorías.
        let card_h = ui::s(96);
        y += card_h + ui::s(18);
        let cols = if st.cw > ui::s(680) { 2 } else { 1 };
        let tgap = ui::s(12);
        let tw = (st.cw - tgap * (cols - 1)) / cols;
        let th = ui::s(72);
        for i in 1..NAV.len() {
            let k = (i - 1) as i32;
            let r = rect(st.x0 + (k % cols) * (tw + tgap), y + (k / cols) * (th + tgap), tw, th);
            st.tiles.push(r);
        }
        let rows = ((NAV.len() - 1) as i32 + cols - 1) / cols;
        y += rows * (th + tgap);
    } else {
        for grp in NAV[st.page].groups {
            if !grp.title.is_empty() {
                st.blocks.push(Block { kind: BlockKind::Header(grp.title), rect: rect(st.x0, y, st.cw, header_height()) });
                y += header_height();
            }
            for row in grp.rows {
                match row {
                    Row::S(id) => {
                        let it = config::item(*id);
                        let hgt = block_height(it, st.cw);
                        st.blocks.push(Block { kind: BlockKind::Setting(it), rect: rect(st.x0, y, st.cw, hgt) });
                        y += hgt + gap;
                    }
                    Row::Link { glyph, title, sub, act } => {
                        let idx = st.links.len();
                        st.links.push((glyph, title, sub, *act));
                        st.blocks.push(Block { kind: BlockKind::Link(idx), rect: rect(st.x0, y, st.cw, link_height()) });
                        y += link_height() + gap;
                    }
                    Row::Info(kind) => {
                        st.blocks.push(Block { kind: BlockKind::Info(*kind), rect: rect(st.x0, y, st.cw, link_height()) });
                        y += link_height() + gap;
                    }
                    Row::Gallery => {
                        let h = gallery_height(st.cw, st.walls.len());
                        st.blocks.push(Block { kind: BlockKind::Gallery, rect: rect(st.x0, y, st.cw, h) });
                        y += h + gap;
                    }
                    Row::Displays => {
                        let h = displays_height(st);
                        st.blocks.push(Block { kind: BlockKind::Displays, rect: rect(st.x0, y, st.cw, h) });
                        y += h + gap;
                    }
                }
            }
            y += ui::s(8);
        }
    }
    st.content_h = y;
    let max_scroll = (st.content_h - rh(&st.content_rc)).max(0);
    st.scroll = st.scroll.clamp(0, max_scroll);

    // Zonas sensibles, ya desplazadas.
    let dy = st.content_rc.top - st.scroll;
    for (i, r) in st.tiles.iter().enumerate() {
        st.hits.push((rect(r.left, r.top + dy, rw(r), rh(r)), Hit::Tile(i + 1)));
    }
    for b in st.blocks.iter() {
        let r = rect(b.rect.left, b.rect.top + dy, rw(&b.rect), rh(&b.rect));
        match &b.kind {
            BlockKind::Header(_) | BlockKind::Info(_) => {}
            BlockKind::Link(i) => st.hits.push((r, Hit::Link(*i))),
            BlockKind::Gallery => {
                for i in 0..st.walls.len() {
                    st.hits.push((gallery_rect(st.cw, &r, i), Hit::Wall(i)));
                }
                let (br, nr) = gallery_buttons(st, &r);
                st.hits.push((br, Hit::WallBrowse));
                st.hits.push((nr, Hit::WallNone));
            }
            BlockKind::Displays => {
                for (h, hr) in displays_hits(st, &r) {
                    st.hits.push((hr, h));
                }
            }
            BlockKind::Setting(it) => {
                let it = *it;
                match it.kind {
                    Kind::Bool => st.hits.push((r, Hit::Toggle(it.id))),
                    Kind::Int { .. } => {
                        let (m, p) = stepper_rects(&r);
                        st.hits.push((m, Hit::Step(it.id, -1)));
                        st.hits.push((p, Hit::Step(it.id, 1)));
                        let t = track_rect(&r);
                        st.hits.push((rect(t.left - ui::s(8), t.top - ui::s(12), rw(&t) + ui::s(16), ui::s(28)),
                                      Hit::Slider(it.id, 0)));
                    }
                    Kind::Choice(opts) => {
                        for (i, cr) in chip_rects(&r, opts, it).into_iter().enumerate() {
                            st.hits.push((cr, Hit::Choice(it.id, i as i32)));
                        }
                    }
                    Kind::Cards(_) => {
                        let (a, b2) = card_rects(&r);
                        st.hits.push((a, Hit::Choice(it.id, 0)));
                        st.hits.push((b2, Hit::Choice(it.id, 1)));
                    }
                    Kind::Color => {
                        for (i, sr) in swatch_rects(&r).into_iter().enumerate() {
                            st.hits.push((sr, Hit::Swatch(i)));
                        }
                        for (i, t) in color_tracks(&r).iter().enumerate() {
                            st.hits.push((rect(t.left - ui::s(8), t.top - ui::s(12), rw(t) + ui::s(16), ui::s(28)),
                                          Hit::Slider(it.id, i as u8 + 1)));
                        }
                        st.hits.push((winaccent_rect(&r), Hit::WinAccent));
                    }
                }
            }
        }
    }
}

// ============================================================ aplicar
fn commit(st: &mut State, c: &Config, persist: bool) {
    ui::apply_config(c, persist);
    ui::broadcast_config_changed();
    layout(st);
    unsafe { InvalidateRect(st.hwnd, std::ptr::null(), 0) };
}

fn windows_accent() -> u32 {
    unsafe {
        let mut argb = 0u32;
        let mut opaque = 0i32;
        if windows_sys::Win32::Graphics::Dwm::DwmGetColorizationColor(&mut argb, &mut opaque) == 0 {
            let r = ((argb >> 16) & 0xFF) as u8;
            let g = ((argb >> 8) & 0xFF) as u8;
            let b = (argb & 0xFF) as u8;
            return theme::colorref(r, g, b);
        }
    }
    theme::colorref(59, 130, 246)
}

// ============================================================ pintado
fn paint(st: &State) {
    let cfg = config::current();
    window::paint_buffered(st.hwnd, |g: &Gfx, w, h| {
        let s = theme::style();
        let cap = ui::caption_h();
        let nav_w = ui::s(244);
        // ---- marco
        g.clear(theme::argb(0, 0, 0, 0));
        g.rounded(0.0, 0.0, w as f32, h as f32, ui::sf(10.0), Some(s.window_bg), Some((s.card_border, 1.0)));
        g.fill_rect(0, 0, w, cap, s.title_bg);
        g.hline(0, w, cap - 1, s.separator);
        let ic = ui::s(20);
        g.rounded(ui::s(14) as f32, ((cap - ic) / 2) as f32, ic as f32, ic as f32, ui::sf(4.0),
                  Some(s.accent_solid), None);
        g.text_in("\u{E713}", ui::s(14) as f32, ((cap - ic) / 2) as f32, ic as f32, ic as f32, 12.0, false,
                  s.on_accent, Align::Center, Align::Center, true);
        g.text_in("Configuración", ui::s(42) as f32, 0.0, (w - ui::s(260)) as f32, cap as f32, 14.0, true,
                  s.text, Align::Near, Align::Center, false);
        chrome_btn(g, &s, st.min_rc, "\u{E921}", st.hover == Hit::Min, false);
        chrome_btn(g, &s, st.max_rc, if unsafe { IsZoomed(st.hwnd) } != 0 { "\u{E923}" } else { "\u{E922}" },
                   st.hover == Hit::Max, false);
        chrome_btn(g, &s, st.close_rc, "\u{E8BB}", st.hover == Hit::Close, true);

        // ---- barra lateral
        g.fill_rect(0, cap, nav_w, h - cap, s.nav_bg);
        // buscador
        let sr = st.search_rc;
        let focus = st.searching;
        g.rounded(sr.left as f32, sr.top as f32, rw(&sr) as f32, rh(&sr) as f32, ui::sf(6.0),
                  Some(s.control_bg), Some((if focus { s.accent_solid } else { s.control_border },
                                            if focus { 1.6 } else { 1.0 })));
        g.text_in("\u{E721}", (sr.left + ui::s(8)) as f32, sr.top as f32, ui::s(20) as f32, rh(&sr) as f32,
                  10.0, false, s.text_dim, Align::Center, Align::Center, true);
        let (txt, col) = if st.search.is_empty() {
            ("Buscar un valor".to_string(), s.text_faint)
        } else {
            (st.search.clone(), s.text)
        };
        g.text_in(&txt, (sr.left + ui::s(32)) as f32, sr.top as f32, (rw(&sr) - ui::s(56)) as f32,
                  rh(&sr) as f32, 11.5, false, col, Align::Near, Align::Center, false);
        if !st.search.is_empty() {
            g.text_in("\u{E8BB}", (sr.right - ui::s(24)) as f32, sr.top as f32, ui::s(20) as f32,
                      rh(&sr) as f32, 8.0, false, s.text_dim, Align::Center, Align::Center, true);
        }
        // categorías
        for (i, r) in st.nav.iter().enumerate() {
            let active = i == st.page && st.search.trim().is_empty();
            if active {
                g.rounded(r.left as f32, r.top as f32, rw(r) as f32, rh(r) as f32, ui::sf(6.0),
                          Some(s.row_selected), None);
                g.rounded(r.left as f32, (r.top + ui::s(9)) as f32, ui::s(3) as f32,
                          (rh(r) - ui::s(18)) as f32, ui::sf(1.5), Some(s.accent), None);
            } else if st.hover == Hit::Nav(i) {
                g.rounded(r.left as f32, r.top as f32, rw(r) as f32, rh(r) as f32, ui::sf(6.0),
                          Some(s.row_hover), None);
            }
            let col = if active { s.accent } else { s.text_dim };
            g.text_in(NAV[i].glyph, (r.left + ui::s(10)) as f32, r.top as f32, ui::s(24) as f32,
                      rh(r) as f32, 12.0, false, col, Align::Center, Align::Center, true);
            g.text_in(NAV[i].title, (r.left + ui::s(44)) as f32, r.top as f32, (rw(r) - ui::s(48)) as f32,
                      rh(r) as f32, 12.5, false, s.text, Align::Near, Align::Center, false);
        }

        // ---- cabecera de la página
        let searching = !st.search.trim().is_empty();
        let (title, sub) = if searching {
            (format!("Resultados de «{}»", st.search.trim()),
             format!("{} ajustes coinciden", search_results(&st.search).len()))
        } else if st.page == 0 {
            ("Configuración".to_string(), format!("{} · {}", st.info.equipo, st.info.usuario))
        } else {
            (NAV[st.page].title.to_string(), NAV[st.page].sub.to_string())
        };
        g.text_in(&title, st.x0 as f32, (cap + ui::s(16)) as f32, st.cw as f32, ui::s(32) as f32, 20.0, true,
                  s.text, Align::Near, Align::Center, false);
        g.text_in(&sub, st.x0 as f32, (cap + ui::s(46)) as f32, st.cw as f32, ui::s(20) as f32, 11.5, false,
                  s.text_dim, Align::Near, Align::Center, false);

        // ---- contenido desplazable
        let cr = st.content_rc;
        g.set_clip(cr.left, cr.top, rw(&cr), rh(&cr));
        let dy = cr.top - st.scroll;
        if st.page == 0 && !searching {
            paint_home(g, &s, st, dy);
        }
        for b in st.blocks.iter() {
            let r = rect(b.rect.left, b.rect.top + dy, rw(&b.rect), rh(&b.rect));
            if r.bottom < cr.top || r.top > cr.bottom {
                continue;
            }
            match &b.kind {
                BlockKind::Header(t) => {
                    g.text_in(t, r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32, 12.5, true,
                              s.text, Align::Near, Align::Far, false);
                }
                BlockKind::Setting(it) => paint_block(g, &s, st, &cfg, it, &r),
                BlockKind::Link(i) => {
                    let (glyph, t, sub, _) = st.links[*i];
                    paint_link(g, &s, &r, glyph, t, sub, st.hover == Hit::Link(*i), true);
                }
                BlockKind::Info(kind) => {
                    let (glyph, t, value) = info_row(&st.info, *kind);
                    paint_link(g, &s, &r, glyph, t, &value, false, false);
                }
                BlockKind::Gallery => paint_gallery(g, &s, st, &r),
                BlockKind::Displays => paint_displays(g, &s, st, &r),
            }
        }
        g.reset_clip();
        let max_scroll = (st.content_h - rh(&cr)).max(0);
        if max_scroll > 0 {
            let bar_h = (rh(&cr) * rh(&cr) / st.content_h.max(1)).max(ui::s(30));
            let by = cr.top + (rh(&cr) - bar_h) * st.scroll / max_scroll;
            g.rounded((cr.right + ui::s(8)) as f32, by as f32, ui::s(4) as f32, bar_h as f32, ui::sf(2.0),
                      Some(s.track), None);
        }

        // ---- pie
        g.hline(nav_w, w, h - ui::s(58), s.separator);
        let resettable = NAV[st.page].reset.is_some() && !searching;
        if resettable {
            draw_button(g, &s, st.reset_page_rc, "Restablecer esta página", st.hover == Hit::ResetPage, false);
        }
        draw_button(g, &s, st.reset_all_rc, "Restablecer todo", st.hover == Hit::ResetAll, false);
        draw_button(g, &s, st.open_ini_rc, "Abrir config.ini", st.hover == Hit::OpenIni, false);
    });
}

/// Tarjeta del equipo y mosaicos de categoría de la página de inicio.
fn paint_home(g: &Gfx, s: &theme::Style, st: &State, dy: i32) {
    let card = rect(st.x0, dy, st.cw, ui::s(96));
    g.rounded(card.left as f32, card.top as f32, rw(&card) as f32, rh(&card) as f32, ui::sf(10.0),
              Some(s.card_bg), Some((s.card_border, 1.0)));
    let av = ui::s(56);
    let ax = card.left + ui::s(18);
    let ay = card.top + (rh(&card) - av) / 2;
    g.fill_circle((ax + av / 2) as f32, (ay + av / 2) as f32, (av / 2) as f32, s.accent_solid);
    let initial = st.info.usuario.chars().next().unwrap_or('U').to_uppercase().to_string();
    g.text_in(&initial, ax as f32, ay as f32, av as f32, av as f32, 20.0, true, s.on_accent, Align::Center,
              Align::Center, false);
    let tx = ax + av + ui::s(16);
    g.text_in(&st.info.usuario, tx as f32, (card.top + ui::s(20)) as f32, (rw(&card) - ui::s(120)) as f32,
              ui::s(24) as f32, 15.0, true, s.text, Align::Near, Align::Center, false);
    g.text_in(&st.info.equipo, tx as f32, (card.top + ui::s(44)) as f32, (rw(&card) - ui::s(120)) as f32,
              ui::s(18) as f32, 11.5, false, s.text_dim, Align::Near, Align::Center, false);
    g.text_in(&st.info.windows, tx as f32, (card.top + ui::s(62)) as f32, (rw(&card) - ui::s(120)) as f32,
              ui::s(18) as f32, 10.5, false, s.text_faint, Align::Near, Align::Center, false);

    for (k, r) in st.tiles.iter().enumerate() {
        let i = k + 1;
        let r = rect(r.left, r.top + dy, rw(r), rh(r));
        let hov = st.hover == Hit::Tile(i);
        g.rounded(r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32, ui::sf(8.0),
                  Some(if hov { s.control_hover } else { s.card_bg }), Some((s.card_border, 1.0)));
        g.text_in(NAV[i].glyph, (r.left + ui::s(12)) as f32, r.top as f32, ui::s(30) as f32, rh(&r) as f32,
                  14.0, false, s.accent, Align::Center, Align::Center, true);
        g.text_in(NAV[i].title, (r.left + ui::s(50)) as f32, (r.top + ui::s(14)) as f32,
                  (rw(&r) - ui::s(70)) as f32, ui::s(20) as f32, 12.5, true, s.text, Align::Near,
                  Align::Center, false);
        g.text_in(NAV[i].sub, (r.left + ui::s(50)) as f32, (r.top + ui::s(34)) as f32,
                  (rw(&r) - ui::s(70)) as f32, ui::s(18) as f32, 10.5, false, s.text_dim, Align::Near,
                  Align::Center, false);
        g.text_in("\u{E76C}", (r.right - ui::s(26)) as f32, r.top as f32, ui::s(18) as f32, rh(&r) as f32,
                  8.5, false, s.text_faint, Align::Center, Align::Center, true);
    }
}

/// Galería de fondos: miniaturas del tamaño de la pantalla, con el actual marcado.
fn paint_gallery(g: &Gfx, s: &theme::Style, st: &State, r: &RECT) {
    let current = crate::desktop::wallpaper_path().to_lowercase();
    let (tw, th) = thumb_size();
    for (i, path) in st.walls.iter().enumerate() {
        let tr = gallery_rect(st.cw, r, i);
        if tr.bottom < st.content_rc.top - th || tr.top > st.content_rc.bottom + th {
            continue; // fuera de la vista: ni se carga la miniatura
        }
        let sel = path.to_lowercase() == current;
        let hov = st.hover == Hit::Wall(i);
        // Miniatura (se genera la primera vez que se ve y se queda en caché).
        THUMBS.with(|c| {
            let mut map = c.borrow_mut();
            let entry = map.entry(path.clone()).or_insert_with(|| {
                ui::gdip::Image::thumbnail(path, (tw * 2) as u32, (th * 2) as u32)
            });
            match entry {
                Some(img) => g.draw_image_cover(img, tr.left, tr.top, tw, th),
                None => {
                    g.rounded(tr.left as f32, tr.top as f32, tw as f32, th as f32, ui::sf(6.0),
                              Some(s.control_bg), None);
                    g.text_in("\u{E91B}", tr.left as f32, tr.top as f32, tw as f32, th as f32, 14.0,
                              false, s.text_faint, Align::Center, Align::Center, true);
                }
            }
        });
        // Borde y marca de seleccionado.
        g.rounded(tr.left as f32, tr.top as f32, tw as f32, th as f32, ui::sf(6.0), None,
                  Some((if sel { s.accent_solid } else if hov { s.text_dim } else { s.card_border },
                        if sel { 2.5 } else { 1.0 })));
        if sel {
            let bx = tr.right - ui::s(16);
            let by = tr.top + ui::s(16);
            g.fill_circle(bx as f32, by as f32, ui::sf(10.0), s.accent_solid);
            g.check(bx as f32, by as f32, ui::sf(11.0), s.on_accent, ui::sf(1.8));
        }
    }
    if st.walls.is_empty() {
        g.text_in("No se han encontrado imágenes en las carpetas de fondos.", r.left as f32,
                  r.top as f32, rw(r) as f32, ui::s(40) as f32, 12.0, false, s.text_faint,
                  Align::Near, Align::Center, false);
    }
    let (br, nr) = gallery_buttons(st, r);
    draw_button(g, s, br, "Examinar...", st.hover == Hit::WallBrowse, false);
    draw_button(g, s, nr, "Quitar el fondo", st.hover == Hit::WallNone, false);
}

/// Monitores: tarjeta del seleccionado, resoluciones, frecuencias y giro.
fn paint_displays(g: &Gfx, s: &theme::Style, st: &State, r: &RECT) {
    if st.displays.is_empty() {
        g.text_in("No se han detectado monitores.", r.left as f32, r.top as f32, rw(r) as f32,
                  ui::s(40) as f32, 12.5, false, s.text_faint, Align::Near, Align::Center, false);
        return;
    }
    let sel = st.display_sel.min(st.displays.len() - 1);
    let d = &st.displays[sel];

    // Tarjeta con el monitor actual.
    let card = rect(r.left, r.top, rw(r), ui::s(66));
    g.rounded(card.left as f32, card.top as f32, rw(&card) as f32, rh(&card) as f32, ui::sf(8.0),
              Some(s.card_bg), Some((s.card_border, 1.0)));
    g.text_in("\u{E7F8}", (card.left + ui::s(14)) as f32, card.top as f32, ui::s(30) as f32,
              rh(&card) as f32, 14.0, false, s.accent, Align::Center, Align::Center, true);
    let title = if d.primary { format!("{} (principal)", d.name) } else { d.name.clone() };
    g.text_in(&title, (card.left + ui::s(52)) as f32, (card.top + ui::s(12)) as f32,
              (rw(&card) - ui::s(70)) as f32, ui::s(22) as f32, 13.0, true, s.text, Align::Near,
              Align::Center, false);
    g.text_in(&format!("{} · {} bits · {}°", d.current.full_label(), d.current.bpp, d.rotation),
              (card.left + ui::s(52)) as f32, (card.top + ui::s(34)) as f32,
              (rw(&card) - ui::s(70)) as f32, ui::s(20) as f32, 11.5, false, s.text_dim, Align::Near,
              Align::Center, false);

    let hits = displays_hits(st, r);
    let chip = |g: &Gfx, rr: &RECT, label: &str, on: bool, hov: bool| {
        g.rounded(rr.left as f32, rr.top as f32, rw(rr) as f32, rh(rr) as f32, ui::sf(6.0),
                  Some(if on { s.accent_solid } else if hov { s.control_hover } else { s.control_bg }),
                  Some((if on { s.accent_solid } else { s.control_border }, 1.0)));
        g.text_in(label, rr.left as f32, rr.top as f32, rw(rr) as f32, rh(rr) as f32, 11.5, false,
                  if on { s.on_accent } else { s.text }, Align::Center, Align::Center, false);
    };

    let res = d.resolutions();
    let rates = d.rates_for(d.current.width, d.current.height);
    let mut label_drawn = (false, false, false);
    for (h, rr) in &hits {
        match h {
            Hit::Display(i) => {
                let dd = &st.displays[*i];
                chip(g, rr, &dd.name, *i == sel, st.hover == *h);
            }
            Hit::Resolution(i) => {
                if !label_drawn.0 {
                    g.text_in("Resolución", r.left as f32, (rr.top - ui::s(24)) as f32,
                              rw(r) as f32, ui::s(20) as f32, 11.5, true, s.text_dim, Align::Near,
                              Align::Center, false);
                    label_drawn.0 = true;
                }
                let (w, hh) = res[*i];
                let on = w == d.current.width && hh == d.current.height;
                chip(g, rr, &format!("{} × {}", w, hh), on, st.hover == *h);
            }
            Hit::Rate(i) => {
                if !label_drawn.1 {
                    g.text_in("Frecuencia de actualización", r.left as f32, (rr.top - ui::s(24)) as f32,
                              rw(r) as f32, ui::s(20) as f32, 11.5, true, s.text_dim, Align::Near,
                              Align::Center, false);
                    label_drawn.1 = true;
                }
                let hz = rates[*i];
                chip(g, rr, &format!("{} Hz", hz), hz == d.current.hz, st.hover == *h);
            }
            Hit::Rotate(deg) => {
                if !label_drawn.2 {
                    g.text_in("Orientación", r.left as f32, (rr.top - ui::s(24)) as f32,
                              rw(r) as f32, ui::s(20) as f32, 11.5, true, s.text_dim, Align::Near,
                              Align::Center, false);
                    label_drawn.2 = true;
                }
                let label = match deg {
                    90 => "Vertical",
                    180 => "Horizontal (↔)",
                    270 => "Vertical (↔)",
                    _ => "Horizontal",
                };
                chip(g, rr, label, *deg == d.rotation, st.hover == *h);
            }
            _ => {}
        }
    }
    if let Some(m) = &st.display_msg {
        g.text_in(m, r.left as f32, (r.bottom - ui::s(24)) as f32, rw(r) as f32, ui::s(20) as f32,
                  11.5, false, s.danger, Align::Near, Align::Center, false);
    }
}

/// Fila de tarjeta con icono, título y valor (enlaces e información).
fn paint_link(g: &Gfx, s: &theme::Style, r: &RECT, glyph: &str, title: &str, sub: &str, hov: bool,
              chevron: bool) {
    g.rounded(r.left as f32, r.top as f32, rw(r) as f32, rh(r) as f32, ui::sf(8.0),
              Some(if hov { s.control_hover } else { s.card_bg }), Some((s.card_border, 1.0)));
    g.text_in(glyph, (r.left + ui::s(12)) as f32, r.top as f32, ui::s(28) as f32, rh(r) as f32, 12.5, false,
              s.accent, Align::Center, Align::Center, true);
    let tx = r.left + ui::s(48);
    let tw = rw(r) - ui::s(48) - if chevron { ui::s(34) } else { ui::s(16) };
    if sub.is_empty() {
        g.text_in(title, tx as f32, r.top as f32, tw as f32, rh(r) as f32, 12.5, false, s.text, Align::Near,
                  Align::Center, false);
    } else {
        g.text_in(title, tx as f32, (r.top + ui::s(10)) as f32, tw as f32, ui::s(20) as f32, 12.5, false,
                  s.text, Align::Near, Align::Center, false);
        g.text_in(sub, tx as f32, (r.top + ui::s(30)) as f32, tw as f32, ui::s(18) as f32, 11.0, false,
                  s.text_dim, Align::Near, Align::Center, false);
    }
    if chevron {
        g.text_in("\u{E76C}", (r.right - ui::s(26)) as f32, r.top as f32, ui::s(18) as f32, rh(r) as f32,
                  8.5, false, s.text_faint, Align::Center, Align::Center, true);
    }
}

fn paint_block(g: &Gfx, s: &theme::Style, st: &State, cfg: &Config, it: &'static Item, r: &RECT) {
    let value = cfg.get(it.id);
    let card = !matches!(it.kind, Kind::Cards(_));
    if card {
        g.rounded(r.left as f32, r.top as f32, rw(r) as f32, rh(r) as f32, ui::sf(8.0),
                  Some(s.card_bg), Some((s.card_border, 1.0)));
    }
    if card {
        let label_w = rw(r) - ui::s(170);
        g.text_in(it.label, (r.left + ui::s(16)) as f32, (r.top + ui::s(8)) as f32, label_w as f32,
                  ui::s(22) as f32, 13.0, false, s.text, Align::Near, Align::Center, false);
        if has_help(it) {
            g.text_in(it.help, (r.left + ui::s(16)) as f32, (r.top + ui::s(28)) as f32,
                      (rw(r) - ui::s(130)) as f32, ui::s(18) as f32, 11.5, false, s.text_dim, Align::Near,
                      Align::Center, false);
        }
    } else {
        g.text_in(it.label, r.left as f32, (r.top - ui::s(18)) as f32, rw(r) as f32, ui::s(20) as f32,
                  12.0, true, s.text_dim, Align::Near, Align::Center, false);
    }

    match it.kind {
        Kind::Bool => {
            let sw = switch_rect(r);
            // Windows 11 pone «Activado/Desactivado» junto al interruptor.
            g.text_in(if value != 0 { "Activado" } else { "Desactivado" },
                      (sw.left - ui::s(84)) as f32, sw.top as f32, ui::s(78) as f32, rh(&sw) as f32, 11.0,
                      false, s.text_dim, Align::Far, Align::Center, false);
            g.switch(sw.left as f32, sw.top as f32, rw(&sw) as f32, rh(&sw) as f32, value != 0,
                     s.accent_solid, s.control_bg, if value != 0 { s.on_accent } else { s.text_dim });
        }
        Kind::Int { min, max, unit, .. } => {
            let (mr, pr) = stepper_rects(r);
            g.text_in(&format!("{} {}", value, unit), (mr.left - ui::s(90)) as f32, mr.top as f32,
                      ui::s(84) as f32, rh(&mr) as f32, 12.0, false, s.text_dim, Align::Far, Align::Center,
                      false);
            small_btn(g, s, mr, "\u{E738}", st.hover == Hit::Step(it.id, -1));
            small_btn(g, s, pr, "\u{E710}", st.hover == Hit::Step(it.id, 1));
            let t = track_rect(r);
            slider(g, s, &t, value, min, max);
        }
        Kind::Choice(opts) => {
            for (i, cr) in chip_rects(r, opts, it).into_iter().enumerate() {
                let sel = value == i as i32;
                let hov = st.hover == Hit::Choice(it.id, i as i32);
                g.rounded(cr.left as f32, cr.top as f32, rw(&cr) as f32, rh(&cr) as f32, ui::sf(6.0),
                          Some(if sel { s.accent_solid } else if hov { s.control_hover } else { s.control_bg }),
                          Some((if sel { s.accent_solid } else { s.control_border }, 1.0)));
                g.text_in(opts[i], cr.left as f32, cr.top as f32, rw(&cr) as f32, rh(&cr) as f32, 12.0,
                          false, if sel { s.on_accent } else { s.text }, Align::Center, Align::Center, false);
            }
        }
        Kind::Cards(opts) => {
            let (a, b) = card_rects(r);
            mode_card(g, s, &a, true, opts[0], value == 0, st.hover == Hit::Choice(it.id, 0));
            mode_card(g, s, &b, false, opts.get(1).copied().unwrap_or("Claro"), value == 1,
                      st.hover == Hit::Choice(it.id, 1));
        }
        Kind::Color => {
            let cur = cfg.accent_colorref();
            for (i, sr) in swatch_rects(r).into_iter().enumerate() {
                let preset = &ACCENT_PRESETS[i];
                let sel = preset.color == cur;
                let cx = (sr.left + sr.right) as f32 / 2.0;
                let cy = (sr.top + sr.bottom) as f32 / 2.0;
                let rad = rw(&sr) as f32 / 2.0;
                if sel || st.hover == Hit::Swatch(i) {
                    g.stroke_circle(cx, cy, rad + ui::sf(3.0), if sel { s.text } else { s.text_faint },
                                    if sel { ui::sf(2.0) } else { 1.0 });
                }
                let c = theme::from_colorref(preset.color);
                g.fill_circle(cx, cy, rad - ui::sf(2.0), c);
                if sel {
                    let lum = (0.299 * theme::r_of(c) as f32 + 0.587 * theme::g_of(c) as f32
                        + 0.114 * theme::b_of(c) as f32) / 255.0;
                    g.check(cx, cy, rad * 0.8,
                            if lum > 0.68 { theme::rgbc(20, 20, 24) } else { theme::rgbc(255, 255, 255) },
                            ui::sf(2.0));
                }
            }
            let names = ["R", "G", "B"];
            let packed = value as u32;
            let chans = [(packed >> 16) & 0xFF, (packed >> 8) & 0xFF, packed & 0xFF];
            for (i, t) in color_tracks(r).iter().enumerate() {
                g.text_in(names[i], (t.left - ui::s(26)) as f32, (t.top - ui::s(10)) as f32, ui::s(20) as f32,
                          ui::s(26) as f32, 11.5, false, s.text_dim, Align::Near, Align::Center, false);
                slider(g, s, t, chans[i] as i32, 0, 255);
                g.text_in(&format!("{}", chans[i]), (t.right + ui::s(10)) as f32, (t.top - ui::s(10)) as f32,
                          ui::s(40) as f32, ui::s(26) as f32, 11.5, false, s.text_dim, Align::Near,
                          Align::Center, false);
            }
            let pv = rect(r.right - ui::s(60), r.top + ui::s(10), ui::s(44), ui::s(24));
            g.rounded(pv.left as f32, pv.top as f32, rw(&pv) as f32, rh(&pv) as f32, ui::sf(5.0),
                      Some(theme::from_colorref(cfg.accent_colorref())), Some((s.card_border, 1.0)));
            draw_button(g, s, winaccent_rect(r), "Usar el color de Windows", st.hover == Hit::WinAccent, false);
        }
    }
}

fn slider(g: &Gfx, s: &theme::Style, track: &RECT, value: i32, min: i32, max: i32) {
    let tw = rw(track) as f32;
    let th = rh(track) as f32;
    g.rounded(track.left as f32, track.top as f32, tw, th, th / 2.0, Some(s.track), None);
    let frac = if max > min { (value - min) as f32 / (max - min) as f32 } else { 0.0 };
    let fillw = tw * frac.clamp(0.0, 1.0);
    g.rounded(track.left as f32, track.top as f32, fillw, th, th / 2.0, Some(s.accent_solid), None);
    let kx = track.left as f32 + fillw;
    g.fill_circle(kx, track.top as f32 + th / 2.0, ui::sf(8.0), s.accent_solid);
    g.fill_circle(kx, track.top as f32 + th / 2.0, ui::sf(4.0), s.on_accent);
}

fn mode_card(g: &Gfx, s: &theme::Style, r: &RECT, dark_mode: bool, label: &str, selected: bool, hover: bool) {
    g.rounded(r.left as f32, r.top as f32, rw(r) as f32, rh(r) as f32, ui::sf(10.0),
              Some(if hover { s.control_hover } else { s.card_bg }),
              Some((if selected { s.accent_solid } else { s.control_border }, if selected { 2.0 } else { 1.0 })));
    let (bg, title) = if dark_mode {
        (theme::rgbc(17, 20, 27), theme::rgbc(23, 27, 36))
    } else {
        (theme::rgbc(247, 248, 250), theme::rgbc(237, 239, 243))
    };
    let px = r.left + ui::s(12);
    let py = r.top + ui::s(12);
    let pw = rw(r) - ui::s(24);
    let ph = rh(r) - ui::s(52);
    g.rounded(px as f32, py as f32, pw as f32, ph as f32, ui::sf(6.0), Some(bg), None);
    g.rounded(px as f32, py as f32, pw as f32, ui::s(18) as f32, ui::sf(6.0), Some(title), None);
    g.rounded((px + ui::s(8)) as f32, (py + ui::s(5)) as f32, ui::s(8) as f32, ui::s(8) as f32, ui::sf(2.0),
              Some(s.accent_solid), None);
    g.text_in(label, (r.left + ui::s(14)) as f32, (r.bottom - ui::s(36)) as f32, (rw(r) - ui::s(50)) as f32,
              ui::s(28) as f32, 13.0, false, s.text, Align::Near, Align::Center, false);
    if selected {
        let bx = (r.right - ui::s(34)) as f32;
        let by = (r.bottom - ui::s(32)) as f32;
        g.fill_circle(bx + ui::s(10) as f32, by + ui::s(10) as f32, ui::s(10) as f32, s.accent_solid);
        g.check(bx + ui::s(10) as f32, by + ui::s(10) as f32, ui::sf(11.0), s.on_accent, ui::sf(1.8));
    }
}

fn chrome_btn(g: &Gfx, s: &theme::Style, r: RECT, glyph: &str, hov: bool, close: bool) {
    if hov {
        g.fill_rect(r.left, r.top, rw(&r), rh(&r), if close { s.close_hover } else { s.control_hover });
    }
    let col = if hov && close { theme::rgbc(255, 255, 255) } else { s.text_dim };
    g.text_in(glyph, r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32, 10.0, false, col,
              Align::Center, Align::Center, true);
}

fn small_btn(g: &Gfx, s: &theme::Style, r: RECT, glyph: &str, hov: bool) {
    g.rounded(r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32, ui::sf(5.0),
              Some(if hov { s.control_hover } else { s.control_bg }), Some((s.control_border, 1.0)));
    g.text_in(glyph, r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32, 8.0, false, s.text,
              Align::Center, Align::Center, true);
}

fn draw_button(g: &Gfx, s: &theme::Style, r: RECT, label: &str, hov: bool, primary: bool) {
    if r.right <= r.left {
        return;
    }
    let (fill, border, fg) = if primary {
        (if hov { s.accent_hover } else { s.accent_solid }, theme::argb(0, 0, 0, 0), s.on_accent)
    } else {
        (if hov { s.control_hover } else { s.control_bg }, s.control_border, s.text)
    };
    g.rounded(r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32, ui::sf(6.0), Some(fill),
              Some((border, 1.0)));
    g.text_in(label, r.left as f32, r.top as f32, rw(&r) as f32, rh(&r) as f32, 12.5, false, fg,
              Align::Center, Align::Center, false);
}

// ============================================================ interacción
fn hit(st: &State, x: i32, y: i32) -> Hit {
    // La X del buscador tiene prioridad sobre el propio cuadro.
    if !st.search.is_empty()
        && in_rect(&rect(st.search_rc.right - ui::s(28), st.search_rc.top, ui::s(28), rh(&st.search_rc)), x, y)
    {
        return Hit::ClearSearch;
    }
    for (r, h) in st.hits.iter() {
        if !in_rect(r, x, y) {
            continue;
        }
        let is_chrome = matches!(h, Hit::Nav(_) | Hit::Min | Hit::Max | Hit::Close | Hit::Search
                                     | Hit::ResetPage | Hit::ResetAll | Hit::OpenIni);
        if !is_chrome && !(y >= st.content_rc.top && y <= st.content_rc.bottom) {
            continue;
        }
        if *h == Hit::ResetPage && NAV[st.page].reset.is_none() {
            continue;
        }
        return *h;
    }
    Hit::None
}

fn value_at(track: &RECT, x: i32, min: i32, max: i32, step: i32) -> i32 {
    let w = rw(track).max(1);
    let raw = min + ((x - track.left) as i64 * (max - min) as i64 / w as i64) as i32;
    let step = step.max(1);
    let snapped = ((raw - min + step / 2) / step) * step + min;
    snapped.clamp(min, max)
}

fn slider_track(st: &State, id: Id, channel: u8) -> Option<RECT> {
    let dy = st.content_rc.top - st.scroll;
    for b in st.blocks.iter() {
        let BlockKind::Setting(it) = &b.kind else { continue };
        if it.id != id {
            continue;
        }
        let r = rect(b.rect.left, b.rect.top + dy, rw(&b.rect), rh(&b.rect));
        return Some(if channel == 0 { track_rect(&r) } else { color_tracks(&r)[(channel - 1) as usize] });
    }
    None
}

fn apply_slider(st: &mut State, id: Id, channel: u8, x: i32, persist: bool) {
    let Some(t) = slider_track(st, id, channel) else { return };
    let mut c = config::current();
    if channel == 0 {
        if let Kind::Int { min, max, step, .. } = config::item(id).kind {
            c.set(id, value_at(&t, x, min, max, step));
        }
    } else {
        let v = value_at(&t, x, 0, 255, 1) as u32;
        let packed = c.get(id) as u32;
        let shift = 16 - (channel as u32 - 1) * 8;
        let cleared = packed & !(0xFF << shift);
        c.set(id, (cleared | (v << shift)) as i32);
    }
    commit(st, &c, persist);
}

fn nudge(st: &mut State, id: Id, dir: i32) {
    let mut c = config::current();
    match config::item(id).kind {
        Kind::Int { step, .. } => {
            let v = c.get(id) + dir * step.max(1);
            c.set(id, v);
        }
        Kind::Bool => c.toggle(id),
        Kind::Choice(opts) | Kind::Cards(opts) => {
            let n = opts.len() as i32;
            c.set(id, (c.get(id) + dir).rem_euclid(n));
        }
        Kind::Color => {}
    }
    commit(st, &c, true);
}

fn open_with_shell(target: &str) {
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), ui::wide("open").as_ptr(), ui::wide(target).as_ptr(),
                      std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

fn open_ini() {
    config::save(&config::current());
    let path = config::config_file();
    open_with_shell(&path.to_string_lossy());
}

fn goto(st: &mut State, page: usize) {
    st.page = page.min(NAV.len() - 1);
    st.scroll = 0;
    st.hover = Hit::None;
    st.search.clear();
    st.searching = false;
    layout(st);
    unsafe { InvalidateRect(st.hwnd, std::ptr::null(), 0) };
}

fn click(st: &mut State, x: i32, y: i32) {
    let hwnd = st.hwnd;
    let h = hit(st, x, y);
    if h != Hit::Search && h != Hit::ClearSearch {
        st.searching = false;
    }
    match h {
        Hit::Close => unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0); },
        Hit::Min => unsafe { ShowWindow(hwnd, SW_MINIMIZE); },
        Hit::Max => unsafe {
            ShowWindow(hwnd, if IsZoomed(hwnd) != 0 { SW_RESTORE } else { SW_MAXIMIZE });
        },
        Hit::Nav(i) => goto(st, i),
        Hit::Tile(i) => goto(st, i),
        Hit::Search => {
            st.searching = true;
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
        }
        Hit::ClearSearch => {
            st.search.clear();
            st.searching = false;
            st.scroll = 0;
            layout(st);
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
        }
        Hit::Link(i) => {
            let act = st.links[i].3;
            match act {
                Act::Url(u) => open_with_shell(u),
                Act::Own("--abrir-ini") => open_ini(),
                Act::Own(a) => crate::shell::launch_own(a),
            }
        }
        Hit::Toggle(id) => {
            let mut c = config::current();
            c.toggle(id);
            commit(st, &c, true);
        }
        Hit::Choice(id, v) => {
            let mut c = config::current();
            c.set(id, v);
            commit(st, &c, true);
        }
        Hit::Step(id, d) => nudge(st, id, d),
        Hit::Swatch(i) => {
            let mut c = config::current();
            c.set_accent_colorref(ACCENT_PRESETS[i].color);
            commit(st, &c, true);
        }
        Hit::WinAccent => {
            let mut c = config::current();
            c.set_accent_colorref(windows_accent());
            commit(st, &c, true);
        }
        Hit::Slider(id, ch) => {
            st.drag = Some((id, ch));
            unsafe { SetCapture(hwnd) };
            apply_slider(st, id, ch, x, false);
        }
        Hit::ResetPage => {
            if let Some(p) = NAV[st.page].reset {
                let mut c = config::current();
                c.reset_page(p);
                commit(st, &c, true);
            }
        }
        Hit::ResetAll => {
            let c = Config::default();
            commit(st, &c, true);
        }
        Hit::OpenIni => open_ini(),
        Hit::Wall(i) => {
            if let Some(p) = st.walls.get(i).cloned() {
                crate::desktop::set_wallpaper(&p);
                // El escritorio propio se entera por la difusión de ajustes.
                ui::broadcast_config_changed();
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            }
        }
        Hit::WallBrowse => {
            if let Some(p) = display::browse_image(hwnd) {
                crate::desktop::set_wallpaper(&p);
                st.walls = display::wallpapers();
                ui::broadcast_config_changed();
                layout(st);
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            }
        }
        Hit::WallNone => {
            crate::desktop::set_wallpaper("");
            ui::broadcast_config_changed();
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
        }
        Hit::Display(i) => {
            st.display_sel = i;
            st.display_msg = None;
            layout(st);
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
        }
        Hit::Resolution(i) => {
            let sel = st.display_sel.min(st.displays.len().saturating_sub(1));
            if let Some(d) = st.displays.get(sel) {
                let res = d.resolutions();
                if let Some(&(w, h)) = res.get(i) {
                    // Se conserva la frecuencia si la nueva resolución la admite.
                    let hz = if d.rates_for(w, h).contains(&d.current.hz) { d.current.hz } else { 0 };
                    let dev = d.device.clone();
                    st.display_msg = display::set_mode(&dev, w, h, hz).err();
                    st.displays = display::displays();
                    layout(st);
                    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
                }
            }
        }
        Hit::Rate(i) => {
            let sel = st.display_sel.min(st.displays.len().saturating_sub(1));
            if let Some(d) = st.displays.get(sel) {
                let rates = d.rates_for(d.current.width, d.current.height);
                if let Some(&hz) = rates.get(i) {
                    let (dev, w, h) = (d.device.clone(), d.current.width, d.current.height);
                    st.display_msg = display::set_mode(&dev, w, h, hz).err();
                    st.displays = display::displays();
                    layout(st);
                    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
                }
            }
        }
        Hit::Rotate(deg) => {
            let sel = st.display_sel.min(st.displays.len().saturating_sub(1));
            if let Some(d) = st.displays.get(sel) {
                let dev = d.device.clone();
                st.display_msg = display::set_rotation(&dev, deg).err();
                st.displays = display::displays();
                layout(st);
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            }
        }
        Hit::None => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let st_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if msg == ui::config_changed_msg() {
        if !st_ptr.is_null() {
            window::refresh_frame(hwnd);
            layout(&mut *st_ptr);
            InvalidateRect(hwnd, std::ptr::null(), 0);
        }
        return 0;
    }
    match msg {
        WM_NCCALCSIZE if wp != 0 => 0,
        WM_NCHITTEST => {
            let mut pt = POINT { x: (lp & 0xFFFF) as i16 as i32, y: ((lp >> 16) & 0xFFFF) as i16 as i32 };
            ScreenToClient(hwnd, &mut pt);
            let mut rc = er();
            GetClientRect(hwnd, &mut rc);
            let b = ui::s(6);
            if IsZoomed(hwnd) == 0 {
                let (l, r, t, bo) = (pt.x < b, pt.x >= rc.right - b, pt.y < b, pt.y >= rc.bottom - b);
                if t && l { return HTTOPLEFT as LRESULT; }
                if t && r { return HTTOPRIGHT as LRESULT; }
                if bo && l { return HTBOTTOMLEFT as LRESULT; }
                if bo && r { return HTBOTTOMRIGHT as LRESULT; }
                if t { return HTTOP as LRESULT; }
                if bo { return HTBOTTOM as LRESULT; }
                if l { return HTLEFT as LRESULT; }
                if r { return HTRIGHT as LRESULT; }
            }
            if pt.y < ui::caption_h() {
                if !st_ptr.is_null() {
                    let st = &*st_ptr;
                    if in_rect(&st.close_rc, pt.x, pt.y) || in_rect(&st.min_rc, pt.x, pt.y)
                        || in_rect(&st.max_rc, pt.x, pt.y) {
                        return HTCLIENT as LRESULT;
                    }
                }
                return HTCAPTION as LRESULT;
            }
            HTCLIENT as LRESULT
        }
        WM_SIZE => {
            if !st_ptr.is_null() {
                layout(&mut *st_ptr);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            if !st_ptr.is_null() {
                paint(&*st_ptr);
            }
            0
        }
        WM_TIMER => {
            // Refresco de los datos del equipo (red, batería, memoria).
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let needs = st.page == 0 || NAV[st.page].groups.iter()
                    .any(|g| g.rows.iter().any(|r| matches!(r, Row::Info(_))));
                if needs {
                    st.info = collect_info();
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_MOUSEMOVE => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                if let Some((id, ch)) = st.drag {
                    apply_slider(st, id, ch, x, false);
                } else {
                    let h = hit(st, x, y);
                    if h != st.hover {
                        st.hover = h;
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                click(st, x, y);
            }
            0
        }
        WM_LBUTTONUP => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                if st.drag.take().is_some() {
                    ReleaseCapture();
                    let c = config::current();
                    commit(st, &c, true);
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let delta = ((wp >> 16) & 0xFFFF) as i16 as i32;
                let max_scroll = (st.content_h - rh(&st.content_rc)).max(0);
                st.scroll = (st.scroll - (delta / 120) * ui::s(48)).clamp(0, max_scroll);
                layout(st);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_CHAR => {
            // Escribir en el buscador (una vez pulsado el cuadro).
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                if st.searching {
                    let ch = wp as u32;
                    if ch == VK_BACK as u32 {
                        st.search.pop();
                    } else if ch >= 0x20 {
                        if let Some(c) = char::from_u32(ch) {
                            st.search.push(c);
                        }
                    }
                    st.scroll = 0;
                    layout(st);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_KEYDOWN => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let vk = wp as u32;
                let max_scroll = (st.content_h - rh(&st.content_rc)).max(0);
                if vk == VK_ESCAPE as u32 {
                    if st.searching || !st.search.is_empty() {
                        st.search.clear();
                        st.searching = false;
                        layout(st);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    } else {
                        PostMessageW(hwnd, WM_CLOSE, 0, 0);
                    }
                } else if vk == VK_DOWN as u32 || vk == VK_UP as u32 {
                    let d = if vk == VK_DOWN as u32 { 1 } else { -1 };
                    st.scroll = (st.scroll + d * ui::s(40)).clamp(0, max_scroll);
                    layout(st);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                } else if vk == VK_LEFT as u32 || vk == VK_RIGHT as u32 {
                    let d = if vk == VK_RIGHT as u32 { 1 } else { -1 };
                    let mut pt = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut pt);
                    ScreenToClient(hwnd, &mut pt);
                    match hit(st, pt.x, pt.y) {
                        Hit::Slider(id, 0) | Hit::Step(id, _) | Hit::Toggle(id) | Hit::Choice(id, _) => {
                            nudge(st, id, d)
                        }
                        _ => {}
                    }
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

pub fn run() -> i32 {
    unsafe {
        let hinst = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null());
        ui::init(hinst);

        let page = page_for_arg(&args::positional().unwrap_or_default());

        let class_name = ui::wide("OShellRustSettings");
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

        let mut wa = er();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut _ as *mut c_void, 0);
        let w = ui::s(1040).min(wa.right - wa.left);
        let h = ui::s(720).min(wa.bottom - wa.top);
        let x = wa.left + (wa.right - wa.left - w) / 2;
        let y = wa.top + (wa.bottom - wa.top - h) / 2;

        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            class_name.as_ptr(),
            ui::wide("Configuración").as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_THICKFRAME,
            x, y, w, h,
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
            page,
            hover: Hit::None,
            scroll: 0,
            content_h: 0,
            drag: None,
            search: String::new(),
            searching: false,
            info: collect_info(),
            walls: display::wallpapers(),
            displays: display::displays(),
            display_sel: 0,
            display_msg: None,
            x0: 0,
            cw: 0,
            content_rc: er(),
            nav: Vec::new(),
            search_rc: er(),
            tiles: Vec::new(),
            blocks: Vec::new(),
            links: Vec::new(),
            hits: Vec::new(),
            min_rc: er(),
            max_rc: er(),
            close_rc: er(),
            reset_page_rc: er(),
            reset_all_rc: er(),
            open_ini_rc: er(),
        });
        let raw = Box::into_raw(st);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
        window::refresh_frame(hwnd);
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0,
                     SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
        layout(&mut *raw);
        SetTimer(hwnd, 1, 5000, None);
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetForegroundWindow(hwnd);
        ui::apply_perf();

        let code = window::message_loop();
        ui::shutdown();
        code
    }
}
