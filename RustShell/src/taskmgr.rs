// Administrador de tareas (--taskmgr). Port de CppShell/src/taskmgr.cpp.
//
// Marco propio con barra de título (icono + minimizar/maximizar/cerrar),
// pestañas Procesos · Rendimiento · Inicio · Servicios, barra de herramientas
// por pestaña con buscador, y tabla con columnas ordenables. La pestaña
// Procesos usa un único barrido de NtQuerySystemInformation (sin abrir un handle
// por proceso) para nombre, PID, %CPU (por delta), memoria, E/S y hilos.
//
// Las gráficas de Rendimiento y las pestañas Inicio/Servicios comparten la
// estructura pero su contenido completo se añadirá en iteraciones siguientes.
#![allow(non_snake_case, non_upper_case_globals)]

use std::collections::HashMap;
use std::ffi::c_void;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, ScreenToClient};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    OpenProcess, TerminateProcess, PROCESS_TERMINATE,
};
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_BACK, VK_DELETE, VK_ESCAPE,
};
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::ui::gdip::{Align, Chrome, Gfx};
use crate::ui::config::{self, Id};
use crate::ui::menu;
use crate::ui::{self, theme, window};
use crate::perf;
use crate::services;
use crate::startup;

// ---------------------------------------------------------------- Nt API
#[link(name = "ntdll")]
extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
}
const SYSTEM_PROCESS_INFORMATION_CLASS: u32 = 5;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}
#[repr(C)]
struct Spi {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: UnicodeString,
    base_priority: i32,
    unique_process_id: isize,
    inherited_from_unique_process_id: isize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
}

// ---------------------------------------------------------------- modelo
#[derive(Clone)]
struct Proc {
    pid: u32,
    name: String,
    ws: u64,
    threads: u32,
    cpu: f32,
}

// ---- pestañas y botones ----
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Proc = 0,
    Perf = 1,
    Startup = 2,
    Services = 3,
}
const TAB_NAMES: [&str; 4] = ["Procesos", "Rendimiento", "Inicio", "Servicios"];

// Identificadores de zonas interactivas (hover/click).
const B_NONE: i32 = 0;
const B_MIN: i32 = 1;
const B_MAX: i32 = 2;
const B_CLOSE: i32 = 3;
const B_TAB0: i32 = 10; // + índice de pestaña
const B_END: i32 = 20; // Finalizar tarea
const B_NEWTASK: i32 = 21; // Nueva tarea
const B_TOGGLE: i32 = 22; // Habilitar/Deshabilitar (Inicio)
const B_PERF0: i32 = 200; // + índice de la tarjeta de dispositivo (Rendimiento)

const B_SVC_START: i32 = 23;
const B_SVC_STOP: i32 = 24;
const B_SVC_RESTART: i32 = 25;
const B_FREERAM: i32 = 26; // Liberar RAM (Procesos)

const STARTUP_COLS: [Col; 4] = [
    Col { title: "Nombre", width: 240, right: false },
    Col { title: "Estado", width: 120, right: false },
    Col { title: "Origen", width: 200, right: false },
    Col { title: "Comando", width: 0, right: false },
];

const SERVICE_COLS: [Col; 5] = [
    Col { title: "Nombre", width: 0, right: false },
    Col { title: "Identificador", width: 190, right: false },
    Col { title: "PID", width: 76, right: true },
    Col { title: "Estado", width: 130, right: false },
    Col { title: "Tipo de inicio", width: 130, right: false },
];

fn columns(tab: Tab) -> &'static [Col] {
    match tab {
        Tab::Startup => &STARTUP_COLS,
        Tab::Services => &SERVICE_COLS,
        _ => &PROC_COLS,
    }
}

// ---- columnas de la tabla ----
struct Col {
    title: &'static str,
    width: i32, // 0 = ocupa el resto
    right: bool,
}
const PROC_COLS: [Col; 6] = [
    Col { title: "Nombre", width: 0, right: false },
    Col { title: "PID", width: 76, right: true },
    Col { title: "CPU", width: 76, right: true },
    Col { title: "Memoria", width: 104, right: true },
    Col { title: "E/S", width: 96, right: true },
    Col { title: "Hilos", width: 64, right: true },
];

// ------------------------------------------------- dispositivos de Rendimiento
#[derive(Clone, Copy, PartialEq)]
enum DevKind {
    Cpu,
    Mem,
    Gpu,
    Temp,
}

/// Una tarjeta de la pestaña Rendimiento (CPU, memoria, una GPU o un sensor).
struct PerfDev {
    /// Clave estable entre refrescos: por ella se conserva el historial.
    key: String,
    kind: DevKind,
    title: String,
    subtitle: String,
    /// Texto grande (ya formateado).
    display: String,
    /// Valor para la gráfica, en % del máximo.
    pct: f32,
    /// Escala del eje Y ("100 %", "100 °C", "24 GB"...).
    axis: String,
    detail: Vec<(String, String)>,
    /// Historial circular (0-100), el último valor es el más reciente.
    hist: Vec<f32>,
    /// Historial por núcleo (solo CPU).
    cores: Vec<Vec<f32>>,
    /// Sin datos: se dibuja el aviso en lugar de una gráfica a cero.
    missing: Option<String>,
}

// ---------------------------------------------------------------- estado
struct State {
    hwnd: HWND,
    tab: Tab,
    procs: Vec<Proc>,
    view: Vec<usize>, // índices filtrados+ordenados
    prev_cpu: HashMap<u32, i64>, // user+kernel previos por pid
    prev_tick: u64,
    cpus: f64,
    // tabla
    sort_col: usize,
    sort_desc: bool,
    scroll: i32,
    hover_row: i32,
    sel_pid: u32,
    // interacción
    hover_btn: i32,
    filter: String,
    // layout (en píxeles de cliente)
    tab_rc: [RECT; 4],
    search_rc: RECT,
    btn_end: RECT,
    btn_newtask: RECT,
    btn_toggle: RECT,
    btn_freeram: RECT,
    content: RECT,
    col_x: Vec<i32>,  // borde izquierdo de cada columna + fin
    // Inicio
    startup_items: Vec<startup::Entry>,
    startup_sel: isize,
    // Servicios
    services: Vec<services::Service>,
    svc_view: Vec<usize>,
    svc_sel: isize,
    /// Último mensaje del SCM (error de permisos, servicio detenido...).
    svc_msg: Option<String>,
    /// Resultado de «Liberar RAM», en la barra de la pestaña Procesos.
    proc_msg: Option<String>,
    btn_svc_start: RECT,
    btn_svc_stop: RECT,
    btn_svc_restart: RECT,
    // resumen
    mem_used: u64,
    mem_total: u64,
    mem_load: u32,
    cpu_total: f64,
    /// Porcentaje que el kernel apunta al «Proceso inactivo del sistema»
    /// (PID 0). No es un proceso: es el tiempo que la CPU pasa sin trabajo,
    /// así que no se lista, se enseña como dato de la CPU.
    cpu_idle: f64,
    // Rendimiento
    /// Muestreador abierto solo mientras la pestaña está a la vista.
    sampler: Option<perf::Sampler>,
    perf_devs: Vec<PerfDev>,
    perf_sel: usize,
    perf_list: RECT,
    perf_main: RECT,
    perf_cards: Vec<RECT>,
}


fn empty_rect() -> RECT {
    RECT { left: 0, top: 0, right: 0, bottom: 0 }
}
fn in_rect(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn format_bytes(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{} {}", bytes, U[0]) } else { format!("{:.1} {}", v, U[i]) }
}

// ---------------------------------------------------------------- muestreo
fn snapshot(st: &mut State) {
    let mut buf: Vec<u8> = vec![0u8; 512 * 1024];
    loop {
        let mut needed: u32 = 0;
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_PROCESS_INFORMATION_CLASS,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
                &mut needed,
            )
        };
        if status == 0xC000_0004u32 as i32 {
            buf.resize((needed as usize).max(buf.len() * 2), 0);
            continue;
        }
        if status < 0 {
            return;
        }
        break;
    }

    let now = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
    let dwall = (now.saturating_sub(st.prev_tick)) as f64; // ms
    let mut new_prev: HashMap<u32, i64> = HashMap::new();
    let mut procs = Vec::with_capacity(st.procs.len().max(64));
    let mut total_cpu = 0.0f64;
    let mut idle_cpu = 0.0f64;

    let mut offset = 0usize;
    loop {
        let p = unsafe { &*(buf.as_ptr().add(offset) as *const Spi) };
        let pid = p.unique_process_id as u32;
        let name = if p.image_name.buffer.is_null() || p.image_name.length == 0 {
            String::new()
        } else {
            let len = (p.image_name.length / 2) as usize;
            let slice = unsafe { std::slice::from_raw_parts(p.image_name.buffer, len) };
            String::from_utf16_lossy(slice)
        };
        let times = p.user_time + p.kernel_time; // 100 ns
        let mut cpu = 0.0f32;
        if st.prev_tick != 0 && dwall > 0.0 {
            if let Some(&prev) = st.prev_cpu.get(&pid) {
                let dcpu = (times - prev).max(0) as f64 / 1e4; // ms de CPU
                cpu = (dcpu / dwall / st.cpus * 100.0) as f32;
                if pid != 0 {
                    total_cpu += dcpu / dwall / st.cpus * 100.0;
                }
            }
        }
        new_prev.insert(pid, times);
        // El PID 0 («Proceso inactivo del sistema») no es un proceso: es la
        // cuenta del tiempo que cada núcleo pasa sin nada que hacer, y por eso
        // sale casi siempre al 99 %. El Administrador de tareas de Windows no
        // lo lista; aquí tampoco: su valor se enseña como «Inactivo» en la
        // pestaña Rendimiento, que es lo que de verdad significa.
        if pid == 0 {
            idle_cpu = cpu as f64;
        } else {
            procs.push(Proc { pid, name, ws: p.working_set_size as u64, threads: p.number_of_threads, cpu });
        }
        if p.next_entry_offset == 0 {
            break;
        }
        offset += p.next_entry_offset as usize;
    }

    st.procs = procs;
    st.prev_cpu = new_prev;
    st.prev_tick = now;
    st.cpu_total = total_cpu.min(100.0);
    st.cpu_idle = idle_cpu.clamp(0.0, 100.0);

    // memoria
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut m) != 0 {
            st.mem_total = m.ullTotalPhys;
            st.mem_used = m.ullTotalPhys - m.ullAvailPhys;
            st.mem_load = m.dwMemoryLoad;
        }
    }

    rebuild_view(st);
    // Las métricas de Rendimiento solo se toman si la pestaña está a la vista.
    sample_perf(st);
}

// ------------------------------------------------- muestreo de Rendimiento
fn gb(v: u64) -> String {
    format!("{:.1} GB", v as f64 / 1_073_741_824.0)
}

