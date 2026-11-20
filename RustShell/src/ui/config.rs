// Configuración global de OptimizedShell.
//
// Toda la interfaz (tema, barra de tareas, menú Inicio, listas, rendimiento)
// se describe aquí como una tabla declarativa de ajustes: cada entrada tiene
// clave de INI, etiqueta, ayuda, tipo y valor por defecto. La página de
// Configuración se dibuja a partir de esta tabla, de modo que añadir un ajuste
// nuevo es añadir una fila a `ITEMS` (y leerlo donde haga falta).
//
// Los valores se guardan como enteros en `%APPDATA%\OptimizedShell\config.ini`,
// agrupados por secciones. Se importa el antiguo `theme.ini` la primera vez.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;

// ------------------------------------------------------------ identificadores
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
pub enum Id {
    // Apariencia
    Modo = 0,
    Acento,
    Desenfoque,
    TinteFondo,
    ContrasteTexto,
    OpacidadBarra,
    OpacidadVentanas,
    Radio,
    EscalaInterfaz,
    EscalaTexto,
    Fuente,
    AltoTitulo,
    // Barra de tareas
    BarraAlto,
    BarraPosicion,
    BarraAlineacion,
    BarraAnchoBoton,
    BarraMostrarTitulos,
    BarraMostrarInicio,
    BarraMostrarReloj,
    BarraReloj24h,
    BarraSegundos,
    BarraFecha,
    BarraReservarArea,
    BarraSiempreVisible,
    BarraRefresco,
    // Menú Inicio
    MenuAncho,
    MenuAltoElemento,
    MenuIconos,
    MenuEjecutar,
    MenuBloquear,
    MenuApagado,
    MenuAlto,
    MenuColumnaDerecha,
    MenuBusqueda,
    MenuFrecuentes,
    MenuIconosApps,
    // Listas, explorador y administrador
    ListaAltoFila,
    ExplPanelLateral,
    ExplOcultos,
    ExplCarpetasPrimero,
    ExplColumnaFecha,
    ExplColumnaTipo,
    ExplColumnaTamano,
    TaskmgrRefresco,
    PerfPorNucleo,
    PerfHistorial,
    // Rendimiento
    Eco,
    LiberarMemoria,
    AnchoBotonTitulo,
    // Barra de tareas: iconos de las aplicaciones y botón Inicio
    BarraIconosApps,
    BarraTamanoIcono,
    BarraInicioCentro,
    BarraIconoInicio,
    BarraIconoIndice,
    // Área de notificación (bandeja del sistema)
    BandejaMostrar,
    BandejaSonido,
    BandejaRed,
    BandejaBateria,
    BandejaBluetooth,
    BandejaNotificaciones,
    BandejaPorcentajeBateria,
    BandejaSeparador,
    // Notificaciones
    NotifActivadas,
    NotifNoMolestar,
    NotifDuracion,
    NotifMax,
    NotifAncho,
    NotifPosicion,
    NotifHistorial,
    NotifSonido,
    // Administrador de tareas y explorador (segunda tanda)
    TaskmgrIconos,
    ExplIconos,
    ExplConfirmarBorrado,
    ExplAutoRefresco,
    // Escritorio
    EscritorioActivo,
    EscritorioAjuste,
    EscritorioColor,
    HotkeysActivas,
    BandejaIconosApps,
    // Iconos del escritorio
    EscritorioIconos,
    EscritorioTamanoIcono,
    EscritorioEtiquetas,
    EscritorioEquipo,
    EscritorioPapelera,
    EscritorioUsuario,
    EscritorioRed,
    EscritorioPanel,
    EscritorioArchivos,
    EscritorioPublico,
    EscritorioOrden,
    EscritorioOcultos,
    EscritorioAutoRefresco,
}

pub const COUNT: usize = Id::EscritorioAutoRefresco as usize + 1;

/// Página de la ventana de Configuración a la que pertenece cada ajuste.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Apariencia = 0,
    Barra = 1,
    Menu = 2,
    Listas = 3,
    Rendimiento = 4,
    Bandeja = 5,
    Notificaciones = 6,
    Escritorio = 7,
}

#[derive(Clone, Copy)]
pub enum Kind {
    /// Interruptor (0/1).
    Bool,
    /// Entero con rango, paso y unidad para la etiqueta ("px", "%", "ms"...).
    Int { min: i32, max: i32, step: i32, unit: &'static str },
    /// Índice dentro de una lista de opciones, dibujado como control segmentado.
    Choice(&'static [&'static str]),
    /// Igual que Choice pero con tarjetas de vista previa (modo claro/oscuro).
    Cards(&'static [&'static str]),
    /// Color empaquetado 0xRRGGBB: paleta de muestras + deslizadores R/G/B.
    Color,
}

pub struct Item {
    pub id: Id,
    pub page: Page,
    /// Sección del INI.
    pub section: &'static str,
    /// Clave del INI.
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    pub def: i32,
}

pub const FONTS: &[&str] = &[
    "Segoe UI",
    "Segoe UI Variable",
    "Segoe UI Semilight",
    "Arial",
    "Verdana",
    "Tahoma",
    "Consolas",
    "Cascadia Mono",
];

