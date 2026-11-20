// Estado del sistema que enseña el área de notificación: volumen, batería,
// red (Wi-Fi o Ethernet) y Bluetooth.
//
// Criterios, los mismos que en `perf`:
// - FFI propia y mínima. El volumen necesita COM (Core Audio), así que se
//   llama al vtable a mano en vez de arrastrar una capa de COM generado.
// - Las bibliotecas que pueden no estar (wlanapi en equipos sin Wi-Fi,
//   bthprops en equipos sin Bluetooth) se cargan con LoadLibrary: si faltan,
//   el icono correspondiente simplemente no aparece.
// - El muestreo es perezoso: la barra lo pide cada pocos segundos, no en cada
//   repintado, y el volumen se consulta solo cuando hace falta.
#![allow(dead_code)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::ui::wide;

// ============================================================ utilidades COM
type ComFn = unsafe extern "system" fn();

unsafe fn vtable(obj: *mut c_void) -> *const *const ComFn {
    *(obj as *const *const *const ComFn)
}

unsafe fn com_release(obj: *mut c_void) {
    if obj.is_null() {
        return;
    }
    let f: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(*vtable(obj).add(2));
    f(obj);
}

static COM_READY: AtomicBool = AtomicBool::new(false);

/// Inicializa COM en el hilo actual (una sola vez por proceso de UI).
fn ensure_com() {
    if COM_READY.swap(true, Ordering::Relaxed) {
        return;
    }
    unsafe {
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
    }
}

// ============================================================ volumen
// CLSID_MMDeviceEnumerator {BCDE0395-E52F-467C-8E3D-C4579291692E}
const CLSID_MM_DEVICE_ENUMERATOR: GUID = GUID::from_u128(0xBCDE0395_E52F_467C_8E3D_C4579291692E);
// IID_IMMDeviceEnumerator {A95664D2-9614-4F35-A746-DE8DB63617E6}
const IID_IMM_DEVICE_ENUMERATOR: GUID = GUID::from_u128(0xA95664D2_9614_4F35_A746_DE8DB63617E6);
// IID_IAudioEndpointVolume {5CDF2C82-841E-4546-9722-0CF74078229A}
const IID_IAUDIO_ENDPOINT_VOLUME: GUID = GUID::from_u128(0x5CDF2C82_841E_4546_9722_0CF74078229A);

// Índices en los vtables (IUnknown ocupa siempre los tres primeros).
const V_GET_DEFAULT_ENDPOINT: usize = 4; // IMMDeviceEnumerator
const V_ACTIVATE: usize = 3; // IMMDevice
const V_SET_MASTER_SCALAR: usize = 7; // IAudioEndpointVolume
const V_GET_MASTER_SCALAR: usize = 9;
const V_SET_MUTE: usize = 14;
const V_GET_MUTE: usize = 15;

/// Puntero al control de volumen del dispositivo de salida predeterminado.
/// Se abre y se cierra en cada operación: el dispositivo puede cambiar
/// (auriculares, HDMI) y mantenerlo abierto obligaría a escuchar
/// notificaciones de Core Audio para enterarse.
struct Endpoint(*mut c_void);

impl Endpoint {
    fn open() -> Option<Endpoint> {
        ensure_com();
        unsafe {
            let mut enumer: *mut c_void = std::ptr::null_mut();
            if CoCreateInstance(&CLSID_MM_DEVICE_ENUMERATOR, std::ptr::null_mut(), CLSCTX_INPROC_SERVER,
                                &IID_IMM_DEVICE_ENUMERATOR, &mut enumer) != 0 || enumer.is_null() {
                return None;
            }
            let get_default: unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void) -> i32 =
                std::mem::transmute(*vtable(enumer).add(V_GET_DEFAULT_ENDPOINT));
            let mut device: *mut c_void = std::ptr::null_mut();
            // eRender = 0, eConsole = 0.
            let hr = get_default(enumer, 0, 0, &mut device);
            com_release(enumer);
            if hr != 0 || device.is_null() {
                return None;
            }
            let activate: unsafe extern "system" fn(*mut c_void, *const GUID, u32, *mut c_void, *mut *mut c_void) -> i32 =
                std::mem::transmute(*vtable(device).add(V_ACTIVATE));
            let mut vol: *mut c_void = std::ptr::null_mut();
            let hr = activate(device, &IID_IAUDIO_ENDPOINT_VOLUME, CLSCTX_INPROC_SERVER,
                              std::ptr::null_mut(), &mut vol);
            com_release(device);
            if hr != 0 || vol.is_null() {
                return None;
            }
            Some(Endpoint(vol))
        }
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        unsafe { com_release(self.0) };
    }
}