/// Puntos de historial que caben según los segundos elegidos y el refresco.
fn hist_points() -> usize {
    let c = config::current();
    let secs = c.get(Id::PerfHistorial).max(10) as i64;
    let refresh = c.get(Id::TaskmgrRefresco).max(100) as i64;
    ((secs * 1000 / refresh) as usize).clamp(20, 600)
}

fn push_hist(h: &mut Vec<f32>, v: f32, cap: usize) {
    h.push(v.clamp(0.0, 100.0));
    if h.len() > cap {
        let extra = h.len() - cap;
        h.drain(0..extra);
    }
}

/// Historial previo de una clave, para que cambiar de pestaña o de orden no
/// borre las gráficas.
fn take_hist(old: &mut [PerfDev], key: &str) -> (Vec<f32>, Vec<Vec<f32>>) {
    match old.iter_mut().find(|d| d.key == key) {
        Some(d) => (std::mem::take(&mut d.hist), std::mem::take(&mut d.cores)),
        None => (Vec::new(), Vec::new()),
    }
}

/// Toma una muestra y reconstruye las tarjetas de dispositivo.
fn sample_perf(st: &mut State) {
    let Some(sampler) = st.sampler.as_mut() else { return };
    let snap = sampler.sample();
    let info = sampler.info.clone();
    let cap = hist_points();
    let mut old = std::mem::take(&mut st.perf_devs);
    let mut devs: Vec<PerfDev> = Vec::new();

    // ---- CPU
    {
        let (mut hist, mut cores) = take_hist(&mut old, "cpu");
        push_hist(&mut hist, snap.cpu_total as f32, cap);
        cores.resize(snap.cpu_cores.len(), Vec::new());
        for (i, v) in snap.cpu_cores.iter().enumerate() {
            push_hist(&mut cores[i], *v as f32, cap);
        }
        let threads: u32 = st.procs.iter().map(|p| p.threads).sum();
        devs.push(PerfDev {
            key: "cpu".into(),
            kind: DevKind::Cpu,
            title: "CPU".into(),
            subtitle: if info.name.is_empty() { "Procesador".into() } else { info.name.clone() },
            display: format!("{:.0} %", snap.cpu_total),
            pct: snap.cpu_total as f32,
            axis: "100 %".into(),
            detail: vec![
                ("Uso".into(), format!("{:.1} %", snap.cpu_total)),
                ("Inactivo".into(), format!("{:.1} %", st.cpu_idle)),
                ("Núcleos lógicos".into(), info.logical.to_string()),
                ("Núcleos físicos".into(), info.physical.to_string()),
                ("Velocidad base".into(), if info.mhz > 0 { format!("{:.2} GHz", info.mhz as f64 / 1000.0) } else { "—".into() }),
                ("Procesos".into(), st.procs.len().to_string()),
                ("Hilos".into(), threads.to_string()),
            ],
            hist,
            cores,
            missing: None,
        });
    }

    // ---- Memoria
    {
        let (mut hist, _) = take_hist(&mut old, "mem");
        let m = &snap.mem;
        push_hist(&mut hist, m.load as f32, cap);
        devs.push(PerfDev {
            key: "mem".into(),
            kind: DevKind::Mem,
            title: "Memoria".into(),
            subtitle: format!("{} en uso de {}", gb(m.used), gb(m.total)),
            display: format!("{} %", m.load),
            pct: m.load as f32,
            axis: gb(m.total),
            detail: vec![
                ("En uso".into(), gb(m.used)),
                ("Disponible".into(), gb(m.total.saturating_sub(m.used))),
                ("Total".into(), gb(m.total)),
                ("Confirmación".into(), format!("{} / {}", gb(m.commit_used), gb(m.commit_total))),
                ("Uso".into(), format!("{} %", m.load)),
            ],
            hist,
            cores: Vec::new(),
            missing: None,
        });
    }

    // ---- GPUs
    if snap.gpus.is_empty() {
        let (hist, _) = take_hist(&mut old, "gpu:none");
        devs.push(PerfDev {
            key: "gpu:none".into(),
            kind: DevKind::Gpu,
            title: "GPU".into(),
            subtitle: "Sin datos".into(),
            display: "—".into(),
            pct: 0.0,
            axis: "100 %".into(),
            detail: Vec::new(),
            hist,
            cores: Vec::new(),
            missing: Some(if snap.gpu_counters {
                "No se ha encontrado ningún adaptador gráfico con contadores de uso.".into()
            } else {
                "Este Windows no expone los contadores «GPU Engine» (requiere Windows 10 1709 o posterior).".into()
            }),
        });
    }
    for (i, gpuv) in snap.gpus.iter().enumerate() {
        let key = format!("gpu:{}", gpuv.key);
        let (mut hist, _) = take_hist(&mut old, &key);
        push_hist(&mut hist, gpuv.usage as f32, cap);
        let mut detail = vec![
            ("Uso".into(), format!("{:.0} %", gpuv.usage)),
            ("Adaptador".into(), gpuv.name.clone()),
        ];
        if gpuv.mem_total > 0 {
            detail.push(("Memoria dedicada".into(), format!("{} / {}", gb(gpuv.mem_used), gb(gpuv.mem_total))));
        } else if gpuv.mem_used > 0 {
            detail.push(("Memoria dedicada".into(), gb(gpuv.mem_used)));
        }
        devs.push(PerfDev {
            key,
            kind: DevKind::Gpu,
            title: if snap.gpus.len() > 1 { format!("GPU {}", i) } else { "GPU".into() },
            subtitle: gpuv.name.clone(),
            display: format!("{:.0} %", gpuv.usage),
            pct: gpuv.usage as f32,
            axis: "100 %".into(),
            detail,
            hist,
            cores: Vec::new(),
            missing: None,
        });
    }

    // ---- Temperatura
    if snap.temps.is_empty() {
        let (hist, _) = take_hist(&mut old, "temp:none");
        devs.push(PerfDev {
            key: "temp:none".into(),
            kind: DevKind::Temp,
            title: "Temperatura".into(),
            subtitle: "Sin sensores".into(),
            display: "—".into(),
            pct: 0.0,
            axis: "100 °C".into(),
            detail: Vec::new(),
            hist,
            cores: Vec::new(),
            missing: Some(
                "El equipo no expone zonas térmicas ACPI al sistema. Muchos portátiles y placas \
                 solo publican la temperatura por su propio controlador, no por Windows."
                    .into(),
            ),
        });
    }
    for t in snap.temps.iter() {
        let (mut hist, _) = take_hist(&mut old, &t.key);
        // El eje llega a 100 °C: así la gráfica es comparable entre sensores.
        push_hist(&mut hist, t.celsius as f32, cap);
        devs.push(PerfDev {
            key: t.key.clone(),
            kind: DevKind::Temp,
            title: format!("Temp. {}", t.name),
            subtitle: "Zona térmica ACPI".into(),
            display: format!("{:.0} °C", t.celsius),
            pct: t.celsius as f32,
            axis: "100 °C".into(),
            detail: vec![
                ("Temperatura".into(), format!("{:.1} °C", t.celsius)),
                ("Sensor".into(), t.name.clone()),
            ],
            hist,
            cores: Vec::new(),
            missing: None,
        });
    }

    st.perf_devs = devs;
    if st.perf_sel >= st.perf_devs.len() {
        st.perf_sel = 0;
    }
    layout(st);
}

fn cell(p: &Proc, c: usize) -> String {
    match c {
        0 => if p.name.is_empty() { "(sin nombre)".to_string() } else { p.name.clone() },
        1 => p.pid.to_string(),
        2 => if p.cpu >= 0.05 { format!("{:.1} %", p.cpu) } else { "0 %".to_string() },
        3 => format_bytes(p.ws),
        4 => "0 KB/s".to_string(),
        5 => p.threads.to_string(),
        _ => String::new(),
    }
}

fn rebuild_view(st: &mut State) {
    let f = st.filter.to_lowercase();
    st.view = st
        .procs
        .iter()
        .enumerate()
        .filter(|(_, p)| f.is_empty() || p.name.to_lowercase().contains(&f))
        .map(|(i, _)| i)
        .collect();
    let col = st.sort_col;
    let desc = st.sort_desc;
    st.view.sort_by(|&a, &b| {
        let pa = &st.procs[a];
        let pb = &st.procs[b];
        let ord = match col {
            1 => pa.pid.cmp(&pb.pid),
            2 => pa.cpu.partial_cmp(&pb.cpu).unwrap_or(std::cmp::Ordering::Equal),
            3 => pa.ws.cmp(&pb.ws),
            5 => pa.threads.cmp(&pb.threads),
            _ => pa.name.to_lowercase().cmp(&pb.name.to_lowercase()),
        };
        if desc { ord.reverse() } else { ord }
    });
    let max = (st.view.len() as i32 - visible_rows(st)).max(0);
    st.scroll = st.scroll.clamp(0, max);
}

fn visible_rows(st: &State) -> i32 {
    (((st.content.bottom - st.content.top) - ui::s(30)) / row_h()).max(1)
}