const MODOS: &[&str] = &["Oscuro", "Claro"];
const POSICIONES: &[&str] = &["Abajo", "Arriba"];
const ALINEACIONES: &[&str] = &["Izquierda", "Centro"];
const ICONOS_INICIO: &[&str] = &["Glifo", "shell32.dll", "imageres.dll", "Dibujado"];
const ESQUINAS_NOTIF: &[&str] = &["Derecha", "Izquierda", "Centro"];
const AJUSTES_FONDO: &[&str] = &["Rellenar", "Ajustar", "Expandir", "Mosaico", "Centrar"];
const ORDEN_ICONOS: &[&str] = &["Nombre", "Tipo", "Fecha", "Tamaño"];

const APARIENCIA: &str = "Apariencia";
const BARRA: &str = "BarraDeTareas";
const MENU: &str = "MenuInicio";
const LISTAS: &str = "Listas";
const RENDIMIENTO: &str = "Rendimiento";
const BANDEJA: &str = "Bandeja";
const NOTIFICACIONES: &str = "Notificaciones";
const ESCRITORIO: &str = "Escritorio";

/// Tabla de ajustes. **El orden debe coincidir con el de `Id`.**
pub static ITEMS: &[Item] = &[
    // ---------------------------------------------------------- apariencia
    Item { id: Id::Modo, page: Page::Apariencia, section: APARIENCIA, key: "Modo",
        label: "Modo", help: "Paleta clara u oscura para toda la interfaz.",
        kind: Kind::Cards(MODOS), def: 0 },
    Item { id: Id::Acento, page: Page::Apariencia, section: APARIENCIA, key: "Acento",
        label: "Color de acento", help: "Elige una muestra o ajusta el color canal a canal.",
        kind: Kind::Color, def: 0x3B82F6 },
    Item { id: Id::Desenfoque, page: Page::Apariencia, section: APARIENCIA, key: "Desenfoque",
        label: "Desenfoque (efecto cristal)",
        help: "Fondo acrílico en la barra, los paneles y las ventanas. Desactivarlo ahorra GPU.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::TinteFondo, page: Page::Apariencia, section: APARIENCIA, key: "TinteFondo",
        label: "Tinte del fondo", help: "Mezcla los fondos hacia el color de acento.",
        kind: Kind::Int { min: 0, max: 40, step: 1, unit: "%" }, def: 0 },
    Item { id: Id::ContrasteTexto, page: Page::Apariencia, section: APARIENCIA, key: "ContrasteTexto",
        label: "Contraste del texto secundario",
        help: "Acerca los textos atenuados al color de texto principal.",
        kind: Kind::Int { min: 0, max: 100, step: 5, unit: "%" }, def: 0 },
    Item { id: Id::OpacidadBarra, page: Page::Apariencia, section: APARIENCIA, key: "OpacidadBarra",
        label: "Opacidad de la barra de tareas", help: "",
        kind: Kind::Int { min: 0, max: 100, step: 1, unit: "%" }, def: 60 },
    Item { id: Id::OpacidadVentanas, page: Page::Apariencia, section: APARIENCIA, key: "OpacidadVentanas",
        label: "Opacidad de las ventanas", help: "",
        kind: Kind::Int { min: 0, max: 100, step: 1, unit: "%" }, def: 96 },
    Item { id: Id::Radio, page: Page::Apariencia, section: APARIENCIA, key: "Radio",
        label: "Radio de las esquinas", help: "Afecta a tarjetas, botones, menús y filas.",
        kind: Kind::Int { min: 0, max: 20, step: 1, unit: "px" }, def: 10 },
    Item { id: Id::EscalaInterfaz, page: Page::Apariencia, section: APARIENCIA, key: "EscalaInterfaz",
        label: "Escala de la interfaz",
        help: "Multiplica todas las medidas, además del DPI del sistema.",
        kind: Kind::Int { min: 70, max: 200, step: 5, unit: "%" }, def: 100 },
    Item { id: Id::EscalaTexto, page: Page::Apariencia, section: APARIENCIA, key: "EscalaTexto",
        label: "Tamaño del texto", help: "Escala solo las fuentes, no los controles.",
        kind: Kind::Int { min: 70, max: 160, step: 5, unit: "%" }, def: 100 },
    Item { id: Id::Fuente, page: Page::Apariencia, section: APARIENCIA, key: "Fuente",
        label: "Fuente de la interfaz", help: "Si la familia no está instalada, Windows usa la predeterminada.",
        kind: Kind::Choice(FONTS), def: 0 },
    Item { id: Id::AltoTitulo, page: Page::Apariencia, section: APARIENCIA, key: "AltoTitulo",
        label: "Alto de la barra de título", help: "Para las ventanas con marco propio.",
        kind: Kind::Int { min: 28, max: 60, step: 1, unit: "px" }, def: 42 },
    // ------------------------------------------------------- barra de tareas
    Item { id: Id::BarraAlto, page: Page::Barra, section: BARRA, key: "Alto",
        label: "Alto de la barra", help: "",
        kind: Kind::Int { min: 28, max: 80, step: 1, unit: "px" }, def: 48 },
    Item { id: Id::BarraPosicion, page: Page::Barra, section: BARRA, key: "Posicion",
        label: "Posición", help: "Borde de la pantalla al que se acopla la barra.",
        kind: Kind::Choice(POSICIONES), def: 0 },
    Item { id: Id::BarraAlineacion, page: Page::Barra, section: BARRA, key: "Alineacion",
        label: "Alineación de los botones", help: "",
        kind: Kind::Choice(ALINEACIONES), def: 0 },
    Item { id: Id::BarraAnchoBoton, page: Page::Barra, section: BARRA, key: "AnchoBoton",
        label: "Ancho de los botones de ventana", help: "",
        kind: Kind::Int { min: 44, max: 320, step: 2, unit: "px" }, def: 170 },
    Item { id: Id::BarraMostrarTitulos, page: Page::Barra, section: BARRA, key: "MostrarTitulos",
        label: "Mostrar títulos de ventana", help: "Al desactivarlo los botones quedan compactos.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::BarraMostrarInicio, page: Page::Barra, section: BARRA, key: "MostrarBotonInicio",
        label: "Mostrar botón Inicio", help: "", kind: Kind::Bool, def: 1 },
    Item { id: Id::BarraMostrarReloj, page: Page::Barra, section: BARRA, key: "MostrarReloj",
        label: "Mostrar reloj", help: "", kind: Kind::Bool, def: 1 },
    Item { id: Id::BarraReloj24h, page: Page::Barra, section: BARRA, key: "Reloj24h",
        label: "Reloj de 24 horas", help: "Desactivado usa a. m. / p. m.", kind: Kind::Bool, def: 1 },
    Item { id: Id::BarraSegundos, page: Page::Barra, section: BARRA, key: "MostrarSegundos",
        label: "Mostrar segundos", help: "Refresca el reloj cada segundo.", kind: Kind::Bool, def: 0 },
    Item { id: Id::BarraFecha, page: Page::Barra, section: BARRA, key: "MostrarFecha",
        label: "Mostrar fecha", help: "", kind: Kind::Bool, def: 1 },
    Item { id: Id::BarraReservarArea, page: Page::Barra, section: BARRA, key: "ReservarArea",
        label: "Reservar área de trabajo",
        help: "Las ventanas maximizadas no tapan la barra (appbar).", kind: Kind::Bool, def: 1 },
    Item { id: Id::BarraSiempreVisible, page: Page::Barra, section: BARRA, key: "SiempreVisible",
        label: "Siempre visible", help: "Mantiene la barra por encima del resto de ventanas.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::BarraRefresco, page: Page::Barra, section: BARRA, key: "IntervaloRefresco",
        label: "Intervalo de refresco", help: "Cada cuánto se revisa la lista de ventanas.",
        kind: Kind::Int { min: 300, max: 5000, step: 100, unit: "ms" }, def: 1500 },
    // ---------------------------------------------------------- menú Inicio
    Item { id: Id::MenuAncho, page: Page::Menu, section: MENU, key: "Ancho",
        label: "Ancho del menú", help: "",
        kind: Kind::Int { min: 200, max: 520, step: 4, unit: "px" }, def: 300 },
    Item { id: Id::MenuAltoElemento, page: Page::Menu, section: MENU, key: "AltoElemento",
        label: "Alto de cada elemento", help: "",
        kind: Kind::Int { min: 26, max: 60, step: 1, unit: "px" }, def: 40 },
    Item { id: Id::MenuIconos, page: Page::Menu, section: MENU, key: "MostrarIconos",
        label: "Mostrar iconos", help: "", kind: Kind::Bool, def: 1 },
    Item { id: Id::MenuEjecutar, page: Page::Menu, section: MENU, key: "MostrarEjecutar",
        label: "Incluir «Ejecutar...»", help: "", kind: Kind::Bool, def: 1 },
    Item { id: Id::MenuBloquear, page: Page::Menu, section: MENU, key: "MostrarBloquear",
        label: "Incluir «Bloquear»", help: "", kind: Kind::Bool, def: 1 },
    Item { id: Id::MenuApagado, page: Page::Menu, section: MENU, key: "MostrarApagado",
        label: "Incluir «Reiniciar» y «Apagar»", help: "", kind: Kind::Bool, def: 1 },
    Item { id: Id::MenuAlto, page: Page::Menu, section: MENU, key: "Alto",
        label: "Alto del menú", help: "Cuanto más alto, más programas se ven sin desplazar.",
        kind: Kind::Int { min: 320, max: 900, step: 10, unit: "px" }, def: 560 },
    Item { id: Id::MenuColumnaDerecha, page: Page::Menu, section: MENU, key: "ColumnaDerecha",
        label: "Mostrar columna derecha",
        help: "Usuario, carpetas personales, Equipo, Panel de control y apagado.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::MenuBusqueda, page: Page::Menu, section: MENU, key: "Busqueda",
        label: "Mostrar cuadro de búsqueda", help: "Filtra los programas según escribes.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::MenuFrecuentes, page: Page::Menu, section: MENU, key: "Frecuentes",
        label: "Programas frecuentes",
        help: "Cuántos aparecen en la primera pantalla, ordenados por uso.",
        kind: Kind::Int { min: 0, max: 20, step: 1, unit: "" }, def: 9 },
    Item { id: Id::MenuIconosApps, page: Page::Menu, section: MENU, key: "IconosApps",
        label: "Iconos de los programas",
        help: "Los pide al shell (SHGetFileInfo) solo para las filas visibles.",
        kind: Kind::Bool, def: 1 },
    // --------------------------------------------- listas / explorador / admin
    Item { id: Id::ListaAltoFila, page: Page::Listas, section: LISTAS, key: "AltoFila",
        label: "Alto de las filas", help: "Explorador y administrador de tareas.",
        kind: Kind::Int { min: 18, max: 48, step: 1, unit: "px" }, def: 28 },
    Item { id: Id::ExplPanelLateral, page: Page::Listas, section: LISTAS, key: "PanelLateral",
        label: "Panel lateral del explorador", help: "Accesos rápidos a las carpetas del usuario.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::ExplOcultos, page: Page::Listas, section: LISTAS, key: "ArchivosOcultos",
        label: "Mostrar archivos ocultos", help: "", kind: Kind::Bool, def: 1 },
    Item { id: Id::ExplCarpetasPrimero, page: Page::Listas, section: LISTAS, key: "CarpetasPrimero",
        label: "Carpetas primero", help: "Ignora el criterio de orden para separar carpetas y archivos.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::ExplColumnaFecha, page: Page::Listas, section: LISTAS, key: "ColumnaFecha",
        label: "Columna «Fecha de modificación»", help: "", kind: Kind::Bool, def: 1 },
    Item { id: Id::ExplColumnaTipo, page: Page::Listas, section: LISTAS, key: "ColumnaTipo",
        label: "Columna «Tipo»", help: "", kind: Kind::Bool, def: 1 },
    Item { id: Id::ExplColumnaTamano, page: Page::Listas, section: LISTAS, key: "ColumnaTamano",
        label: "Columna «Tamaño»", help: "", kind: Kind::Bool, def: 1 },
    Item { id: Id::TaskmgrRefresco, page: Page::Listas, section: LISTAS, key: "RefrescoAdminTareas",
        label: "Refresco del administrador de tareas", help: "Intervalos más largos consumen menos CPU.",
        kind: Kind::Int { min: 500, max: 5000, step: 100, unit: "ms" }, def: 1000 },
    Item { id: Id::PerfPorNucleo, page: Page::Listas, section: LISTAS, key: "PerfPorNucleo",
        label: "Gráficas por núcleo",
        help: "En la pestaña Rendimiento, añade una mini gráfica por núcleo lógico.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::PerfHistorial, page: Page::Listas, section: LISTAS, key: "PerfHistorial",
        label: "Historial de las gráficas", help: "Ventana de tiempo que muestran las gráficas de Rendimiento.",
        kind: Kind::Int { min: 15, max: 300, step: 5, unit: "s" }, def: 60 },
    // --------------------------------------------------------- rendimiento
    Item { id: Id::Eco, page: Page::Rendimiento, section: RENDIMIENTO, key: "EcoQoS",
        label: "Modo EcoQoS", help: "Windows programa el proceso en núcleos de bajo consumo.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::LiberarMemoria, page: Page::Rendimiento, section: RENDIMIENTO, key: "LiberarMemoria",
        label: "Liberar memoria sin usar",
        help: "Devuelve al sistema las páginas inactivas tras cada refresco.", kind: Kind::Bool, def: 1 },
    Item { id: Id::AnchoBotonTitulo, page: Page::Apariencia, section: APARIENCIA, key: "AnchoBotonTitulo",
        label: "Ancho de los botones de la barra de título", help: "Minimizar, maximizar y cerrar.",
        kind: Kind::Int { min: 34, max: 64, step: 1, unit: "px" }, def: 46 },
    // ------------------------------- barra: iconos de apps y botón Inicio
    Item { id: Id::BarraIconosApps, page: Page::Barra, section: BARRA, key: "IconosApps",
        label: "Mostrar el icono de cada aplicación",
        help: "Usa el icono real de la ventana; si no tiene, el del ejecutable.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::BarraTamanoIcono, page: Page::Barra, section: BARRA, key: "TamanoIcono",
        label: "Tamaño de los iconos", help: "",
        kind: Kind::Int { min: 12, max: 32, step: 1, unit: "px" }, def: 20 },
    Item { id: Id::BarraInicioCentro, page: Page::Barra, section: BARRA, key: "InicioCentrado",
        label: "Botón Inicio centrado",
        help: "Coloca Inicio y los botones de ventana en el centro, como Windows 11.",
        kind: Kind::Bool, def: 0 },
    Item { id: Id::BarraIconoInicio, page: Page::Barra, section: BARRA, key: "IconoInicio",
        label: "Icono del botón Inicio",
        help: "Extraído de una DLL del sistema, o dibujado con el color de acento.",
        kind: Kind::Choice(ICONOS_INICIO), def: 1 },
    Item { id: Id::BarraIconoIndice, page: Page::Barra, section: BARRA, key: "IconoInicioIndice",
        label: "Índice del icono dentro de la DLL",
        help: "276 es el logotipo de Windows en shell32.dll.",
        kind: Kind::Int { min: 0, max: 400, step: 1, unit: "" }, def: 276 },
    // ------------------------------------------------- área de notificación
    Item { id: Id::BandejaMostrar, page: Page::Bandeja, section: BANDEJA, key: "Mostrar",
        label: "Mostrar el área de notificación",
        help: "Sonido, red, batería, Bluetooth y notificaciones, junto al reloj.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::BandejaSonido, page: Page::Bandeja, section: BANDEJA, key: "Sonido",
        label: "Volumen", help: "Icono de altavoz con el nivel del dispositivo predeterminado.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::BandejaRed, page: Page::Bandeja, section: BANDEJA, key: "Red",
        label: "Red", help: "Wi-Fi (con su intensidad) o Ethernet, según la conexión activa.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::BandejaBateria, page: Page::Bandeja, section: BANDEJA, key: "Bateria",
        label: "Batería", help: "Se oculta sola en equipos sin batería.", kind: Kind::Bool, def: 1 },
    Item { id: Id::BandejaBluetooth, page: Page::Bandeja, section: BANDEJA, key: "Bluetooth",
        label: "Bluetooth", help: "Se oculta solo si el equipo no tiene radio Bluetooth.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::BandejaNotificaciones, page: Page::Bandeja, section: BANDEJA, key: "Notificaciones",
        label: "Centro de notificaciones", help: "Campana con el número de avisos sin leer.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::BandejaPorcentajeBateria, page: Page::Bandeja, section: BANDEJA, key: "PorcentajeBateria",
        label: "Mostrar el porcentaje de batería", help: "", kind: Kind::Bool, def: 1 },
    Item { id: Id::BandejaSeparador, page: Page::Bandeja, section: BANDEJA, key: "Separador",
        label: "Separador antes del reloj", help: "Línea fina entre los iconos y la hora.",
        kind: Kind::Bool, def: 1 },
    // ------------------------------------------------------- notificaciones
    Item { id: Id::NotifActivadas, page: Page::Notificaciones, section: NOTIFICACIONES, key: "Activadas",
        label: "Notificaciones", help: "Avisos emergentes de OptimizedShell y de las aplicaciones.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::NotifNoMolestar, page: Page::Notificaciones, section: NOTIFICACIONES, key: "NoMolestar",
        label: "No molestar",
        help: "Las notificaciones no aparecen en pantalla, pero sí en el centro de notificaciones.",
        kind: Kind::Bool, def: 0 },
    Item { id: Id::NotifDuracion, page: Page::Notificaciones, section: NOTIFICACIONES, key: "Duracion",
        label: "Tiempo en pantalla", help: "",
        kind: Kind::Int { min: 2000, max: 20000, step: 500, unit: "ms" }, def: 6000 },
    Item { id: Id::NotifMax, page: Page::Notificaciones, section: NOTIFICACIONES, key: "MaxVisibles",
        label: "Avisos visibles a la vez", help: "Los demás esperan su turno en la cola.",
        kind: Kind::Int { min: 1, max: 5, step: 1, unit: "" }, def: 3 },
    Item { id: Id::NotifAncho, page: Page::Notificaciones, section: NOTIFICACIONES, key: "Ancho",
        label: "Ancho de los avisos", help: "",
        kind: Kind::Int { min: 260, max: 520, step: 4, unit: "px" }, def: 360 },
    Item { id: Id::NotifPosicion, page: Page::Notificaciones, section: NOTIFICACIONES, key: "Posicion",
        label: "Esquina en la que aparecen", help: "",
        kind: Kind::Choice(ESQUINAS_NOTIF), def: 0 },
    Item { id: Id::NotifHistorial, page: Page::Notificaciones, section: NOTIFICACIONES, key: "Historial",
        label: "Avisos guardados en el centro", help: "",
        kind: Kind::Int { min: 0, max: 100, step: 5, unit: "" }, def: 30 },
    Item { id: Id::NotifSonido, page: Page::Notificaciones, section: NOTIFICACIONES, key: "Sonido",
        label: "Sonido al llegar un aviso", help: "Usa el sonido de notificación del sistema.",
        kind: Kind::Bool, def: 1 },
    // ------------------------------------- administrador de tareas y explorador
    Item { id: Id::TaskmgrIconos, page: Page::Listas, section: LISTAS, key: "IconosProcesos",
        label: "Iconos en la lista de procesos",
        help: "Solo se piden para las filas visibles, y se guardan en caché.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::ExplIconos, page: Page::Listas, section: LISTAS, key: "IconosExplorador",
        label: "Iconos reales en el explorador",
        help: "El icono que el shell asocia a cada tipo de archivo.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::ExplConfirmarBorrado, page: Page::Listas, section: LISTAS, key: "ConfirmarBorrado",
        label: "Confirmar al eliminar",
        help: "Los archivos van a la Papelera salvo que se use Mayús+Supr.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::ExplAutoRefresco, page: Page::Listas, section: LISTAS, key: "AutoRefresco",
        label: "Actualizar la carpeta sola",
        help: "Vigila los cambios del directorio (ReadDirectoryChangesW).",
        kind: Kind::Bool, def: 1 },
    // ---------------------------------------------------------- escritorio
    Item { id: Id::EscritorioActivo, page: Page::Escritorio, section: ESCRITORIO, key: "Activo",
        label: "Dibujar el escritorio",
        help: "Ventana de fondo con el papel tapiz. Se omite sola mientras el Explorador de Windows sea el shell.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::EscritorioAjuste, page: Page::Escritorio, section: ESCRITORIO, key: "Ajuste",
        label: "Ajuste de la imagen", help: "",
        kind: Kind::Choice(AJUSTES_FONDO), def: 0 },
    Item { id: Id::EscritorioColor, page: Page::Escritorio, section: ESCRITORIO, key: "Color",
        label: "Color de fondo",
        help: "Se ve cuando no hay imagen o cuando la imagen no cubre toda la pantalla.",
        kind: Kind::Color, def: 0x10141B },
    Item { id: Id::HotkeysActivas, page: Page::Escritorio, section: ESCRITORIO, key: "Atajos",
        label: "Atajos de teclado globales",
        help: "Win+E explorador, Win+R ejecutar, Win+I configuración, Win+D escritorio, Ctrl+Mayús+Esc administrador.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::BandejaIconosApps, page: Page::Bandeja, section: BANDEJA, key: "IconosApps",
        label: "Iconos de las aplicaciones",
        help: "Los que publican con Shell_NotifyIcon. Solo llegan si OptimizedShell es el shell.",
        kind: Kind::Bool, def: 1 },
    // ------------------------------------------- iconos del escritorio
    Item { id: Id::EscritorioIconos, page: Page::Escritorio, section: ESCRITORIO, key: "Iconos",
        label: "Mostrar los iconos del escritorio",
        help: "Al desactivarlo el escritorio queda limpio: solo el fondo.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::EscritorioTamanoIcono, page: Page::Escritorio, section: ESCRITORIO, key: "TamanoIcono",
        label: "Tamaño de los iconos", help: "Lado del icono, sin contar el rótulo.",
        kind: Kind::Int { min: 32, max: 96, step: 8, unit: "px" }, def: 48 },
    Item { id: Id::EscritorioEtiquetas, page: Page::Escritorio, section: ESCRITORIO, key: "Etiquetas",
        label: "Mostrar los nombres", help: "Sin rótulos los iconos ocupan bastante menos.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::EscritorioEquipo, page: Page::Escritorio, section: ESCRITORIO, key: "IconoEquipo",
        label: "Este equipo", help: "Unidades y carpetas del equipo.", kind: Kind::Bool, def: 1 },
    Item { id: Id::EscritorioPapelera, page: Page::Escritorio, section: ESCRITORIO, key: "IconoPapelera",
        label: "Papelera de reciclaje", help: "El icono cambia según esté vacía o no.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::EscritorioUsuario, page: Page::Escritorio, section: ESCRITORIO, key: "IconoUsuario",
        label: "Carpeta del usuario", help: "La carpeta personal (%USERPROFILE%).",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::EscritorioRed, page: Page::Escritorio, section: ESCRITORIO, key: "IconoRed",
        label: "Red", help: "Equipos y recursos compartidos de la red.", kind: Kind::Bool, def: 0 },
    Item { id: Id::EscritorioPanel, page: Page::Escritorio, section: ESCRITORIO, key: "IconoPanel",
        label: "Panel de control", help: "", kind: Kind::Bool, def: 0 },
    Item { id: Id::EscritorioArchivos, page: Page::Escritorio, section: ESCRITORIO, key: "Archivos",
        label: "Archivos y accesos directos del escritorio",
        help: "Lo que haya dentro de la carpeta Escritorio del usuario.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::EscritorioPublico, page: Page::Escritorio, section: ESCRITORIO, key: "ArchivosPublicos",
        label: "Incluir el escritorio público",
        help: "La carpeta Escritorio común a todos los usuarios.",
        kind: Kind::Bool, def: 1 },
    Item { id: Id::EscritorioOrden, page: Page::Escritorio, section: ESCRITORIO, key: "Orden",
        label: "Ordenar los iconos por",
        help: "Los iconos del sistema van primero y las carpetas antes que los archivos.",
        kind: Kind::Choice(ORDEN_ICONOS), def: 0 },
    Item { id: Id::EscritorioOcultos, page: Page::Escritorio, section: ESCRITORIO, key: "Ocultos",
        label: "Mostrar los archivos ocultos", help: "", kind: Kind::Bool, def: 0 },
    Item { id: Id::EscritorioAutoRefresco, page: Page::Escritorio, section: ESCRITORIO, key: "AutoRefresco",
        label: "Actualizar los iconos solos",
        help: "Vigila las carpetas Escritorio y relee cuando algo cambia.",
        kind: Kind::Bool, def: 1 },
];

