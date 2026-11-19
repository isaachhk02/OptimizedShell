// Muestreo de rendimiento del sistema para la pestaña «Rendimiento» del
// administrador de tareas: CPU (total y por núcleo), memoria, GPUs y sensores
// de temperatura.
//
// Criterios:
// - El muestreador se abre al entrar en la pestaña y se cierra al salir, para
//   no pagar el coste de PDH cuando nadie lo mira.
// - Nada es obligatorio: si PDH o DXGI no están disponibles (máquinas sin
//   contadores de GPU, sin sensores ACPI, sesiones remotas...), el resto sigue
//   funcionando y la interfaz lo dice en lugar de enseñar un cero falso.
// - FFI propia y mínima (ntdll, pdh, dxgi) en vez de COM generado: menos código
//   y sin dependencias nuevas.
#![allow(dead_code)]

use std::ffi::c_void;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::SystemInformation::{
    GetSystemInfo, GlobalMemoryStatusEx, MEMORYSTATUSEX, SYSTEM_INFO,
};

use crate::ui::wide;

// ---------------------------------------------------------------- ntdll
#[link(name = "ntdll")]
extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
}

/// SystemProcessorPerformanceInformation.
const SPPI_CLASS: u32 = 8;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Sppi {
    idle_time: i64,
    kernel_time: i64, // incluye el tiempo ocioso
    user_time: i64,
    dpc_time: i64,
    interrupt_time: i64,
    interrupt_count: u32,
}

// ---------------------------------------------------------------- PDH
type PdhHandle = HANDLE;
const PDH_FMT_DOUBLE: u32 = 0x0000_0200;

#[repr(C)]
struct PdhCounterValue {
    c_status: u32,
    _pad: u32,
    double_value: f64,
}

#[repr(C)]
struct PdhItemW {
    name: *mut u16,
    value: PdhCounterValue,
}

#[link(name = "pdh")]
extern "system" {
    fn PdhOpenQueryW(data_source: *const u16, user_data: usize, query: *mut PdhHandle) -> i32;
    fn PdhAddEnglishCounterW(query: PdhHandle, path: *const u16, user_data: usize, counter: *mut PdhHandle) -> i32;
    fn PdhCollectQueryData(query: PdhHandle) -> i32;
    fn PdhGetFormattedCounterArrayW(
        counter: PdhHandle,
        format: u32,
        buffer_size: *mut u32,
        item_count: *mut u32,
        items: *mut PdhItemW,
    ) -> i32;
    fn PdhCloseQuery(query: PdhHandle) -> i32;
}

/// Lee un contador con comodín como lista de (instancia, valor).
unsafe fn counter_array(counter: PdhHandle) -> Vec<(String, f64)> {
    if counter.is_null() {
        return Vec::new();
    }
    let mut size: u32 = 0;
    let mut count: u32 = 0;
    // Primera llamada: solo para saber el tamaño.
    PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, std::ptr::null_mut());
    if size == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u8; size as usize + 64];
    let mut size2 = buf.len() as u32;
    if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size2, &mut count, buf.as_mut_ptr() as *mut PdhItemW) != 0 {
        return Vec::new();
    }
    let items = std::slice::from_raw_parts(buf.as_ptr() as *const PdhItemW, count as usize);
    let mut out = Vec::with_capacity(count as usize);
    for it in items {
        if it.name.is_null() || it.value.c_status != 0 {
            continue;
        }
        let mut len = 0usize;
        while *it.name.add(len) != 0 && len < 512 {
            len += 1;
        }
        let name = String::from_utf16_lossy(std::slice::from_raw_parts(it.name, len));
        out.push((name, it.value.double_value));
    }
    out
}

// ---------------------------------------------------------------- DXGI
// Se llama al vtable a mano: solo hacen falta EnumAdapters1 y GetDesc1, y así
// no entra ninguna dependencia de COM generado.
#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