// ---------------------------------------------------------------- layout
fn layout(st: &mut State) {
    let mut rc = empty_rect();
    unsafe { GetClientRect(st.hwnd, &mut rc) };
    let w = rc.right;
    let h = rc.bottom;

    // pestañas
    let mut tx = ui::s(12);
    for i in 0..4 {
        let tw = ui::s(24) + TAB_NAMES[i].chars().count() as i32 * ui::s(8);
        st.tab_rc[i] = RECT { left: tx, top: CAPTION_S() + ui::s(4), right: tx + tw, bottom: CAPTION_S() + ui::s(4) + ui::s(36) };
        tx += tw + ui::s(4);
    }
    // barra de herramientas
    let y = CAPTION_S() + ui::s(48);
    let bh = ui::s(32);
    st.search_rc = RECT { left: ui::s(14), top: y, right: ui::s(14) + ui::s(260), bottom: y + bh };
    st.btn_end = empty_rect();
    st.btn_newtask = empty_rect();
    st.btn_toggle = empty_rect();
    st.btn_freeram = empty_rect();
    let mut rx = w - ui::s(14);
    let right = |width: i32, rx: &mut i32| -> RECT {
        *rx -= width;
        let r = RECT { left: *rx, top: y, right: *rx + width, bottom: y + bh };
        *rx -= ui::s(8);
        r
    };
    match st.tab {
        Tab::Proc => {
            st.btn_end = right(ui::s(132), &mut rx);
            st.btn_newtask = right(ui::s(124), &mut rx);
            st.btn_freeram = right(ui::s(118), &mut rx);
        }
        Tab::Startup => {
            st.btn_toggle = right(ui::s(150), &mut rx);
        }
        Tab::Services => {
            st.btn_svc_restart = right(ui::s(112), &mut rx);
            st.btn_svc_stop = right(ui::s(100), &mut rx);
            st.btn_svc_start = right(ui::s(100), &mut rx);
        }
        _ => {}
    }
    // contenido
    st.content = RECT { left: ui::s(8), top: y + bh + ui::s(10), right: w - ui::s(8), bottom: h - ui::s(8) };

    // Rendimiento: lista de dispositivos a la izquierda, detalle a la derecha.
    if st.tab == Tab::Perf {
        let list_w = ui::s(250).min((st.content.right - st.content.left) / 3);
        st.perf_list = RECT { left: st.content.left + ui::s(4), top: st.content.top, right: st.content.left + list_w, bottom: st.content.bottom };
        st.perf_main = RECT { left: st.perf_list.right + ui::s(14), top: st.content.top, right: st.content.right - ui::s(6), bottom: st.content.bottom };
        let card_h = ui::s(62);
        let gap = ui::s(8);
        st.perf_cards = (0..st.perf_devs.len())
            .map(|i| {
                let top = st.perf_list.top + i as i32 * (card_h + gap);
                RECT { left: st.perf_list.left, top, right: st.perf_list.right, bottom: top + card_h }
            })
            .collect();
    }

    // columnas de la tabla (según la pestaña)
    let cols = columns(st.tab);
    let body_left = st.content.left + ui::s(8);
    let body_right = st.content.right - ui::s(10);
    let mut fixed = 0;
    for c in cols {
        fixed += ui::s(c.width);
    }
    let flex = (body_right - body_left - fixed - ui::s(8)).max(ui::s(120));
    st.col_x = vec![0; cols.len() + 1];
    let mut cx = body_left;
    for (i, c) in cols.iter().enumerate() {
        st.col_x[i] = cx;
        cx += if c.width == 0 { flex } else { ui::s(c.width) };
    }
    st.col_x[cols.len()] = cx;
}

#[allow(non_snake_case)]
fn CAPTION_S() -> i32 {
    ui::caption_h()
}

/// Alto de fila de las tablas (configurable en Configuración · Listas).
fn row_h() -> i32 {
    ui::s(config::current().get(Id::ListaAltoFila))
}

// ---------------------------------------------------------------- pintado
fn paint(st: &State) {
    window::paint_buffered(st.hwnd, |g: &Gfx, w, h| {
        let s = theme::style();
        let maximized = unsafe { IsZoomed(st.hwnd) != 0 };

        // marco + barra de título (DrawChrome)
        g.clear(theme::argb(0, 0, 0, 0));
        g.rounded(0.0, 0.0, w as f32, h as f32, if maximized { 0.0 } else { ui::sf(10.0) },
                  Some(s.window_bg), Some((s.card_border, 1.0)));
        let ch: Chrome = ui::gdip::chrome_rects(w);
        g.fill_rect(0, 0, w, ch.title_h, s.title_bg);
        g.hline(0, w, ch.title_h - 1, s.separator);
        // icono (recuadro con glifo)
        g.rounded(ui::s(14) as f32, ui::s(11) as f32, ui::s(20) as f32, ui::s(20) as f32, ui::sf(4.0),
                  Some(s.accent_solid), None);
        g.text_in("\u{E950}", ui::s(14) as f32, ui::s(11) as f32, ui::s(20) as f32, ui::s(20) as f32,
                  12.0, false, s.on_accent, Align::Center, Align::Center, true);
        g.text_in("Administrador de tareas", ui::s(42) as f32, 0.0, (w - ui::s(200)) as f32, ch.title_h as f32,
                  14.0, true, s.text, Align::Near, Align::Center, false);
        // botones min/max/cerrar
        chrome_button(g, &s, ch.min, "\u{E921}", st.hover_btn == B_MIN, false);
        chrome_button(g, &s, ch.max, if maximized { "\u{E923}" } else { "\u{E922}" }, st.hover_btn == B_MAX, false);
        chrome_button(g, &s, ch.close, "\u{E8BB}", st.hover_btn == B_CLOSE, true);

        // pestañas
        for i in 0..4 {
            let r = st.tab_rc[i];
            let act = i == st.tab as usize;
            let hov = st.hover_btn == B_TAB0 + i as i32;
            if hov && !act {
                g.rounded(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32,
                          ui::sf(6.0), Some(s.row_hover), None);
            }
            g.text_in(TAB_NAMES[i], r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32,
                      13.0, act, if act { s.text } else { s.text_dim }, Align::Center, Align::Center, false);
            if act {
                g.rounded((r.left + ui::s(12)) as f32, (r.bottom - ui::s(3)) as f32,
                          (r.right - r.left - ui::s(24)) as f32, ui::s(3) as f32, ui::sf(1.5),
                          Some(s.accent), None);
            }
        }
        // resumen a la derecha
        // Con la pestaña Rendimiento abierta se usa la medida del muestreador
        // (tiempos de procesador), más exacta que la suma de los procesos.
        let cpu = st.perf_devs.iter().find(|d| d.kind == DevKind::Cpu).map(|d| d.pct as f64).unwrap_or(st.cpu_total);
        let summary = format!(
            "CPU {:.0} %   ·   Memoria {} / {} ({} %)   ·   {} procesos",
            cpu, format_bytes(st.mem_used), format_bytes(st.mem_total), st.mem_load, st.procs.len()
        );
        let last = st.tab_rc[3];
        g.text_in(&summary, (last.right + ui::s(16)) as f32, st.tab_rc[0].top as f32,
                  (w - last.right - ui::s(32)) as f32, (st.tab_rc[0].bottom - st.tab_rc[0].top) as f32,
                  11.5, false, s.text_dim, Align::Far, Align::Center, false);
        g.hline(0, w, st.tab_rc[0].bottom + ui::s(2), s.separator);

        // barra de herramientas
        if st.tab != Tab::Perf {
            draw_field(g, &s, st.search_rc);
            g.text_in("\u{E721}", (st.search_rc.left + ui::s(8)) as f32, st.search_rc.top as f32,
                      ui::s(20) as f32, (st.search_rc.bottom - st.search_rc.top) as f32,
                      12.0, false, s.text_dim, Align::Center, Align::Center, true);
            let (txt, col) = if st.filter.is_empty() {
                ("Buscar".to_string(), s.text_faint)
            } else {
                (st.filter.clone(), s.text)
            };
            g.text_in(&txt, (st.search_rc.left + ui::s(34)) as f32, st.search_rc.top as f32,
                      (st.search_rc.right - st.search_rc.left - ui::s(42)) as f32,
                      (st.search_rc.bottom - st.search_rc.top) as f32, 12.0, false, col,
                      Align::Near, Align::Center, false);
        }
        match st.tab {
            Tab::Proc => {
                draw_button(g, &s, st.btn_end, "Finalizar tarea", st.hover_btn == B_END, false, true);
                draw_button(g, &s, st.btn_newtask, "Nueva tarea", st.hover_btn == B_NEWTASK, true, false);
                draw_button(g, &s, st.btn_freeram, "Liberar RAM", st.hover_btn == B_FREERAM,
                            false, false);
                // Resultado de la última liberación (o el aviso de elevación).
                if let Some(m) = &st.proc_msg {
                    g.text_in(m, (st.search_rc.right + ui::s(14)) as f32, st.search_rc.top as f32,
                              (st.btn_freeram.left - st.search_rc.right - ui::s(28)) as f32,
                              (st.search_rc.bottom - st.search_rc.top) as f32, 11.0, false,
                              s.text_dim, Align::Near, Align::Center, false);
                }
            }
            Tab::Startup => {
                let label = match st.startup_items.get(st.startup_sel.max(0) as usize) {
                    Some(e) if e.enabled => "Deshabilitar",
                    Some(_) => "Habilitar",
                    None => "Habilitar",
                };
                draw_button(g, &s, st.btn_toggle, label, st.hover_btn == B_TOGGLE, true, false);
            }
            Tab::Services => {
                let sel = selected_service(st);
                let running = sel.map(|s| s.state.running()).unwrap_or(false);
                let any = sel.is_some();
                draw_button(g, &s, st.btn_svc_start, "Iniciar", st.hover_btn == B_SVC_START,
                            any && !running, false);
                draw_button(g, &s, st.btn_svc_stop, "Detener", st.hover_btn == B_SVC_STOP, false,
                            any && running);
                draw_button(g, &s, st.btn_svc_restart, "Reiniciar", st.hover_btn == B_SVC_RESTART,
                            false, false);
                // Aviso del SCM (normalmente, falta de permisos).
                if let Some(m) = &st.svc_msg {
                    g.text_in(m, (st.search_rc.right + ui::s(14)) as f32, st.search_rc.top as f32,
                              (st.btn_svc_start.left - st.search_rc.right - ui::s(28)) as f32,
                              (st.search_rc.bottom - st.search_rc.top) as f32, 11.0, false, s.danger,
                              Align::Near, Align::Center, false);
                } else if !services::is_elevated() {
                    g.text_in("Inicia OptimizedShell como administrador para controlar servicios.",
                              (st.search_rc.right + ui::s(14)) as f32, st.search_rc.top as f32,
                              (st.btn_svc_start.left - st.search_rc.right - ui::s(28)) as f32,
                              (st.search_rc.bottom - st.search_rc.top) as f32, 11.0, false, s.text_faint,
                              Align::Near, Align::Center, false);
                }
            }
            _ => {}
        }

        // cuerpo según la pestaña
        match st.tab {
            Tab::Proc => paint_table(g, &s, st),
            Tab::Perf => paint_perf(g, &s, st),
            Tab::Startup => paint_startup(g, &s, st),
            Tab::Services => paint_services(g, &s, st),
        }
    });
}

fn chrome_button(g: &Gfx, s: &theme::Style, rc: (i32, i32, i32, i32), glyph: &str, hov: bool, close: bool) {
    let (x, y, w, h) = rc;
    if hov {
        g.fill_rect(x, y, w, h, if close { s.close_hover } else { s.control_hover });
    }
    let col = if hov && close { theme::rgbc(255, 255, 255) } else { s.text_dim };
    g.text_in(glyph, x as f32, y as f32, w as f32, h as f32, 10.0, false, col, Align::Center, Align::Center, true);
}

fn draw_field(g: &Gfx, s: &theme::Style, rc: RECT) {
    g.rounded(rc.left as f32, rc.top as f32, (rc.right - rc.left) as f32, (rc.bottom - rc.top) as f32,
              ui::sf(6.0), Some(s.control_bg), Some((s.control_border, 1.0)));
}