pub fn item(id: Id) -> &'static Item {
    &ITEMS[id as usize]
}

/// Ajustes de una página, en el orden en que deben dibujarse.
pub fn items_of(page: Page) -> impl Iterator<Item = &'static Item> {
    ITEMS.iter().filter(move |i| i.page as usize == page as usize)
}

// ------------------------------------------------------------ configuración
#[derive(Clone, Copy, PartialEq)]
pub struct Config {
    pub v: [i32; COUNT],
}

impl Default for Config {
    fn default() -> Self {
        let mut v = [0i32; COUNT];
        for (i, it) in ITEMS.iter().enumerate() {
            v[i] = it.def;
        }
        Config { v }
    }
}

fn clamp_for(it: &Item, val: i32) -> i32 {
    match it.kind {
        Kind::Bool => (val != 0) as i32,
        Kind::Int { min, max, .. } => val.clamp(min, max),
        Kind::Choice(opts) | Kind::Cards(opts) => val.clamp(0, opts.len() as i32 - 1),
        Kind::Color => val & 0xFF_FFFF,
    }
}

impl Config {
    #[inline]
    pub fn get(&self, id: Id) -> i32 {
        self.v[id as usize]
    }
    #[inline]
    pub fn b(&self, id: Id) -> bool {
        self.v[id as usize] != 0
    }
    pub fn set(&mut self, id: Id, val: i32) {
        self.v[id as usize] = clamp_for(item(id), val);
    }
    pub fn toggle(&mut self, id: Id) {
        let v = self.v[id as usize] == 0;
        self.set(id, v as i32);
    }
    /// Restablece los valores por defecto de una página.
    pub fn reset_page(&mut self, page: Page) {
        for it in ITEMS.iter() {
            if it.page as usize == page as usize {
                self.v[it.id as usize] = it.def;
            }
        }
    }