/// Volumen (0-100) y silencio del dispositivo de salida predeterminado.
/// `None` si no hay dispositivo de audio.
pub fn volume() -> Option<(i32, bool)> {
    let ep = Endpoint::open()?;
    unsafe {
        let get_scalar: unsafe extern "system" fn(*mut c_void, *mut f32) -> i32 =
            std::mem::transmute(*vtable(ep.0).add(V_GET_MASTER_SCALAR));
        let get_mute: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32 =
            std::mem::transmute(*vtable(ep.0).add(V_GET_MUTE));
        let mut level = 0.0f32;
        let mut muted = 0i32;
        if get_scalar(ep.0, &mut level) != 0 {
            return None;
        }
        get_mute(ep.0, &mut muted);
        Some(((level * 100.0 + 0.5) as i32, muted != 0))
    }
}

/// Fija el volumen (0-100) del dispositivo predeterminado.
pub fn set_volume(pct: i32) {
    let Some(ep) = Endpoint::open() else { return };
    unsafe {
        let set_scalar: unsafe extern "system" fn(*mut c_void, f32, *const GUID) -> i32 =
            std::mem::transmute(*vtable(ep.0).add(V_SET_MASTER_SCALAR));
        set_scalar(ep.0, pct.clamp(0, 100) as f32 / 100.0, std::ptr::null());
    }
}

/// Silencia o restaura el sonido. Devuelve el estado resultante.
pub fn toggle_mute() -> bool {
    let Some(ep) = Endpoint::open() else { return false };
    unsafe {
        let get_mute: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32 =
            std::mem::transmute(*vtable(ep.0).add(V_GET_MUTE));
        let set_mute: unsafe extern "system" fn(*mut c_void, i32, *const GUID) -> i32 =
            std::mem::transmute(*vtable(ep.0).add(V_SET_MUTE));
        let mut muted = 0i32;
        get_mute(ep.0, &mut muted);
        let next = (muted == 0) as i32;
        set_mute(ep.0, next, std::ptr::null());
        next != 0
    }
}

// ============================================================ batería
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SystemPowerStatus {
    ac_line_status: u8,
    battery_flag: u8,
    battery_life_percent: u8,
    system_status_flag: u8,
    battery_life_time: u32,
    battery_full_life_time: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
}

#[derive(Clone, Copy, Default, PartialEq)]
pub struct Battery {
    /// Hay batería instalada.
    pub present: bool,
    /// Carga restante en % (255 si el sistema no lo sabe -> se guarda -1).
    pub percent: i32,
    /// Conectado a la corriente.
    pub charging: bool,
    /// Ahorro de energía activo.
    pub saver: bool,
    /// Segundos restantes estimados, si el sistema los da.
    pub seconds_left: i32,
}

pub fn battery() -> Battery {
    let mut st = SystemPowerStatus::default();
    if unsafe { GetSystemPowerStatus(&mut st) } == 0 {
        return Battery::default();
    }
    // battery_flag 128 = no hay batería; 255 = desconocido.
    let present = st.battery_flag != 128 && st.battery_flag != 255;
    Battery {
        present,
        percent: if st.battery_life_percent == 255 { -1 } else { st.battery_life_percent as i32 },
        charging: st.ac_line_status == 1,
        saver: st.system_status_flag == 1,
        seconds_left: if st.battery_life_time == u32::MAX { -1 } else { st.battery_life_time as i32 },
    }
}

// ============================================================ red
#[derive(Clone, PartialEq)]
pub enum Net {
    /// Sin conexión operativa.
    Off,
    /// Cable conectado, con su velocidad en Mbit/s (0 si no se conoce).
    Ethernet { mbps: u32 },
    /// Wi-Fi asociado, con el SSID y la calidad de la señal (0-100).
    Wifi { ssid: String, quality: u32 },
}

impl Default for Net {
    fn default() -> Self {
        Net::Off
    }
}

// ---------------------------------------------------------------- iphlpapi
const MAX_INTERFACE_NAME_LEN: usize = 256;
const MAXLEN_PHYSADDR: usize = 8;
const MAXLEN_IFDESCR: usize = 256;

#[repr(C)]
#[derive(Clone, Copy)]
struct MibIfRow {
    name: [u16; MAX_INTERFACE_NAME_LEN],
    index: u32,
    kind: u32,
    mtu: u32,
    speed: u32,
    phys_addr_len: u32,
    phys_addr: [u8; MAXLEN_PHYSADDR],
    admin_status: u32,
    oper_status: u32,
    last_change: u32,
    in_octets: u32,
    in_ucast_pkts: u32,
    in_nucast_pkts: u32,
    in_discards: u32,
    in_errors: u32,
    in_unknown_protos: u32,
    out_octets: u32,
    out_ucast_pkts: u32,
    out_nucast_pkts: u32,
    out_discards: u32,
    out_errors: u32,
    out_qlen: u32,
    descr_len: u32,
    descr: [u8; MAXLEN_IFDESCR],
}