fn draw_button(g: &Gfx, s: &theme::Style, rc: RECT, label: &str, hov: bool, primary: bool, danger: bool) {
    if rc.right <= rc.left {
        return;
    }
    let (mut fill, mut border, mut fg) = (if hov { s.control_hover } else { s.control_bg }, s.control_border, s.text);
    if primary {
        fill = if hov { s.accent_hover } else { s.accent_solid };
        border = theme::argb(0, 0, 0, 0);
        fg = s.on_accent;
    }
    if danger {
        fill = if hov { theme::mix(s.control_bg, s.danger, 0.28, 255) } else { theme::mix(s.control_bg, s.danger, 0.14, 255) };
        border = theme::with_alpha(s.danger, 130);
        fg = s.danger;
    }
    g.rounded(rc.left as f32, rc.top as f32, (rc.right - rc.left) as f32, (rc.bottom - rc.top) as f32,
              ui::sf(6.0), Some(fill), Some((border, 1.0)));
    g.text_in(label, rc.left as f32, rc.top as f32, (rc.right - rc.left) as f32, (rc.bottom - rc.top) as f32,
              13.0, false, fg, Align::Center, Align::Center, false);
}

// ------------------------------------------------- pintado de Rendimiento
fn dev_color(s: &theme::Style, d: &PerfDev) -> u32 {
    match d.kind {
        DevKind::Mem => s.success,
        DevKind::Temp => {
            // El color avisa antes que el número.
            if d.pct >= 85.0 { s.danger } else if d.pct >= 70.0 { s.warning } else { s.success }
        }
        _ => s.accent,
    }
}

/// Gráfica de historial: fondo, cuadrícula, área y curva.
fn draw_graph(g: &Gfx, s: &theme::Style, rc: RECT, hist: &[f32], color: u32, grid: bool, width: f32) {
    let (x0, y0) = (rc.left as f32, rc.top as f32);
    let w = (rc.right - rc.left) as f32;
    let h = (rc.bottom - rc.top) as f32;
    if w <= 2.0 || h <= 2.0 {
        return;
    }
    g.rounded(x0, y0, w, h, ui::sf(6.0), Some(s.graph_bg), Some((s.card_border, 1.0)));
    if grid {
        for i in 1..4 {
            let y = y0 + h * i as f32 / 4.0;
            g.line(x0 + 1.0, y, x0 + w - 1.0, y, s.grid_line, 1.0);
        }
        for i in 1..6 {
            let x = x0 + w * i as f32 / 6.0;
            g.line(x, y0 + 1.0, x, y0 + h - 1.0, s.grid_line, 1.0);
        }
    }
    let n = hist.len();
    if n < 2 {
        return;
    }
    // El eje X es fijo (los segundos configurados): el historial se va
    // llenando desde la derecha, como en el administrador de Windows.
    let cap = hist_points().max(2) as f32;
    let step = w / (cap - 1.0);
    let pts: Vec<(f32, f32)> = hist
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = (x0 + w - (n - 1 - i) as f32 * step).max(x0);
            (x, y0 + h - (v / 100.0).clamp(0.0, 1.0) * h)
        })
        .collect();
    let mut poly = pts.clone();
    poly.push((pts[pts.len() - 1].0, y0 + h));
    poly.push((pts[0].0, y0 + h));
    g.fill_poly(&poly, theme::with_alpha(color, 64));
    g.draw_lines(&pts, color, width);
}

fn paint_perf(g: &Gfx, s: &theme::Style, st: &State) {
    if st.perf_devs.is_empty() {
        g.text_in("Tomando la primera muestra...", st.content.left as f32, (st.content.top + ui::s(20)) as f32,
                  (st.content.right - st.content.left) as f32, ui::s(28) as f32, 13.0, false, s.text_faint,
                  Align::Center, Align::Center, false);
        return;
    }

    // ---- columna izquierda: una tarjeta por dispositivo
    for (i, d) in st.perf_devs.iter().enumerate() {
        let Some(r) = st.perf_cards.get(i) else { continue };
        if r.bottom > st.perf_list.bottom {
            break;
        }
        let sel = i == st.perf_sel;
        let hov = st.hover_btn == B_PERF0 + i as i32;
        let col = dev_color(s, d);
        g.rounded(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32,
                  ui::sf(8.0), Some(if sel { s.row_selected } else if hov { s.row_hover } else { s.card_bg }),
                  Some((if sel { col } else { s.card_border }, if sel { 1.6 } else { 1.0 })));
        // mini gráfica de fondo, en la mitad derecha de la tarjeta
        // La mini gráfica ocupa la mitad derecha, por encima del subtítulo.
        let mini = RECT {
            left: r.left + (r.right - r.left) * 55 / 100,
            top: r.top + ui::s(8),
            right: r.right - ui::s(10),
            bottom: r.bottom - ui::s(22),
        };
        if d.missing.is_none() {
            draw_graph(g, s, mini, &d.hist, col, false, ui::sf(1.2));
        }
        g.text_in(&d.title, (r.left + ui::s(12)) as f32, (r.top + ui::s(6)) as f32,
                  ((r.right - r.left) * 55 / 100 - ui::s(14)) as f32, ui::s(20) as f32, 12.5, true,
                  s.text, Align::Near, Align::Center, false);
        g.text_in(&d.display, (r.left + ui::s(12)) as f32, (r.top + ui::s(26)) as f32,
                  ((r.right - r.left) * 55 / 100 - ui::s(14)) as f32, ui::s(22) as f32, 15.0, true, col,
                  Align::Near, Align::Center, false);
        g.text_in(&d.subtitle, (r.left + ui::s(12)) as f32, (r.bottom - ui::s(20)) as f32,
                  ((r.right - r.left) - ui::s(20)) as f32, ui::s(16) as f32, 10.0, false, s.text_faint,
                  Align::Near, Align::Center, false);
    }

    // ---- panel derecho: dispositivo seleccionado
    let Some(d) = st.perf_devs.get(st.perf_sel) else { return };
    let m = st.perf_main;
    let col = dev_color(s, d);
    g.text_in(&d.title, m.left as f32, m.top as f32, ((m.right - m.left) / 2) as f32, ui::s(30) as f32,
              19.0, true, s.text, Align::Near, Align::Center, false);
    g.text_in(&d.subtitle, m.left as f32, (m.top + ui::s(28)) as f32, ((m.right - m.left) * 2 / 3) as f32,
              ui::s(20) as f32, 11.5, false, s.text_dim, Align::Near, Align::Center, false);
    g.text_in(&d.display, (m.right - ui::s(200)) as f32, m.top as f32, ui::s(200) as f32, ui::s(40) as f32,
              24.0, true, col, Align::Far, Align::Center, false);

    let top = m.top + ui::s(54);
    if let Some(msg) = &d.missing {
        let box_rc = RECT { left: m.left, top, right: m.right, bottom: m.bottom };
        g.rounded(box_rc.left as f32, box_rc.top as f32, (box_rc.right - box_rc.left) as f32,
                  (box_rc.bottom - box_rc.top) as f32, ui::sf(8.0), Some(s.graph_bg), Some((s.card_border, 1.0)));
        g.text_in("\u{E783}", box_rc.left as f32, (box_rc.top + ui::s(40)) as f32,
                  (box_rc.right - box_rc.left) as f32, ui::s(40) as f32, 22.0, false, s.text_faint,
                  Align::Center, Align::Center, true);
        g.text_in("Sin datos disponibles", box_rc.left as f32, (box_rc.top + ui::s(86)) as f32,
                  (box_rc.right - box_rc.left) as f32, ui::s(26) as f32, 14.0, true, s.text_dim,
                  Align::Center, Align::Center, false);
        g.text_in(msg, (box_rc.left + ui::s(40)) as f32, (box_rc.top + ui::s(112)) as f32,
                  (box_rc.right - box_rc.left - ui::s(80)) as f32, ui::s(60) as f32, 12.0, false, s.text_faint,
                  Align::Center, Align::Near, false);
        return;
    }

    // Reparto vertical: gráfica grande, (opcional) rejilla por núcleo y datos.
    let rows = (d.detail.len() as i32 + 1) / 2;
    let detail_h = rows * ui::s(24) + ui::s(14);
    let show_cores = d.kind == DevKind::Cpu
        && config::current().b(Id::PerfPorNucleo)
        && !d.cores.is_empty();
    let avail = (m.bottom - top - detail_h).max(ui::s(120));
    let cores_h = if show_cores { (avail * 38 / 100).max(ui::s(70)) } else { 0 };
    let graph_h = avail - cores_h - if show_cores { ui::s(26) } else { 0 };
    let graph = RECT { left: m.left, top, right: m.right, bottom: top + graph_h };
    draw_graph(g, s, graph, &d.hist, col, true, ui::sf(1.8));
    // etiquetas de los ejes
    g.text_in(&d.axis, (graph.left + ui::s(6)) as f32, (graph.top + ui::s(2)) as f32, ui::s(90) as f32,
              ui::s(16) as f32, 10.0, false, s.text_faint, Align::Near, Align::Near, false);
    g.text_in("0", (graph.left + ui::s(6)) as f32, (graph.bottom - ui::s(18)) as f32, ui::s(60) as f32,
              ui::s(16) as f32, 10.0, false, s.text_faint, Align::Near, Align::Near, false);
    g.text_in(&format!("{} s", config::current().get(Id::PerfHistorial)),
              (graph.left + ui::s(6)) as f32, (graph.bottom - ui::s(18)) as f32,
              (graph.right - graph.left - ui::s(14)) as f32, ui::s(16) as f32, 10.0, false, s.text_faint,
              Align::Far, Align::Near, false);

    let mut y = graph.bottom + ui::s(12);
    if show_cores {
        g.text_in("Núcleos lógicos", m.left as f32, y as f32, (m.right - m.left) as f32, ui::s(16) as f32,
                  10.5, true, s.text_dim, Align::Near, Align::Center, false);
        y += ui::s(18);
        let n = d.cores.len() as i32;
        let cols = if n <= 4 { n.max(1) } else if n <= 8 { 4 } else { 8 };
        let crows = (n + cols - 1) / cols;
        let gap = ui::s(6);
        let cw = ((m.right - m.left) - (cols - 1) * gap) / cols;
        let chh = ((cores_h - ui::s(18)) - (crows - 1) * gap) / crows.max(1);
        for (i, hist) in d.cores.iter().enumerate() {
            let cx = m.left + (i as i32 % cols) * (cw + gap);
            let cy = y + (i as i32 / cols) * (chh + gap);
            let rc = RECT { left: cx, top: cy, right: cx + cw, bottom: cy + chh };
            if rc.bottom > m.bottom - detail_h {
                break;
            }
            draw_graph(g, s, rc, hist, col, false, ui::sf(1.2));
            g.text_in(&format!("{}", i), (rc.left + ui::s(4)) as f32, (rc.top + ui::s(1)) as f32,
                      ui::s(24) as f32, ui::s(14) as f32, 9.0, false, s.text_faint, Align::Near, Align::Near, false);
        }
        y += cores_h - ui::s(18) + ui::s(10);
    }

    // ---- datos del dispositivo, en dos columnas
    let colw = (m.right - m.left) / 2;
    for (i, (k, v)) in d.detail.iter().enumerate() {
        let cx = m.left + (i as i32 % 2) * colw;
        let cy = y + (i as i32 / 2) * ui::s(24);
        if cy + ui::s(20) > m.bottom {
            break;
        }
        g.text_in(k, cx as f32, cy as f32, (colw / 2) as f32, ui::s(22) as f32, 11.5, false, s.text_dim,
                  Align::Near, Align::Center, false);
        g.text_in(v, (cx + colw / 2) as f32, cy as f32, (colw / 2 - ui::s(16)) as f32, ui::s(22) as f32,
                  11.5, true, s.text, Align::Near, Align::Center, false);
    }
}