// IID_IDXGIFactory1 {770aae78-f26f-4dba-a829-253c83d1b387}
const IID_FACTORY1: Guid = Guid {
    data1: 0x770a_ae78,
    data2: 0xf26f,
    data3: 0x4dba,
    data4: [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
};

#[repr(C)]
#[derive(Clone, Copy)]
struct Luid {
    low: u32,
    high: i32,
}

#[repr(C)]
struct DxgiAdapterDesc1 {
    description: [u16; 128],
    vendor_id: u32,
    device_id: u32,
    sub_sys_id: u32,
    revision: u32,
    dedicated_video_memory: usize,
    dedicated_system_memory: usize,
    shared_system_memory: usize,
    adapter_luid: Luid,
    flags: u32,
}

#[link(name = "dxgi")]
extern "system" {
    fn CreateDXGIFactory1(riid: *const Guid, factory: *mut *mut c_void) -> i32;
}

type ComFn = unsafe extern "system" fn();

/// Índices en el vtable (IUnknown + IDXGIObject + ...).
const V_RELEASE: usize = 2;
const V_ENUM_ADAPTERS1: usize = 12; // IUnknown(3) + IDXGIObject(4) + IDXGIFactory(5)
const V_GET_DESC1: usize = 10; // IUnknown(3) + IDXGIObject(4) + IDXGIAdapter(3)

unsafe fn vtable(obj: *mut c_void) -> *const *const ComFn {
    *(obj as *const *const *const ComFn)
}

unsafe fn com_release(obj: *mut c_void) {
    if obj.is_null() {
        return;
    }
    let f: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(*vtable(obj).add(V_RELEASE));
    f(obj);
}

/// Adaptador gráfico descrito por DXGI.
pub struct Adapter {
    /// Clave igual a la que usan los contadores PDH: `luid_0xHIGH_0xLOW`.
    pub luid_key: String,
    pub name: String,
    pub vram: u64,
}

fn enum_adapters() -> Vec<Adapter> {
    let mut out = Vec::new();
    unsafe {
        let mut factory: *mut c_void = std::ptr::null_mut();
        if CreateDXGIFactory1(&IID_FACTORY1, &mut factory) != 0 || factory.is_null() {
            return out;
        }
        let enum_adapters1: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32 =
            std::mem::transmute(*vtable(factory).add(V_ENUM_ADAPTERS1));
        let mut i = 0u32;
        loop {
            let mut adapter: *mut c_void = std::ptr::null_mut();
            if enum_adapters1(factory, i, &mut adapter) != 0 || adapter.is_null() {
                break;
            }
            let get_desc1: unsafe extern "system" fn(*mut c_void, *mut DxgiAdapterDesc1) -> i32 =
                std::mem::transmute(*vtable(adapter).add(V_GET_DESC1));
            let mut d: DxgiAdapterDesc1 = std::mem::zeroed();
            if get_desc1(adapter, &mut d) == 0 && d.flags & 2 == 0 {
                // flags & 2 = DXGI_ADAPTER_FLAG_SOFTWARE (render por software).
                let len = d.description.iter().position(|&c| c == 0).unwrap_or(128);
                out.push(Adapter {
                    luid_key: format!("luid_0x{:08X}_0x{:08X}", d.adapter_luid.high, d.adapter_luid.low),
                    name: String::from_utf16_lossy(&d.description[..len]),
                    vram: d.dedicated_video_memory as u64,
                });
            }
            com_release(adapter);
            i += 1;
            if i > 16 {
                break;
            }
        }
        com_release(factory);
    }
    out
}

// ------------------------------------------------------------ información fija
#[derive(Clone, Default)]
pub struct CpuInfo {
    pub name: String,
    pub mhz: u32,
    pub logical: u32,
    pub physical: u32,
}

fn reg_cpu_info() -> (String, u32) {
    use windows_sys::Win32::System::Registry::*;
    const SUB: &str = "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0";
    let mut name = String::new();
    let mut mhz = 0u32;
    unsafe {
        let mut k: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, wide(SUB).as_ptr(), 0, KEY_QUERY_VALUE, &mut k) == 0 {
            let mut buf = [0u16; 256];
            let mut len = (buf.len() * 2) as u32;
            let mut kind = 0u32;
            if RegQueryValueExW(k, wide("ProcessorNameString").as_ptr(), std::ptr::null_mut(), &mut kind,
                                buf.as_mut_ptr() as *mut u8, &mut len) == 0 {
                let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                name = String::from_utf16_lossy(&buf[..n]).trim().to_string();
            }
            let mut v = 0u32;
            let mut len = 4u32;
            if RegQueryValueExW(k, wide("~MHz").as_ptr(), std::ptr::null_mut(), &mut kind,
                                &mut v as *mut u32 as *mut u8, &mut len) == 0 {
                mhz = v;
            }
            RegCloseKey(k);
        }
    }
    (name, mhz)
}