    // atajos de lectura más usados
    pub fn dark(&self) -> bool {
        self.get(Id::Modo) == 0
    }
    /// Acento como COLORREF de Windows (0x00BBGGRR).
    pub fn accent_colorref(&self) -> u32 {
        let c = self.get(Id::Acento) as u32;
        let (r, g, b) = ((c >> 16) & 0xFF, (c >> 8) & 0xFF, c & 0xFF);
        r | (g << 8) | (b << 16)
    }
    pub fn set_accent_colorref(&mut self, cr: u32) {
        let (r, g, b) = (cr & 0xFF, (cr >> 8) & 0xFF, (cr >> 16) & 0xFF);
        self.set(Id::Acento, ((r << 16) | (g << 8) | b) as i32);
    }
    pub fn font_family(&self) -> &'static str {
        FONTS[(self.get(Id::Fuente) as usize).min(FONTS.len() - 1)]
    }
}

// ------------------------------------------------------------ estado global
static CURRENT: Mutex<Option<Config>> = Mutex::new(None);
// Espejos atómicos de los valores que se leen en cada llamada de dibujo.
static SCALE: AtomicI32 = AtomicI32::new(100);
static FONT_SCALE: AtomicI32 = AtomicI32::new(100);
static RADIUS: AtomicI32 = AtomicI32::new(10);
static FONT_IDX: AtomicI32 = AtomicI32::new(0);