fn paint_table(g: &Gfx, s: &theme::Style, st: &State) {
    let hx = &st.col_x;
    let header_top = st.content.top;
    let header_h = ui::s(30);
    // cabecera
    for (i, c) in PROC_COLS.iter().enumerate() {
        let mut rx = hx[i] + ui::s(2);
        let mut rw = hx[i + 1] - hx[i] - ui::s(10);
        if i == st.sort_col && c.right {
            rw -= ui::s(18);
        }
        let col = if i == st.sort_col { s.text } else { s.text_dim };
        g.text_in(c.title, rx as f32, header_top as f32, rw as f32, header_h as f32, 11.5, true, col,
                  if c.right { Align::Far } else { Align::Near }, Align::Center, false);
        let _ = &mut rx;
        if i == st.sort_col {
            let ax = hx[i + 1] - ui::s(24);
            g.text_in(if st.sort_desc { "\u{E74B}" } else { "\u{E74A}" }, ax as f32, header_top as f32,
                      ui::s(14) as f32, header_h as f32, 9.0, false, s.accent, Align::Center, Align::Center, true);
        }
    }
    g.hline(st.content.left, st.content.right, header_top + header_h - 1, s.separator);

    // filas
    let body_top = header_top + header_h;
    let body_right = st.content.right - ui::s(10);
    let vis = visible_rows(st);
    for row in 0..vis {
        let idx = (st.scroll + row) as usize;
        if idx >= st.view.len() {
            break;
        }
        let p = &st.procs[st.view[idx]];
        let ry = body_top + row * row_h();
        let rr_x = st.content.left + ui::s(2);
        let rr_w = body_right - rr_x;
        let rr_h = row_h() - ui::s(2);
        if p.pid == st.sel_pid {
            g.rounded(rr_x as f32, ry as f32, rr_w as f32, rr_h as f32, ui::sf(5.0), Some(s.row_selected), None);
        } else if row == st.hover_row {
            g.rounded(rr_x as f32, ry as f32, rr_w as f32, rr_h as f32, ui::sf(5.0), Some(s.row_hover), None);
        }
        // Icono del ejecutable (solo para las filas visibles, y cacheado).
        let icons = config::current().b(Id::TaskmgrIconos);
        let isz = ui::s(16);
        if icons {
            let icon = proc_icon(p.pid);
            if icon.is_null() {
                g.text_in("\u{E7C3}", (hx[0] + ui::s(2)) as f32, ry as f32, isz as f32, rr_h as f32,
                          10.0, false, s.text_faint, Align::Center, Align::Center, true);
            } else {
                g.draw_hicon(icon, hx[0] + ui::s(2), ry + (rr_h - isz) / 2, isz);
            }
        }
        for (c, col) in PROC_COLS.iter().enumerate() {
            let extra = if c == 0 && icons { isz + ui::s(8) } else { 0 };
            let cx = hx[c] + ui::s(2) + extra;
            let cw = hx[c + 1] - hx[c] - ui::s(10) - extra;
            let color = match c {
                0 => s.text,
                2 => if p.cpu >= 50.0 { s.danger } else if p.cpu >= 15.0 { s.warning } else { s.text_dim },
                3 => if p.ws >= 1024 * 1024 * 1024 { s.warning } else { s.text_dim },
                _ => s.text_dim,
            };
            g.text_in(&cell(p, c), cx as f32, ry as f32, cw as f32, rr_h as f32, 11.5, false, color,
                      if col.right { Align::Far } else { Align::Near }, Align::Center, false);
        }
    }
    if st.view.is_empty() {
        let msg = if st.filter.is_empty() { "Cargando..." } else { "Ningún proceso coincide." };
        g.text_in(msg, st.content.left as f32, (body_top + ui::s(20)) as f32,
                  (st.content.right - st.content.left) as f32, ui::s(28) as f32, 13.0, false, s.text_faint,
                  Align::Center, Align::Center, false);
    }
}

fn paint_startup(g: &Gfx, s: &theme::Style, st: &State) {
    let cols = &STARTUP_COLS;
    let hx = &st.col_x;
    let header_top = st.content.top;
    let header_h = ui::s(30);
    for (i, c) in cols.iter().enumerate() {
        g.text_in(c.title, (hx[i] + ui::s(2)) as f32, header_top as f32,
                  (hx[i + 1] - hx[i] - ui::s(10)) as f32, header_h as f32, 11.5, true, s.text_dim,
                  Align::Near, Align::Center, false);
    }
    g.hline(st.content.left, st.content.right, header_top + header_h - 1, s.separator);
    let body_top = header_top + header_h;
    let vis = visible_rows(st);
    for row in 0..vis {
        let idx = (st.scroll + row) as usize;
        if idx >= st.startup_items.len() {
            break;
        }
        let e = &st.startup_items[idx];
        let ry = body_top + row * row_h();
        let rr_x = st.content.left + ui::s(2);
        let rr_w = st.content.right - ui::s(10) - rr_x;
        if idx as isize == st.startup_sel {
            g.rounded(rr_x as f32, ry as f32, rr_w as f32, (row_h() - ui::s(2)) as f32, ui::sf(5.0), Some(s.row_selected), None);
        }
        let vals = [
            e.name.clone(),
            if e.enabled { "Habilitado".to_string() } else { "Deshabilitado".to_string() },
            e.location.clone(),
            e.command.clone(),
        ];
        for (c, v) in vals.iter().enumerate() {
            let color = if c == 1 { if e.enabled { s.success } else { s.text_faint } } else if c == 0 { s.text } else { s.text_dim };
            g.text_in(v, (hx[c] + ui::s(2)) as f32, ry as f32, (hx[c + 1] - hx[c] - ui::s(10)) as f32,
                      (row_h() - ui::s(2)) as f32, 11.5, false, color, Align::Near, Align::Center, false);
        }
    }
    if st.startup_items.is_empty() {
        g.text_in("No hay programas de inicio.", st.content.left as f32, (body_top + ui::s(20)) as f32,
                  (st.content.right - st.content.left) as f32, ui::s(28) as f32, 13.0, false, s.text_faint,
                  Align::Center, Align::Center, false);
    }
}

// ---------------------------------------------------------------- servicios
fn load_services(st: &mut State) {
    st.services = services::enumerate();
    // El tipo de inicio se consulta servicio a servicio, así que solo se pide
    // para lo que de verdad se va a ver.
    services::fill_start_types(&mut st.services);
    rebuild_services(st);
}

fn rebuild_services(st: &mut State) {
    let f = st.filter.to_lowercase();
    st.svc_view = st
        .services
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            f.is_empty() || s.display.to_lowercase().contains(&f) || s.name.to_lowercase().contains(&f)
        })
        .map(|(i, _)| i)
        .collect();
    if st.svc_sel >= st.svc_view.len() as isize {
        st.svc_sel = -1;
    }
    let max = (st.svc_view.len() as i32 - visible_rows(st)).max(0);
    st.scroll = st.scroll.clamp(0, max);
}

fn selected_service(st: &State) -> Option<&services::Service> {
    let i = *st.svc_view.get(st.svc_sel.max(0) as usize)?;
    st.services.get(i)
}

/// Ejecuta una acción del SCM sobre el servicio seleccionado y refresca su
/// estado, dejando el mensaje de error a la vista si lo hubo.
fn service_action(st: &mut State, what: i32) {
    let Some(svc) = selected_service(st) else { return };
    let name = svc.name.clone();
    let r = match what {
        B_SVC_START => services::start(&name),
        B_SVC_STOP => services::stop(&name),
        _ => services::restart(&name),
    };
    st.svc_msg = match r {
        Ok(()) => None,
        Err(e) => Some(e),
    };
    // Solo se vuelve a preguntar por este servicio, no por los 300 del equipo.
    let state = services::query_state(&name);
    if let Some(s) = st.services.iter_mut().find(|s| s.name == name) {
        s.state = state;
        if !state.running() {
            s.pid = 0;
        }
    }
}

fn paint_services(g: &Gfx, s: &theme::Style, st: &State) {
    let cols = &SERVICE_COLS;
    let hx = &st.col_x;
    let header_top = st.content.top;
    let header_h = ui::s(30);
    for (i, c) in cols.iter().enumerate() {
        g.text_in(c.title, (hx[i] + ui::s(2)) as f32, header_top as f32,
                  (hx[i + 1] - hx[i] - ui::s(10)) as f32, header_h as f32, 11.5, true, s.text_dim,
                  if c.right { Align::Far } else { Align::Near }, Align::Center, false);
    }
    g.hline(st.content.left, st.content.right, header_top + header_h - 1, s.separator);

    let body_top = header_top + header_h;
    let vis = visible_rows(st);
    for row in 0..vis {
        let idx = (st.scroll + row) as usize;
        if idx >= st.svc_view.len() {
            break;
        }
        let sv = &st.services[st.svc_view[idx]];
        let ry = body_top + row * row_h();
        let rr_x = st.content.left + ui::s(2);
        let rr_w = st.content.right - ui::s(10) - rr_x;
        let rr_h = row_h() - ui::s(2);
        if idx as isize == st.svc_sel {
            g.rounded(rr_x as f32, ry as f32, rr_w as f32, rr_h as f32, ui::sf(5.0), Some(s.row_selected), None);
        } else if row == st.hover_row {
            g.rounded(rr_x as f32, ry as f32, rr_w as f32, rr_h as f32, ui::sf(5.0), Some(s.row_hover), None);
        }
        // Punto de estado delante del nombre.
        let dot = if sv.state.running() { s.success } else { s.text_faint };
        g.fill_circle((hx[0] + ui::s(8)) as f32, (ry + rr_h / 2) as f32, ui::sf(3.5), dot);
        let vals = [
            sv.display.clone(),
            sv.name.clone(),
            if sv.pid == 0 { String::new() } else { sv.pid.to_string() },
            sv.state.label().to_string(),
            sv.start.label().to_string(),
        ];
        for (c, v) in vals.iter().enumerate() {
            let color = match c {
                0 => s.text,
                3 => if sv.state.running() { s.success } else { s.text_dim },
                4 => if sv.start == services::StartType::Disabled { s.text_faint } else { s.text_dim },
                _ => s.text_dim,
            };
            let x0 = hx[c] + if c == 0 { ui::s(18) } else { ui::s(2) };
            let w = hx[c + 1] - x0 - ui::s(10);
            g.text_in(v, x0 as f32, ry as f32, w as f32, rr_h as f32, 11.5, false, color,
                      if cols[c].right { Align::Far } else { Align::Near }, Align::Center, false);
        }
    }
    if st.svc_view.is_empty() {
        let msg = if st.services.is_empty() {
            "No se pudo consultar el administrador de servicios."
        } else {
            "Ningún servicio coincide."
        };
        g.text_in(msg, st.content.left as f32, (body_top + ui::s(20)) as f32,
                  (st.content.right - st.content.left) as f32, ui::s(28) as f32, 13.0, false, s.text_faint,
                  Align::Center, Align::Center, false);
    }
}