/// Núcleos físicos a partir de las relaciones de procesador. Si la API falla,
/// se estima la mitad de los lógicos (hyper-threading).
fn physical_cores(logical: u32) -> u32 {
    use windows_sys::Win32::System::SystemInformation::{
        GetLogicalProcessorInformation, SYSTEM_LOGICAL_PROCESSOR_INFORMATION,
    };
    unsafe {
        let mut len: u32 = 0;
        GetLogicalProcessorInformation(std::ptr::null_mut(), &mut len);
        if len == 0 {
            return (logical / 2).max(1);
        }
        let n = len as usize / std::mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>();
        let mut buf: Vec<SYSTEM_LOGICAL_PROCESSOR_INFORMATION> = vec![std::mem::zeroed(); n.max(1)];
        let mut len2 = (buf.len() * std::mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>()) as u32;
        if GetLogicalProcessorInformation(buf.as_mut_ptr(), &mut len2) == 0 {
            return (logical / 2).max(1);
        }
        let count = buf.iter().filter(|e| e.Relationship == 0 /*RelationProcessorCore*/).count() as u32;
        if count == 0 { (logical / 2).max(1) } else { count }
    }
}

pub fn cpu_info() -> CpuInfo {
    let (name, mhz) = reg_cpu_info();
    let logical = unsafe {
        let mut si: SYSTEM_INFO = std::mem::zeroed();
        GetSystemInfo(&mut si);
        si.dwNumberOfProcessors.max(1)
    };
    CpuInfo { name, mhz, logical, physical: physical_cores(logical) }
}

// ------------------------------------------------------------ instantánea
#[derive(Clone, Default)]
pub struct Mem {
    pub used: u64,
    pub total: u64,
    pub load: u32,
    pub commit_used: u64,
    pub commit_total: u64,
}

#[derive(Clone)]
pub struct Gpu {
    pub key: String,
    pub name: String,
    /// Uso en % (máximo entre tipos de motor, como el administrador de Windows).
    pub usage: f64,
    pub mem_used: u64,
    pub mem_total: u64,
}

#[derive(Clone)]
pub struct TempSensor {
    pub key: String,
    pub name: String,
    pub celsius: f64,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub cpu_total: f64,
    pub cpu_cores: Vec<f64>,
    pub mem: Mem,
    pub gpus: Vec<Gpu>,
    pub temps: Vec<TempSensor>,
    /// Hay contadores de GPU disponibles (Windows 10 1709 o posterior).
    pub gpu_counters: bool,
    /// Hay zonas térmicas ACPI expuestas al sistema.
    pub temp_counters: bool,
}

struct Pdh {
    query: PdhHandle,
    gpu_engine: PdhHandle,
    gpu_mem: PdhHandle,
    thermal: PdhHandle,
    thermal_hp: PdhHandle,
}

impl Pdh {
    fn open() -> Option<Pdh> {
        unsafe {
            let mut query: PdhHandle = std::ptr::null_mut();
            if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 || query.is_null() {
                return None;
            }
            let add = |path: &str| -> PdhHandle {
                let mut c: PdhHandle = std::ptr::null_mut();
                if PdhAddEnglishCounterW(query, wide(path).as_ptr(), 0, &mut c) != 0 {
                    return std::ptr::null_mut();
                }
                c
            };
            let p = Pdh {
                query,
                gpu_engine: add("\\GPU Engine(*)\\Utilization Percentage"),
                gpu_mem: add("\\GPU Adapter Memory(*)\\Dedicated Usage"),
                thermal: add("\\Thermal Zone Information(*)\\Temperature"),
                thermal_hp: add("\\Thermal Zone Information(*)\\High Precision Temperature"),
            };
            // Primera recogida: los contadores de tasa necesitan dos muestras.
            PdhCollectQueryData(query);
            Some(p)
        }
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        unsafe {
            if !self.query.is_null() {
                PdhCloseQuery(self.query);
            }
        }
    }
}

/// Muestreador con estado (deltas de CPU y consulta PDH abierta).
pub struct Sampler {
    prev: Vec<Sppi>,
    pdh: Option<Pdh>,
    adapters: Vec<Adapter>,
    pub info: CpuInfo,
    samples: u32,
}

impl Sampler {
    pub fn new() -> Sampler {
        Sampler {
            prev: Vec::new(),
            pdh: Pdh::open(),
            adapters: enum_adapters(),
            info: cpu_info(),
            samples: 0,
        }
    }

    /// Toma una instantánea. La primera devuelve CPU a 0 (hace falta un delta).
    pub fn sample(&mut self) -> Snapshot {
        let mut snap = Snapshot::default();
        self.samples += 1;
        self.sample_cpu(&mut snap);
        self.sample_mem(&mut snap);
        self.sample_pdh(&mut snap);
        snap
    }