/// Configuración actualmente activa.
pub fn current() -> Config {
    CURRENT.lock().unwrap().unwrap_or_default()
}

/// Publica una configuración en memoria (sin escribirla en disco).
pub fn set_current(c: Config) {
    SCALE.store(c.get(Id::EscalaInterfaz), Ordering::Relaxed);
    FONT_SCALE.store(c.get(Id::EscalaTexto), Ordering::Relaxed);
    RADIUS.store(c.get(Id::Radio), Ordering::Relaxed);
    FONT_IDX.store(c.get(Id::Fuente), Ordering::Relaxed);
    *CURRENT.lock().unwrap() = Some(c);
}

/// Escala de la interfaz en % (barata: lectura atómica).
#[inline]
pub fn scale_pct() -> i32 {
    SCALE.load(Ordering::Relaxed)
}

/// Escala del texto en %.
#[inline]
pub fn font_pct() -> i32 {
    FONT_SCALE.load(Ordering::Relaxed)
}

/// Factor de radio de esquina respecto al diseño base (10 px).
#[inline]
pub fn radius_factor() -> f32 {
    RADIUS.load(Ordering::Relaxed) as f32 / 10.0
}

/// Índice de la familia tipográfica activa.
#[inline]
pub fn font_index() -> i32 {
    FONT_IDX.load(Ordering::Relaxed)
}