// ------------------------------------------------- acciones sobre un proceso
const PRIORITY_LEVELS: [(u32, &str); 6] = [
    (0x0000_0100, "Tiempo real"),
    (0x0000_0080, "Alta"),
    (0x0000_8000, "Por encima de lo normal"),
    (0x0000_0020, "Normal"),
    (0x0000_4000, "Por debajo de lo normal"),
    (0x0000_0040, "Baja"),
];

/// Abre el proceso con los permisos necesarios para consultarlo y cambiarlo.
fn open_for_tuning(pid: u32) -> windows_sys::Win32::Foundation::HANDLE {
    use windows_sys::Win32::System::Threading::{
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    };
    unsafe { OpenProcess(PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) }
}

fn process_priority(pid: u32) -> u32 {
    use windows_sys::Win32::System::Threading::GetPriorityClass;
    unsafe {
        let h = open_for_tuning(pid);
        if h.is_null() {
            return 0;
        }
        let p = GetPriorityClass(h);
        CloseHandle(h);
        p
    }
}

fn set_process_priority(pid: u32, class: u32) -> bool {
    use windows_sys::Win32::System::Threading::SetPriorityClass;
    unsafe {
        let h = open_for_tuning(pid);
        if h.is_null() {
            return false;
        }
        let ok = SetPriorityClass(h, class) != 0;
        CloseHandle(h);
        ok
    }
}

/// Máscara de afinidad del proceso y del sistema.
fn process_affinity(pid: u32) -> Option<(usize, usize)> {
    use windows_sys::Win32::System::Threading::GetProcessAffinityMask;
    unsafe {
        let h = open_for_tuning(pid);
        if h.is_null() {
            return None;
        }
        let mut pm: usize = 0;
        let mut sm: usize = 0;
        let ok = GetProcessAffinityMask(h, &mut pm, &mut sm) != 0;
        CloseHandle(h);
        if ok { Some((pm, sm)) } else { None }
    }
}

fn set_process_affinity(pid: u32, mask: usize) -> bool {
    use windows_sys::Win32::System::Threading::SetProcessAffinityMask;
    unsafe {
        let h = open_for_tuning(pid);
        if h.is_null() {
            return false;
        }
        let ok = SetProcessAffinityMask(h, mask) != 0;
        CloseHandle(h);
        ok
    }
}

/// ¿El proceso está en modo de eficiencia (EcoQoS)?
fn process_eco(pid: u32) -> bool {
    use windows_sys::Win32::System::Threading::{
        GetProcessInformation, ProcessPowerThrottling, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_STATE,
    };
    unsafe {
        let h = open_for_tuning(pid);
        if h.is_null() {
            return false;
        }
        let mut stt: PROCESS_POWER_THROTTLING_STATE = std::mem::zeroed();
        let ok = GetProcessInformation(h, ProcessPowerThrottling, &mut stt as *mut _ as *mut c_void,
                                       std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32) != 0;
        CloseHandle(h);
        ok && stt.StateMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0
    }
}

fn set_process_eco(pid: u32, on: bool) -> bool {
    use windows_sys::Win32::System::Threading::{
        SetProcessInformation, ProcessPowerThrottling, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
    };
    unsafe {
        let h = open_for_tuning(pid);
        if h.is_null() {
            return false;
        }
        let stt = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: if on { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
        };
        let ok = SetProcessInformation(h, ProcessPowerThrottling, &stt as *const _ as *const c_void,
                                       std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32) != 0;
        // EcoQoS implica prioridad baja: es lo que hace el propio Windows.
        if ok && on {
            set_process_priority(pid, 0x0000_4000);
        }
        CloseHandle(h);
        ok
    }
}

/// Ruta del ejecutable de un proceso (cacheada por PID).
fn process_path(pid: u32) -> String {
    use windows_sys::Win32::System::Threading::{
        QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    PROC_PATHS.with(|c| {
        if let Some(p) = c.borrow().get(&pid) {
            return p.clone();
        }
        let mut out = String::new();
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if !h.is_null() {
                let mut buf = [0u16; 512];
                let mut len = buf.len() as u32;
                if QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0 {
                    out = String::from_utf16_lossy(&buf[..len as usize]);
                }
                CloseHandle(h);
            }
        }
        c.borrow_mut().insert(pid, out.clone());
        out
    })
}

thread_local! {
    /// Ruta del ejecutable por PID: no cambia mientras el proceso vive.
    static PROC_PATHS: std::cell::RefCell<HashMap<u32, String>> =
        std::cell::RefCell::new(HashMap::new());
    /// Icono por PID (el del ejecutable, cacheado por ruta en `apps`).
    static PROC_ICONS: std::cell::RefCell<HashMap<u32, isize>> =
        std::cell::RefCell::new(HashMap::new());
}

/// Icono del ejecutable de un proceso. Se resuelve solo para las filas que se
/// ven y se cachea: pedirlo para los 300 procesos en cada refresco no.
fn proc_icon(pid: u32) -> HICON {
    PROC_ICONS.with(|c| {
        if let Some(&h) = c.borrow().get(&pid) {
            return h as HICON;
        }
        let path = process_path(pid);
        let icon = if path.is_empty() {
            std::ptr::null_mut()
        } else {
            crate::apps::icon(&path)
        };
        let mut map = c.borrow_mut();
        // Tope de seguridad, como en el menú Inicio.
        if map.len() < 512 {
            map.insert(pid, icon as isize);
        }
        icon
    })
}

/// Abre el Explorador en la carpeta del ejecutable del proceso.
fn open_process_location(pid: u32) {
    let path = process_path(pid);
    if path.is_empty() {
        return;
    }
    if let Some(dir) = std::path::Path::new(&path).parent() {
        crate::shell::launch_own(&format!("--explorer \"{}\"", dir.to_string_lossy()));
    }
}

// ---------------------------------------------------------------- liberar RAM
// «Liberar RAM» hace dos cosas, de menos a más invasivo:
//
//   1. Recorta el conjunto de trabajo de cada proceso al que se puede abrir un
//      handle (`EmptyWorkingSet`): Windows devuelve a la lista de disponibles
//      las páginas que ese proceso no está usando ahora mismo. No mata nada ni
//      pierde datos: lo que haga falta otra vez se vuelve a paginar.
//   2. Si hay elevación, vacía además las listas de memoria en espera y
//      modificada del sistema (`NtSetSystemInformation`), que es lo que de
//      verdad baja el «en uso» cuando la caché se ha comido la RAM.
//
// Sin elevación solo se hace el paso 1 y se dice por qué.
#[link(name = "ntdll")]
extern "system" {
    fn NtSetSystemInformation(class: u32, info: *mut c_void, len: u32) -> i32;
}

/// SystemMemoryListInformation.
const SYSTEM_MEMORY_LIST_CLASS: u32 = 80;
/// Órdenes de SYSTEM_MEMORY_LIST_COMMAND.
const MEMORY_FLUSH_MODIFIED_LIST: u32 = 3;
const MEMORY_PURGE_STANDBY_LIST: u32 = 4;

/// Memoria física en uso ahora mismo, en bytes.
fn physical_in_use() -> u64 {
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut m) == 0 {
            return 0;
        }
        m.ullTotalPhys - m.ullAvailPhys
    }
}

/// Recorta el conjunto de trabajo de un proceso. Devuelve true si se pudo.
fn trim_process(pid: u32) -> bool {
    use windows_sys::Win32::System::ProcessStatus::EmptyWorkingSet;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
    };
    if pid == 0 || pid == 4 {
        // El proceso inactivo y el System no se tocan.
        return false;
    }
    unsafe {
        let h = OpenProcess(PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let ok = EmptyWorkingSet(h) != 0;
        CloseHandle(h);
        ok
    }
}

/// Vacía las listas de memoria en espera y modificada. Necesita elevación
/// (SeProfileSingleProcessPrivilege); devuelve false si Windows la rechaza.
fn purge_system_lists() -> bool {
    unsafe {
        let mut cmd: u32 = MEMORY_FLUSH_MODIFIED_LIST;
        let flushed = NtSetSystemInformation(SYSTEM_MEMORY_LIST_CLASS,
                                             &mut cmd as *mut _ as *mut c_void, 4) >= 0;
        let mut cmd: u32 = MEMORY_PURGE_STANDBY_LIST;
        let purged = NtSetSystemInformation(SYSTEM_MEMORY_LIST_CLASS,
                                            &mut cmd as *mut _ as *mut c_void, 4) >= 0;
        flushed || purged
    }
}

/// Libera memoria y deja el resultado en la barra de herramientas.
fn free_ram(st: &mut State) {
    let before = physical_in_use();
    let pids: Vec<u32> = st.procs.iter().map(|p| p.pid).collect();
    let mut trimmed = 0usize;
    for pid in pids {
        if trim_process(pid) {
            trimmed += 1;
        }
    }
    // El propio administrador de tareas también.
    ui::trim_working_set();
    let lists = if services::is_elevated() { purge_system_lists() } else { false };

    // Windows tarda un instante en devolver las páginas a la lista de libres.
    std::thread::sleep(std::time::Duration::from_millis(250));
    let after = physical_in_use();
    let freed = before.saturating_sub(after);

    let detalle = if lists {
        "listas del sistema incluidas"
    } else if services::is_elevated() {
        "sin tocar las listas del sistema"
    } else {
        "como administrador se libera más"
    };
    st.proc_msg = Some(if freed > 0 {
        format!("Liberados {} ({} procesos, {}).", format_bytes(freed), trimmed, detalle)
    } else {
        format!("Nada que liberar ({} procesos, {}).", trimmed, detalle)
    });
    snapshot(st);
}