#[link(name = "iphlpapi")]
extern "system" {
    fn GetIfTable(table: *mut c_void, size: *mut u32, order: i32) -> u32;
}

/// Tipos de interfaz de la MIB que nos interesan.
const IF_TYPE_ETHERNET: u32 = 6;
const IF_TYPE_WIFI: u32 = 71;
const IF_TYPE_LOOPBACK: u32 = 24;
/// INTERNAL_IF_OPER_STATUS: 4 = conectada, 5 = operativa.
const OPER_CONNECTED: u32 = 4;

/// Recorre la tabla de interfaces y devuelve (hay_ethernet, mbps, hay_wifi_activa).
fn if_table() -> (bool, u32, bool) {
    unsafe {
        let mut size: u32 = 0;
        GetIfTable(std::ptr::null_mut(), &mut size, 0);
        if size == 0 {
            return (false, 0, false);
        }
        let mut buf = vec![0u8; size as usize + 1024];
        let mut size2 = buf.len() as u32;
        if GetIfTable(buf.as_mut_ptr() as *mut c_void, &mut size2, 0) != 0 {
            return (false, 0, false);
        }
        let count = *(buf.as_ptr() as *const u32) as usize;
        let rows = buf.as_ptr().add(std::mem::size_of::<u32>()) as *const MibIfRow;
        let (mut eth, mut mbps, mut wifi) = (false, 0u32, false);
        for i in 0..count.min(256) {
            let r = &*rows.add(i);
            if r.kind == IF_TYPE_LOOPBACK || r.oper_status < OPER_CONNECTED {
                continue;
            }
            // Las interfaces virtuales (VPN, hipervisores) también son "ethernet";
            // aquí solo interesa saber si hay alguna vía de salida por cable.
            if r.kind == IF_TYPE_ETHERNET {
                eth = true;
                mbps = mbps.max(r.speed / 1_000_000);
            } else if r.kind == IF_TYPE_WIFI {
                wifi = true;
            }
        }
        (eth, mbps, wifi)
    }
}

// ---------------------------------------------------------------- wlanapi
#[repr(C)]
struct Dot11Ssid {
    length: u32,
    ssid: [u8; 32],
}

#[repr(C)]
struct WlanAssociationAttributes {
    ssid: Dot11Ssid,
    bss_type: u32,
    bssid: [u8; 6],
    phy_type: u32,
    phy_index: u32,
    signal_quality: u32,
    rx_rate: u32,
    tx_rate: u32,
}

#[repr(C)]
struct WlanConnectionAttributes {
    state: u32,
    mode: u32,
    profile: [u16; 256],
    association: WlanAssociationAttributes,
    // Las atributos de seguridad vienen después; no se leen.
}

#[repr(C)]
struct WlanInterfaceInfo {
    guid: GUID,
    description: [u16; 256],
    state: u32,
}

#[repr(C)]
struct WlanInterfaceInfoList {
    count: u32,
    index: u32,
    // info[1] a continuación
}

/// Punteros a wlanapi.dll, cargada bajo demanda (puede no existir).
struct WlanApi {
    module: HMODULE,
    open: unsafe extern "system" fn(u32, *mut c_void, *mut u32, *mut *mut c_void) -> u32,
    close: unsafe extern "system" fn(*mut c_void, *mut c_void) -> u32,
    enum_ifaces: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut WlanInterfaceInfoList) -> u32,
    query: unsafe extern "system" fn(*mut c_void, *const GUID, u32, *mut c_void, *mut u32, *mut *mut c_void, *mut u32) -> u32,
    free: unsafe extern "system" fn(*mut c_void),
}

impl WlanApi {
    fn load() -> Option<WlanApi> {
        unsafe {
            let module = LoadLibraryW(wide("wlanapi.dll").as_ptr());
            if module.is_null() {
                return None;
            }
            let get = |name: &str| {
                let c = format!("{}\0", name);
                GetProcAddress(module, c.as_ptr())
            };
            let api = WlanApi {
                module,
                open: std::mem::transmute(get("WlanOpenHandle")?),
                close: std::mem::transmute(get("WlanCloseHandle")?),
                enum_ifaces: std::mem::transmute(get("WlanEnumInterfaces")?),
                query: std::mem::transmute(get("WlanQueryInterface")?),
                free: std::mem::transmute(get("WlanFreeMemory")?),
            };
            Some(api)
        }
    }
}

impl Drop for WlanApi {
    fn drop(&mut self) {
        unsafe { FreeLibrary(self.module) };
    }
}