// ------------------------------------------------------------ persistencia
pub fn config_dir() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_default();
    let dir = PathBuf::from(base).join("OptimizedShell");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.ini")
}

fn legacy_file() -> PathBuf {
    config_dir().join("theme.ini")
}

/// INI mínimo: pares `clave=valor`, con la sección actual como prefijo
/// `Seccion/Clave` para poder repetir claves entre secciones.
fn read_ini(path: &PathBuf) -> Vec<(String, String)> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut section = String::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            section = line.trim_matches(|c| c == '[' || c == ']').to_string();
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.push((format!("{}/{}", section, k.trim()), v.trim().to_string()));
        }
    }
    out
}

fn find<'a>(kv: &'a [(String, String)], section: &str, key: &str) -> Option<&'a str> {
    let full = format!("{}/{}", section, key);
    kv.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&full))
        .map(|(_, v)| v.as_str())
        // Tolerancia: clave sin sección o en otra sección.
        .or_else(|| {
            kv.iter()
                .find(|(k, _)| k.rsplit('/').next().unwrap_or("").eq_ignore_ascii_case(key))
                .map(|(_, v)| v.as_str())
        })
}

fn parse_value(it: &Item, raw: &str) -> Option<i32> {
    let raw = raw.trim();
    match it.kind {
        Kind::Color => u32::from_str_radix(raw.trim_start_matches('#'), 16).ok().map(|c| (c & 0xFF_FFFF) as i32),
        Kind::Bool => match raw.to_ascii_lowercase().as_str() {
            "1" | "si" | "sí" | "true" | "on" => Some(1),
            "0" | "no" | "false" | "off" => Some(0),
            _ => raw.parse().ok(),
        },
        Kind::Cards(opts) | Kind::Choice(opts) => {
            if let Some(i) = opts.iter().position(|o| o.eq_ignore_ascii_case(raw)) {
                return Some(i as i32);
            }
            raw.parse().ok()
        }
        Kind::Int { .. } => raw.parse().ok(),
    }
}