/// Menú contextual de un proceso: finalizar, ubicación, prioridad, eficiencia
/// y afinidad.
fn process_menu(st: &mut State, screen_x: i32, screen_y: i32) {
    let pid = st.sel_pid;
    if pid == 0 {
        return;
    }
    let name = st
        .procs
        .iter()
        .find(|p| p.pid == pid)
        .map(|p| p.name.clone())
        .unwrap_or_default();
    let cur = process_priority(pid);
    let eco = process_eco(pid);
    let path = process_path(pid);

    let mut items = vec![
        menu::Item::header(&format!("{} ({})", name, pid)),
        menu::Item::new(1, "Finalizar tarea").glyph("\u{E711}").danger(),
        menu::Item::new(2, "Abrir ubicación del archivo")
            .glyph("\u{E8DA}")
            .enabled(!path.is_empty()),
        menu::Item::new(3, "Copiar ruta").glyph("\u{E8C8}").enabled(!path.is_empty()),
        menu::Item::sep(),
        menu::Item::header("Prioridad"),
    ];
    for (i, (class, label)) in PRIORITY_LEVELS.iter().enumerate() {
        items.push(menu::Item::new(10 + i as i32, label).checked(cur == *class));
    }
    items.push(menu::Item::sep());
    items.push(
        menu::Item::new(20, "Modo de eficiencia (EcoQoS)")
            .glyph("\u{E945}")
            .checked(eco),
    );
    items.push(
        menu::Item::new(21, "Liberar la memoria de este proceso")
            .glyph("\u{EA3A}")
            .hint("EmptyWorkingSet"),
    );
    items.push(menu::Item::new(30, "Afinidad de procesadores...").glyph("\u{E950}"));

    match menu::popup(st.hwnd, screen_x, screen_y, items) {
        1 => end_selected(st),
        2 => open_process_location(pid),
        3 => {
            ui::set_clipboard_text(&path);
        }
        id @ 10..=15 => {
            set_process_priority(pid, PRIORITY_LEVELS[(id - 10) as usize].0);
        }
        20 => {
            set_process_eco(pid, !eco);
        }
        21 => {
            let before = physical_in_use();
            let ok = trim_process(pid);
            std::thread::sleep(std::time::Duration::from_millis(150));
            let freed = before.saturating_sub(physical_in_use());
            st.proc_msg = Some(if !ok {
                format!("No se pudo abrir el proceso {} (puede que haga falta elevación).", pid)
            } else if freed > 0 {
                format!("{}: liberados {}.", name, format_bytes(freed))
            } else {
                format!("{}: nada que liberar ahora mismo.", name)
            });
            snapshot(st);
        }
        30 => affinity_menu(st, screen_x, screen_y, pid),
        _ => {}
    }
}

/// Segundo menú: una casilla por procesador lógico y «Aplicar».
fn affinity_menu(st: &State, x: i32, y: i32, pid: u32) {
    let Some((mask, system)) = process_affinity(pid) else { return };
    let cores = (0..usize::BITS as usize).filter(|i| system & (1 << i) != 0).collect::<Vec<_>>();
    if cores.is_empty() {
        return;
    }
    let mut items = vec![menu::Item::header("Procesadores permitidos")];
    for &c in &cores {
        items.push(
            menu::Item::new(100 + c as i32, &format!("CPU {}", c))
                .checked(mask & (1 << c) != 0)
                .keep_open(),
        );
    }
    items.push(menu::Item::sep());
    items.push(menu::Item::new(1, "Todos los procesadores").glyph("\u{E73E}"));
    items.push(menu::Item::new(2, "Aplicar").glyph("\u{E930}"));

    let out = menu::popup_ex(st.hwnd, x, y, items);
    match out.id {
        1 => {
            set_process_affinity(pid, system);
        }
        2 => {
            // Las casillas están en el mismo orden en que se construyeron.
            let mut m: usize = 0;
            for (i, &c) in cores.iter().enumerate() {
                if out.checked.get(i + 1).copied().unwrap_or(false) {
                    m |= 1 << c;
                }
            }
            if m != 0 {
                set_process_affinity(pid, m);
            }
        }
        _ => {}
    }
}

/// Menú contextual de un servicio: iniciar, detener, reiniciar y tipo de inicio.
fn service_menu(st: &mut State, x: i32, y: i32) {
    let Some(sv) = selected_service(st) else { return };
    let name = sv.name.clone();
    let display = sv.display.clone();
    let running = sv.state.running();
    let pid = sv.pid;
    let cur = sv.start;

    let items = vec![
        menu::Item::header(&display),
        menu::Item::new(1, "Iniciar").glyph("\u{E768}").enabled(!running),
        menu::Item::new(2, "Detener").glyph("\u{E71A}").enabled(running),
        menu::Item::new(3, "Reiniciar").glyph("\u{E72C}").enabled(running),
        menu::Item::sep(),
        menu::Item::new(4, "Ver el proceso").glyph("\u{E9D9}").enabled(pid != 0),
        menu::Item::new(5, "Copiar el identificador").glyph("\u{E8C8}"),
        menu::Item::sep(),
        menu::Item::header("Tipo de inicio"),
        menu::Item::new(10, "Automático").checked(cur == services::StartType::Automatic),
        menu::Item::new(11, "Manual").checked(cur == services::StartType::Manual),
        menu::Item::new(12, "Deshabilitado").checked(cur == services::StartType::Disabled),
    ];
    let id = menu::popup(st.hwnd, x, y, items);
    match id {
        1 => service_action(st, B_SVC_START),
        2 => service_action(st, B_SVC_STOP),
        3 => service_action(st, B_SVC_RESTART),
        4 => {
            // Salta a Procesos con ese PID seleccionado.
            st.sel_pid = pid;
            switch_tab(st, Tab::Proc);
            snapshot(st);
        }
        5 => {
            ui::set_clipboard_text(&name);
        }
        10 | 11 | 12 => {
            let t = match id {
                10 => services::StartType::Automatic,
                11 => services::StartType::Manual,
                _ => services::StartType::Disabled,
            };
            st.svc_msg = match services::set_start_type(&name, t) {
                Ok(()) => {
                    if let Some(s) = st.services.iter_mut().find(|s| s.name == name) {
                        s.start = t;
                    }
                    None
                }
                Err(e) => Some(e),
            };
        }
        _ => {}
    }
}

fn load_startup(st: &mut State) {
    st.startup_items = startup::enumerate();
    if st.startup_sel >= st.startup_items.len() as isize {
        st.startup_sel = -1;
    }
}

fn toggle_startup(st: &mut State) {
    if let Some(e) = st.startup_items.get(st.startup_sel.max(0) as usize) {
        let want = !e.enabled;
        let e = e.clone();
        if startup::set_enabled(&e, want) == 0 {
            load_startup(st);
        }
    }
}

// ---------------------------------------------------------------- interacción
fn hit_test_btn(st: &State, x: i32, y: i32) -> i32 {
    let mut rc = empty_rect();
    unsafe { GetClientRect(st.hwnd, &mut rc) };
    let ch = ui::gdip::chrome_rects(rc.right);
    let hit = |r: (i32, i32, i32, i32)| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3;
    if hit(ch.close) {
        return B_CLOSE;
    }
    if hit(ch.max) {
        return B_MAX;
    }
    if hit(ch.min) {
        return B_MIN;
    }
    for i in 0..4 {
        if in_rect(&st.tab_rc[i], x, y) {
            return B_TAB0 + i as i32;
        }
    }
    match st.tab {
        Tab::Proc => {
            if in_rect(&st.btn_end, x, y) {
                return B_END;
            }
            if in_rect(&st.btn_newtask, x, y) {
                return B_NEWTASK;
            }
            if in_rect(&st.btn_freeram, x, y) {
                return B_FREERAM;
            }
        }
        Tab::Startup => {
            if in_rect(&st.btn_toggle, x, y) {
                return B_TOGGLE;
            }
        }
        Tab::Services => {
            if in_rect(&st.btn_svc_start, x, y) {
                return B_SVC_START;
            }
            if in_rect(&st.btn_svc_stop, x, y) {
                return B_SVC_STOP;
            }
            if in_rect(&st.btn_svc_restart, x, y) {
                return B_SVC_RESTART;
            }
        }
        Tab::Perf => {
            for (i, r) in st.perf_cards.iter().enumerate() {
                if in_rect(r, x, y) {
                    return B_PERF0 + i as i32;
                }
            }
        }

    }
    B_NONE
}

fn row_count(st: &State) -> i32 {
    match st.tab {
        Tab::Startup => st.startup_items.len() as i32,
        Tab::Services => st.svc_view.len() as i32,
        _ => st.view.len() as i32,
    }
}

fn row_at(st: &State, x: i32, y: i32) -> i32 {
    let body_top = st.content.top + ui::s(30);
    if x < st.content.left || x >= st.content.right || y < body_top || y >= st.content.bottom {
        return -1;
    }
    let r = st.scroll + (y - body_top) / row_h();
    if r < row_count(st) { r } else { -1 }
}

fn col_at(st: &State, x: i32, y: i32) -> i32 {
    let header_top = st.content.top;
    if y < header_top || y >= header_top + ui::s(30) {
        return -1;
    }
    for i in 0..PROC_COLS.len() {
        if x >= st.col_x[i] && x < st.col_x[i + 1] {
            return i as i32;
        }
    }
    -1
}

fn end_selected(st: &State) {
    if st.sel_pid == 0 {
        return;
    }
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, st.sel_pid);
        if !h.is_null() {
            TerminateProcess(h, 1);
            CloseHandle(h);
        }
    }
}