    fn sample_cpu(&mut self, snap: &mut Snapshot) {
        let n = self.info.logical as usize;
        let mut cur: Vec<Sppi> = vec![Sppi::default(); n];
        let len = (n * std::mem::size_of::<Sppi>()) as u32;
        let mut ret = 0u32;
        let status = unsafe {
            NtQuerySystemInformation(SPPI_CLASS, cur.as_mut_ptr() as *mut c_void, len, &mut ret)
        };
        if status < 0 {
            return;
        }
        if self.prev.len() == n {
            let mut total = 0.0;
            for i in 0..n {
                let didle = (cur[i].idle_time - self.prev[i].idle_time).max(0) as f64;
                let dkernel = (cur[i].kernel_time - self.prev[i].kernel_time).max(0) as f64;
                let duser = (cur[i].user_time - self.prev[i].user_time).max(0) as f64;
                let dtotal = dkernel + duser; // kernel_time ya incluye el ocioso
                let usage = if dtotal > 0.0 { (1.0 - didle / dtotal) * 100.0 } else { 0.0 };
                let usage = usage.clamp(0.0, 100.0);
                snap.cpu_cores.push(usage);
                total += usage;
            }
            snap.cpu_total = if n > 0 { total / n as f64 } else { 0.0 };
        } else {
            snap.cpu_cores = vec![0.0; n];
        }
        self.prev = cur;
    }

    fn sample_mem(&self, snap: &mut Snapshot) {
        unsafe {
            let mut m: MEMORYSTATUSEX = std::mem::zeroed();
            m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
            if GlobalMemoryStatusEx(&mut m) != 0 {
                snap.mem = Mem {
                    used: m.ullTotalPhys - m.ullAvailPhys,
                    total: m.ullTotalPhys,
                    load: m.dwMemoryLoad,
                    commit_used: m.ullTotalPageFile.saturating_sub(m.ullAvailPageFile),
                    commit_total: m.ullTotalPageFile,
                };
            }
        }
    }

    fn sample_pdh(&mut self, snap: &mut Snapshot) {
        let Some(pdh) = self.pdh.as_ref() else { return };
        unsafe {
            if PdhCollectQueryData(pdh.query) != 0 {
                return;
            }
            // --- GPU: uso por LUID, tomando el máximo entre tipos de motor.
            let engines = counter_array(pdh.gpu_engine);
            snap.gpu_counters = !pdh.gpu_engine.is_null();
            let mut by_engine: Vec<(String, String, f64)> = Vec::new(); // luid, engtype, suma
            for (inst, v) in &engines {
                let Some(luid) = luid_of(inst) else { continue };
                let engtype = inst.rsplit("engtype_").next().unwrap_or("").to_string();
                match by_engine.iter_mut().find(|(l, e, _)| *l == luid && *e == engtype) {
                    Some(entry) => entry.2 += v,
                    None => by_engine.push((luid, engtype, *v)),
                }
            }
            let mut usage: Vec<(String, f64)> = Vec::new();
            for (luid, _, v) in &by_engine {
                match usage.iter_mut().find(|(l, _)| l == luid) {
                    Some(e) => e.1 = e.1.max(*v),
                    None => usage.push((luid.clone(), *v)),
                }
            }
            // --- memoria dedicada en uso por LUID
            let mut mem_used: Vec<(String, f64)> = Vec::new();
            for (inst, v) in counter_array(pdh.gpu_mem) {
                let Some(luid) = luid_of(&inst) else { continue };
                match mem_used.iter_mut().find(|(l, _)| *l == luid) {
                    Some(e) => e.1 += v,
                    None => mem_used.push((luid, v)),
                }
            }
            // --- se juntan con los adaptadores de DXGI (nombre y VRAM total)
            for a in &self.adapters {
                let u = usage.iter().find(|(l, _)| *l == a.luid_key).map(|(_, v)| *v).unwrap_or(0.0);
                let m = mem_used.iter().find(|(l, _)| *l == a.luid_key).map(|(_, v)| *v).unwrap_or(0.0);
                snap.gpus.push(Gpu {
                    key: a.luid_key.clone(),
                    name: a.name.clone(),
                    usage: u.clamp(0.0, 100.0),
                    mem_used: m as u64,
                    mem_total: a.vram,
                });
            }
            // GPUs con contadores pero sin adaptador DXGI reconocido (raro).
            // Solo se muestran si de verdad están haciendo algo: Windows expone
            // LUID de motores virtuales (render remoto, captura) que si no
            // ensuciarían la lista con entradas a cero.
            for (luid, v) in &usage {
                let m = mem_used.iter().find(|(l, _)| l == luid).map(|(_, x)| *x).unwrap_or(0.0);
                if *v < 0.5 && m <= 0.0 {
                    continue;
                }
                if !snap.gpus.iter().any(|g| g.key == *luid) {
                    snap.gpus.push(Gpu {
                        key: luid.clone(),
                        name: format!("GPU ({})", luid),
                        usage: v.clamp(0.0, 100.0),
                        mem_used: m as u64,
                        mem_total: 0,
                    });
                }
            }

            // --- temperatura: Kelvin (o deci-Kelvin en el contador de precisión)
            let mut temps: Vec<(String, f64)> = Vec::new();
            for (inst, v) in counter_array(pdh.thermal_hp) {
                if let Some(c) = to_celsius(v) {
                    temps.push((inst, c));
                }
            }
            if temps.is_empty() {
                for (inst, v) in counter_array(pdh.thermal) {
                    if let Some(c) = to_celsius(v) {
                        temps.push((inst, c));
                    }
                }
            }
            snap.temp_counters = !temps.is_empty();
            for (inst, c) in temps {
                snap.temps.push(TempSensor {
                    key: format!("temp:{}", inst),
                    name: pretty_zone(&inst),
                    celsius: c,
                });
            }
        }
    }
}