fn format_value(it: &Item, v: i32) -> String {
    match it.kind {
        Kind::Color => format!("{:06X}", v & 0xFF_FFFF),
        Kind::Cards(opts) | Kind::Choice(opts) => {
            opts.get(v as usize).map(|s| s.to_string()).unwrap_or_else(|| v.to_string())
        }
        _ => v.to_string(),
    }
}

/// Lee la configuración del disco (con los valores por defecto donde falte).
pub fn load() -> Config {
    let mut c = Config::default();
    let kv = read_ini(&config_file());
    for it in ITEMS.iter() {
        if let Some(raw) = find(&kv, it.section, it.key) {
            if let Some(v) = parse_value(it, raw) {
                c.v[it.id as usize] = clamp_for(it, v);
            }
        }
    }
    if kv.is_empty() {
        import_legacy(&mut c);
    }
    c
}

/// Importa el antiguo `theme.ini` ([Tema] Modo/Acento/Desenfoque/Opacidad...).
fn import_legacy(c: &mut Config) {
    let kv = read_ini(&legacy_file());
    if kv.is_empty() {
        return;
    }
    if let Some(m) = find(&kv, "Tema", "Modo") {
        c.set(Id::Modo, if m.eq_ignore_ascii_case("claro") { 1 } else { 0 });
    }
    if let Some(a) = find(&kv, "Tema", "Acento") {
        if let Ok(rgb) = u32::from_str_radix(a.trim_start_matches('#'), 16) {
            c.set(Id::Acento, (rgb & 0xFF_FFFF) as i32);
        }
    }
    let legacy_transp = find(&kv, "Tema", "Transparencia").and_then(|v| v.parse::<i32>().ok()).unwrap_or(1) != 0;
    let blur = find(&kv, "Tema", "Desenfoque")
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(if legacy_transp { 1 } else { 0 });
    c.set(Id::Desenfoque, blur);
    if let Some(v) = find(&kv, "Tema", "OpacidadBarra").and_then(|v| v.parse().ok()) {
        c.set(Id::OpacidadBarra, v);
    }
    if let Some(v) = find(&kv, "Tema", "OpacidadVentanas").and_then(|v| v.parse().ok()) {
        c.set(Id::OpacidadVentanas, v);
    }
}