fn new_task() {
    // Lanza el diálogo de ejecución del sistema.
    let cmd = ui::wide("shell:::{2559a1f3-21d7-11d4-bdaf-00c04f60b9f0}");
    unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            ui::wide("explorer.exe").as_ptr(),
            cmd.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

fn switch_tab(st: &mut State, t: Tab) {
    if st.tab == t {
        return;
    }
    st.tab = t;
    st.filter.clear();
    st.scroll = 0;
    st.proc_msg = None;
    if t == Tab::Startup {
        load_startup(st);
    }
    if t == Tab::Services {
        st.svc_msg = None;
        load_services(st);
    } else {
        // La lista de servicios no se mantiene viva fuera de su pestaña.
        st.services.clear();
        st.svc_view.clear();
    }
    // El muestreador de Rendimiento (PDH incluido) solo vive mientras se ve la
    // pestaña: fuera de ella no cuesta nada.
    if t == Tab::Perf {
        if st.sampler.is_none() {
            st.sampler = Some(perf::Sampler::new());
        }
        layout(st);
        sample_perf(st);
    } else {
        st.sampler = None;
        st.perf_devs.clear();
        st.perf_cards.clear();
    }
    layout(st);
}

// ---------------------------------------------------------------- wndproc
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let st_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    // Cambio de configuración: relee tema, fuentes, alto de fila e intervalo.
    if msg == ui::config_changed_msg() {
        let c = ui::reload_config();
        window::refresh_frame(hwnd);
        SetTimer(hwnd, 1, c.get(Id::TaskmgrRefresco) as u32, None);
        if !st_ptr.is_null() {
            layout(&mut *st_ptr);
        }
        InvalidateRect(hwnd, std::ptr::null(), 0);
        return 0;
    }
    match msg {
        WM_NCCALCSIZE => {
            if wp != 0 {
                // Marco propio: el área cliente ocupa toda la ventana.
                if IsZoomed(hwnd) != 0 {
                    let p = &mut *(lp as *mut NCCALCSIZE_PARAMS);
                    let fx = GetSystemMetrics(SM_CXFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
                    let fy = GetSystemMetrics(SM_CYFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
                    p.rgrc[0].left += fx;
                    p.rgrc[0].right -= fx;
                    p.rgrc[0].top += fy;
                    p.rgrc[0].bottom -= fy;
                }
                return 0;
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_NCHITTEST => {
            let mut pt = POINT { x: (lp & 0xFFFF) as i16 as i32, y: ((lp >> 16) & 0xFFFF) as i16 as i32 };
            ScreenToClient(hwnd, &mut pt);
            let mut rc = empty_rect();
            GetClientRect(hwnd, &mut rc);
            if IsZoomed(hwnd) == 0 {
                let b = ui::s(6);
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
            if pt.y < CAPTION_S() {
                if !st_ptr.is_null() && hit_test_btn(&*st_ptr, pt.x, pt.y) != B_NONE {
                    return HTCLIENT as LRESULT;
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
        WM_TIMER => {
            if !st_ptr.is_null() {
                snapshot(&mut *st_ptr);
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
        WM_MOUSEMOVE => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                let hb = hit_test_btn(st, x, y);
                let hr = row_at(st, x, y);
                if hb != st.hover_btn || hr != st.hover_row {
                    st.hover_btn = hb;
                    st.hover_row = hr;
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                let mut tme: TRACKMOUSEEVENT = std::mem::zeroed();
                tme.cbSize = std::mem::size_of::<TRACKMOUSEEVENT>() as u32;
                tme.dwFlags = TME_LEAVE;
                tme.hwndTrack = hwnd;
                TrackMouseEvent(&mut tme);
            }
            0
        }
        WM_MOUSELEAVE => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                st.hover_btn = B_NONE;
                st.hover_row = -1;
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                let btn = hit_test_btn(st, x, y);
                match btn {
                    B_CLOSE => { PostMessageW(hwnd, WM_CLOSE, 0, 0); return 0; }
                    B_MIN => { ShowWindow(hwnd, SW_MINIMIZE); return 0; }
                    B_MAX => { ShowWindow(hwnd, if IsZoomed(hwnd) != 0 { SW_RESTORE } else { SW_MAXIMIZE }); return 0; }
                    B_END => { end_selected(st); return 0; }
                    B_NEWTASK => { new_task(); return 0; }
                    B_FREERAM => { free_ram(st); InvalidateRect(hwnd, std::ptr::null(), 0); return 0; }
                    B_TOGGLE => { toggle_startup(st); InvalidateRect(hwnd, std::ptr::null(), 0); return 0; }
                    B_SVC_START | B_SVC_STOP | B_SVC_RESTART => {
                        service_action(st, btn);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                        return 0;
                    }
                    b if b >= B_PERF0 => {
                        st.perf_sel = (b - B_PERF0) as usize;
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                        return 0;
                    }
                    b if b >= B_TAB0 && b < B_TAB0 + 4 => {
                        let t = match b - B_TAB0 { 0 => Tab::Proc, 1 => Tab::Perf, 2 => Tab::Startup, _ => Tab::Services };
                        switch_tab(st, t);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                        return 0;
                    }
                    _ => {}
                }
                // cabecera de la tabla (ordenar)
                if st.tab == Tab::Proc {
                    let c = col_at(st, x, y);
                    if c >= 0 {
                        let c = c as usize;
                        if st.sort_col == c {
                            st.sort_desc = !st.sort_desc;
                        } else {
                            st.sort_col = c;
                            st.sort_desc = PROC_COLS[c].right;
                        }
                        rebuild_view(st);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                        return 0;
                    }
                    let r = row_at(st, x, y);
                    if r >= 0 {
                        st.sel_pid = st.procs[st.view[r as usize]].pid;
                        SetFocus(hwnd);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                } else if st.tab == Tab::Startup {
                    let r = row_at(st, x, y);
                    if r >= 0 {
                        st.startup_sel = r as isize;
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                } else if st.tab == Tab::Services {
                    let r = row_at(st, x, y);
                    if r >= 0 {
                        st.svc_sel = r as isize;
                        st.svc_msg = None;
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                }
            }
            0
        }
        WM_RBUTTONUP => {
            // Menú contextual: acciones sobre el proceso o el servicio de la fila.
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                let r = row_at(st, x, y);
                if r >= 0 {
                    let mut pt = POINT { x, y };
                    windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut pt);
                    match st.tab {
                        Tab::Proc => {
                            st.sel_pid = st.procs[st.view[r as usize]].pid;
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                            process_menu(st, pt.x, pt.y);
                            snapshot(st);
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        }
                        Tab::Services => {
                            st.svc_sel = r as isize;
                            st.svc_msg = None;
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                            service_menu(st, pt.x, pt.y);
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        }
                        _ => {}
                    }
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let delta = ((wp >> 16) & 0xFFFF) as i16 as i32;
                let max = (row_count(st) - visible_rows(st)).max(0);
                st.scroll = (st.scroll - (delta / 120) * 3).clamp(0, max);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_CHAR => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let ch = wp as u32;
                let changed = if ch == VK_BACK as u32 {
                    st.filter.pop();
                    true
                } else if ch >= 0x20 {
                    match char::from_u32(ch) {
                        Some(c) => {
                            st.filter.push(c);
                            true
                        }
                        None => false,
                    }
                } else {
                    false
                };
                if changed {
                    if st.tab == Tab::Services {
                        rebuild_services(st);
                    } else {
                        rebuild_view(st);
                    }
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_KEYDOWN => {
            if !st_ptr.is_null() {
                let st = &mut *st_ptr;
                let vk = wp as u32;
                if vk == VK_DELETE as u32 {
                    end_selected(st);
                } else if vk == VK_ESCAPE as u32 && !st.filter.is_empty() {
                    st.filter.clear();
                    if st.tab == Tab::Services {
                        rebuild_services(st);
                    } else {
                        rebuild_view(st);
                    }
                    InvalidateRect(hwnd, std::ptr::null(), 0);
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

        let class_name = ui::wide("OShellRustTaskMgr");
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

        // tamaño y posición centrada en el área de trabajo
        let mut wa = empty_rect();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut _ as *mut c_void, 0);
        let w = ui::s(1060).min(wa.right - wa.left - ui::s(40));
        let h = ui::s(700).min(wa.bottom - wa.top - ui::s(40));
        let x = wa.left + (wa.right - wa.left - w) / 2;
        let y = wa.top + (wa.bottom - wa.top - h) / 2;

        let title = ui::wide("Administrador de tareas");
        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            x, y, w, h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return 1;
        }

        let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
        // Pestaña inicial por argumento: `--taskmgr rendimiento`.
        let tab = match crate::args::positional().unwrap_or_default().to_lowercase().as_str() {
            "rendimiento" | "perf" => Tab::Perf,
            "inicio" | "startup" => Tab::Startup,
            "servicios" | "services" => Tab::Services,
            _ => Tab::Proc,
        };
        let st = Box::new(State {
            hwnd,
            tab,
            procs: Vec::new(),
            view: Vec::new(),
            prev_cpu: HashMap::new(),
            prev_tick: 0,
            cpus,
            sort_col: 2,
            sort_desc: true,
            scroll: 0,
            hover_row: -1,
            sel_pid: 0,
            hover_btn: B_NONE,
            filter: String::new(),
            tab_rc: [empty_rect(); 4],
            search_rc: empty_rect(),
            btn_end: empty_rect(),
            btn_newtask: empty_rect(),
            btn_toggle: empty_rect(),
            btn_freeram: empty_rect(),
            content: empty_rect(),
            col_x: Vec::new(),
            startup_items: Vec::new(),
            startup_sel: -1,
            services: Vec::new(),
            svc_view: Vec::new(),
            svc_sel: -1,
            svc_msg: None,
            proc_msg: None,
            btn_svc_start: empty_rect(),
            btn_svc_stop: empty_rect(),
            btn_svc_restart: empty_rect(),
            mem_used: 0,
            mem_total: 0,
            mem_load: 0,
            cpu_total: 0.0,
            cpu_idle: 0.0,
            sampler: None,
            perf_devs: Vec::new(),
            perf_sel: 0,
            perf_list: empty_rect(),
            perf_main: empty_rect(),
            perf_cards: Vec::new(),
        });
        let st_raw = Box::into_raw(st);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, st_raw as isize);

        window::refresh_frame(hwnd);
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0,
                     SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
        layout(&mut *st_raw);
        // Cada pestaña carga lo suyo también cuando es la de partida
        // (`--taskmgr servicios`), no solo al cambiar a ella.
        match (*st_raw).tab {
            Tab::Perf => (*st_raw).sampler = Some(perf::Sampler::new()),
            Tab::Startup => load_startup(&mut *st_raw),
            Tab::Services => load_services(&mut *st_raw),
            Tab::Proc => {}
        }
        ui::menu::register(hinst);
        snapshot(&mut *st_raw); // primer muestreo (sin %CPU)
        snapshot(&mut *st_raw); // segundo para tener %CPU inmediato no es posible aún; se llenará en el timer
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetTimer(hwnd, 1, config::current().get(Id::TaskmgrRefresco) as u32, None);
        ui::set_process_eco(false);
        if config::current().b(Id::LiberarMemoria) {
            ui::trim_working_set();
        }

        let code = window::message_loop();
        ui::shutdown();
        code
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Liberar la memoria del propio proceso tiene que funcionar siempre (es
    /// el caso en el que no hace falta ningún permiso extra), y la lectura de
    /// memoria física en uso tiene que dar algo creíble.
    #[test]
    fn liberar_memoria_del_proceso_actual() {
        let antes = physical_in_use();
        assert!(antes > 0, "no se pudo leer la memoria en uso");
        let pid = unsafe {
            windows_sys::Win32::System::Threading::GetCurrentProcessId()
        };
        assert!(trim_process(pid), "EmptyWorkingSet falló sobre el propio proceso");
        // El proceso inactivo y System no se tocan nunca.
        assert!(!trim_process(0));
        assert!(!trim_process(4));
    }
}