/// `pid_1234_luid_0x00000000_0x0000C27F_phys_0_eng_0_engtype_3D` -> `luid_0x…_0x…`
fn luid_of(instance: &str) -> Option<String> {
    let i = instance.find("luid_")?;
    let rest = &instance[i..];
    let mut parts = rest.split('_'); // luid, 0xHIGH, 0xLOW, ...
    let a = parts.next()?;
    let b = parts.next()?;
    let c = parts.next()?;
    Some(format!("{}_{}_{}", a, b, c))
}

/// Kelvin o deci-Kelvin -> °C, descartando valores imposibles.
fn to_celsius(v: f64) -> Option<f64> {
    let k = if v > 1000.0 { v / 10.0 } else { v };
    let c = k - 273.15;
    if (-20.0..=150.0).contains(&c) && k > 0.0 {
        Some(c)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El muestreo tiene que funcionar en cualquier máquina: sin GPU con
    /// contadores, sin sensores térmicos o sin PDH, CPU y memoria siguen.
    #[test]
    fn muestreo_coherente() {
        let mut s = Sampler::new();
        let _ = s.sample();
        std::thread::sleep(std::time::Duration::from_millis(250));
        let snap = s.sample();
        assert!(snap.mem.total > 0, "memoria total desconocida");
        assert!(snap.mem.used <= snap.mem.total);
        assert!((0.0..=100.0).contains(&snap.cpu_total), "CPU fuera de rango: {}", snap.cpu_total);
        assert_eq!(snap.cpu_cores.len(), s.info.logical as usize);
        for g in &snap.gpus {
            assert!((0.0..=100.0).contains(&g.usage));
        }
        for t in &snap.temps {
            assert!((-20.0..=150.0).contains(&t.celsius));
        }
        println!(
            "CPU {} ({} lógicos / {} físicos, {} MHz): {:.1} %\nRAM: {} / {} MB\nGPUs: {}\nTemperaturas: {}",
            s.info.name, s.info.logical, s.info.physical, s.info.mhz, snap.cpu_total,
            snap.mem.used / 1024 / 1024, snap.mem.total / 1024 / 1024,
            snap.gpus.iter().map(|g| format!("{} {:.0}% {} MB/{} MB", g.name, g.usage, g.mem_used / 1048576, g.mem_total / 1048576)).collect::<Vec<_>>().join(" | "),
            snap.temps.iter().map(|t| format!("{} {:.1} °C", t.name, t.celsius)).collect::<Vec<_>>().join(" | "),
        );
    }

    #[test]
    fn luid_y_temperatura() {
        assert_eq!(
            luid_of("pid_1234_luid_0x00000000_0x0000C27F_phys_0_eng_0_engtype_3D").unwrap(),
            "luid_0x00000000_0x0000C27F"
        );
        assert!(luid_of("otra cosa").is_none());
        assert!((to_celsius(310.15).unwrap() - 37.0).abs() < 0.01);
        assert!((to_celsius(3101.5).unwrap() - 37.0).abs() < 0.01);
        assert!(to_celsius(0.0).is_none());
    }
}

/// `\_TZ.TZ01` o `ACPI\ThermalZone\TZ01_0` -> `TZ01`
fn pretty_zone(inst: &str) -> String {
    let last = inst.rsplit(['\\', '.']).next().unwrap_or(inst);
    let last = last.trim_end_matches("_0");
    if last.is_empty() { inst.to_string() } else { last.to_string() }
}