/// Escribe la configuración en `config.ini`, agrupada por secciones.
pub fn save(c: &Config) {
    let mut sections: Vec<&'static str> = Vec::new();
    for it in ITEMS.iter() {
        if !sections.contains(&it.section) {
            sections.push(it.section);
        }
    }
    let mut out = String::from("; Configuración de OptimizedShell. Se puede editar a mano.\n; Los valores fuera de rango se recortan al cargar.\n");
    for sec in sections {
        out.push_str(&format!("\n[{}]\n", sec));
        for it in ITEMS.iter().filter(|i| i.section == sec) {
            out.push_str(&format!("{}={}\n", it.key, format_value(it, c.v[it.id as usize])));
        }
    }
    let _ = std::fs::write(config_file(), out);
}

// ------------------------------------------------------------ comprobaciones
#[cfg(test)]
mod tests {
    use super::*;

    /// `Id` se usa como índice de `ITEMS`: el orden debe coincidir.
    #[test]
    fn items_en_orden() {
        assert_eq!(ITEMS.len(), COUNT, "faltan o sobran filas en ITEMS");
        for (i, it) in ITEMS.iter().enumerate() {
            assert_eq!(it.id as usize, i, "ITEMS[{}] no corresponde a {:?}", i, it.id);
        }
    }

    /// Claves únicas dentro de cada sección del INI.
    #[test]
    fn claves_unicas() {
        for (i, a) in ITEMS.iter().enumerate() {
            for b in ITEMS.iter().skip(i + 1) {
                assert!(
                    !(a.section == b.section && a.key.eq_ignore_ascii_case(b.key)),
                    "clave repetida: [{}] {}", a.section, a.key
                );
            }
        }
    }

    #[test]
    fn valores_recortados() {
        let mut c = Config::default();
        c.set(Id::EscalaInterfaz, 1000);
        assert_eq!(c.get(Id::EscalaInterfaz), 200);
        c.set(Id::EscalaInterfaz, -5);
        assert_eq!(c.get(Id::EscalaInterfaz), 70);
        c.set(Id::Desenfoque, 7);
        assert_eq!(c.get(Id::Desenfoque), 1);
    }

    #[test]
    fn acento_ida_y_vuelta() {
        let mut c = Config::default();
        c.set_accent_colorref(colorref_test(10, 20, 30));
        assert_eq!(c.get(Id::Acento), 0x0A141E);
        assert_eq!(c.accent_colorref(), colorref_test(10, 20, 30));
    }

    fn colorref_test(r: u8, g: u8, b: u8) -> u32 {
        (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
    }

    /// Lo que se escribe en el INI se vuelve a leer igual.
    #[test]
    fn texto_ida_y_vuelta() {
        for it in ITEMS.iter() {
            let txt = format_value(it, it.def);
            let back = parse_value(it, &txt).expect("valor ilegible");
            assert_eq!(back, it.def, "{} no sobrevive al guardado", it.key);
        }
    }
}