/// Conexión Wi-Fi activa: SSID y calidad (0-100).
fn wifi_connection() -> Option<(String, u32)> {
    let api = WlanApi::load()?;
    unsafe {
        let mut negotiated = 0u32;
        let mut handle: *mut c_void = std::ptr::null_mut();
        if (api.open)(2, std::ptr::null_mut(), &mut negotiated, &mut handle) != 0 || handle.is_null() {
            return None;
        }
        let mut list: *mut WlanInterfaceInfoList = std::ptr::null_mut();
        let mut out = None;
        if (api.enum_ifaces)(handle, std::ptr::null_mut(), &mut list) == 0 && !list.is_null() {
            let count = (*list).count as usize;
            let infos = (list as *const u8).add(std::mem::size_of::<WlanInterfaceInfoList>()) as *const WlanInterfaceInfo;
            for i in 0..count.min(8) {
                let info = &*infos.add(i);
                // 1 = wlan_interface_state_connected
                if info.state != 1 {
                    continue;
                }
                let mut size = 0u32;
                let mut data: *mut c_void = std::ptr::null_mut();
                // 7 = wlan_intf_opcode_current_connection
                if (api.query)(handle, &info.guid, 7, std::ptr::null_mut(), &mut size, &mut data, std::ptr::null_mut()) == 0
                    && !data.is_null()
                    && size as usize >= std::mem::size_of::<WlanConnectionAttributes>()
                {
                    let conn = &*(data as *const WlanConnectionAttributes);
                    let n = (conn.association.ssid.length as usize).min(32);
                    let ssid = String::from_utf8_lossy(&conn.association.ssid.ssid[..n]).to_string();
                    out = Some((ssid, conn.association.signal_quality.min(100)));
                }
                if !data.is_null() {
                    (api.free)(data);
                }
                if out.is_some() {
                    break;
                }
            }
        }
        if !list.is_null() {
            (api.free)(list as *mut c_void);
        }
        (api.close)(handle, std::ptr::null_mut());
        out
    }
}

/// Estado de red actual. El cable tiene prioridad sobre el Wi-Fi, igual que en
/// el enrutado real de Windows.
pub fn network() -> Net {
    let (eth, mbps, wifi_up) = if_table();
    if eth {
        return Net::Ethernet { mbps };
    }
    if wifi_up {
        if let Some((ssid, quality)) = wifi_connection() {
            return Net::Wifi { ssid, quality };
        }
    }
    Net::Off
}

// ============================================================ Bluetooth
#[repr(C)]
struct BluetoothFindRadioParams {
    size: u32,
}

/// `Some(true)` si hay radio Bluetooth disponible, `Some(false)` si el equipo
/// tiene la pila instalada pero ninguna radio activa, `None` si no hay
/// Bluetooth en el equipo.
pub fn bluetooth() -> Option<bool> {
    unsafe {
        let module = LoadLibraryW(wide("bthprops.cpl").as_ptr());
        if module.is_null() {
            return None;
        }
        let find = GetProcAddress(module, b"BluetoothFindFirstRadio\0".as_ptr());
        let close_find = GetProcAddress(module, b"BluetoothFindRadioClose\0".as_ptr());
        let result = match (find, close_find) {
            (Some(f), Some(c)) => {
                let find: unsafe extern "system" fn(*const BluetoothFindRadioParams, *mut *mut c_void) -> *mut c_void =
                    std::mem::transmute(f);
                let close_find: unsafe extern "system" fn(*mut c_void) -> i32 = std::mem::transmute(c);
                let params = BluetoothFindRadioParams { size: std::mem::size_of::<BluetoothFindRadioParams>() as u32 };
                let mut radio: *mut c_void = std::ptr::null_mut();
                let h = find(&params, &mut radio);
                if h.is_null() {
                    Some(false)
                } else {
                    if !radio.is_null() {
                        windows_sys::Win32::Foundation::CloseHandle(radio);
                    }
                    close_find(h);
                    Some(true)
                }
            }
            _ => None,
        };
        FreeLibrary(module);
        result
    }
}

// ============================================================ instantánea
/// Todo lo que la bandeja necesita, muestreado de una vez.
#[derive(Clone, Default)]
pub struct Snapshot {
    pub volume: i32,
    pub muted: bool,
    pub has_audio: bool,
    pub battery: Battery,
    pub net: Net,
    pub bluetooth: Option<bool>,
}

pub fn snapshot() -> Snapshot {
    let (volume, muted, has_audio) = match volume() {
        Some((v, m)) => (v, m, true),
        None => (0, false, false),
    };
    Snapshot {
        volume,
        muted,
        has_audio,
        battery: battery(),
        net: network(),
        bluetooth: bluetooth(),
    }
}
