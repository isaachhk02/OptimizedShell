// Encender y apagar las radios del equipo.
//
// Son dos caminos distintos, porque Windows no ofrece uno solo:
//
//   * Wi-Fi: wlanapi, con `WlanSetInterface` y el código de operación
//     `wlan_intf_opcode_radio_state`. Es API Win32 documentada y directa.
//   * Bluetooth: no hay API Win32 que apague la radio (las funciones de
//     bthprops solo tocan la visibilidad), así que se usa la API Radio de
//     WinRT (`Windows.Devices.Radios.Radio`) llamando al vtable a mano: se
//     piden las radios, se busca la de Bluetooth y se le cambia el estado.
//
// Para no implementar interfaces de WinRT (las de continuación de las
// operaciones asíncronas), se espera consultando `IAsyncInfo::Status` con un
// tope de tiempo: son operaciones de milisegundos.
//
// Todo esto puede fallar por permisos o porque el equipo no tenga la radio;
// cada función devuelve si pudo hacerlo para poder decirlo en la interfaz.
#![allow(dead_code)]

use std::ffi::c_void;

use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::ui::wide;

// ============================================================ Wi-Fi (wlanapi)
const DOT11_RADIO_STATE_ON: u32 = 1;
const DOT11_RADIO_STATE_OFF: u32 = 2;
/// wlan_intf_opcode_radio_state
const OPCODE_RADIO_STATE: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy)]
struct PhyRadioState {
    phy_index: u32,
    software: u32,
    hardware: u32,
}

#[repr(C)]
struct RadioStateList {
    count: u32,
    phys: [PhyRadioState; 64],
}

#[repr(C)]
struct InterfaceInfo {
    guid: GUID,
    description: [u16; 256],
    state: u32,
}

#[repr(C)]
struct InterfaceInfoList {
    count: u32,
    index: u32,
}

struct Wlan {
    module: HMODULE,
    open: unsafe extern "system" fn(u32, *mut c_void, *mut u32, *mut *mut c_void) -> u32,
    close: unsafe extern "system" fn(*mut c_void, *mut c_void) -> u32,
    enum_ifaces: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut InterfaceInfoList) -> u32,
    query: unsafe extern "system" fn(*mut c_void, *const GUID, u32, *mut c_void, *mut u32, *mut *mut c_void, *mut u32) -> u32,
    set: unsafe extern "system" fn(*mut c_void, *const GUID, u32, u32, *const c_void, *mut c_void) -> u32,
    free: unsafe extern "system" fn(*mut c_void),
}

impl Wlan {
    fn load() -> Option<Wlan> {
        unsafe {
            let module = LoadLibraryW(wide("wlanapi.dll").as_ptr());
            if module.is_null() {
                return None;
            }
            let get = |n: &str| {
                let c = format!("{}\0", n);
                GetProcAddress(module, c.as_ptr())
            };
            Some(Wlan {
                module,
                open: std::mem::transmute(get("WlanOpenHandle")?),
                close: std::mem::transmute(get("WlanCloseHandle")?),
                enum_ifaces: std::mem::transmute(get("WlanEnumInterfaces")?),
                query: std::mem::transmute(get("WlanQueryInterface")?),
                set: std::mem::transmute(get("WlanSetInterface")?),
                free: std::mem::transmute(get("WlanFreeMemory")?),
            })
        }
    }
}

impl Drop for Wlan {
    fn drop(&mut self) {
        unsafe { FreeLibrary(self.module) };
    }
}

/// Recorre las interfaces Wi-Fi llamando a `f` con el handle y el GUID.
fn for_each_wifi<F: FnMut(&Wlan, *mut c_void, &GUID) -> bool>(mut f: F) -> bool {
    let Some(api) = Wlan::load() else { return false };
    unsafe {
        let mut negotiated = 0u32;
        let mut handle: *mut c_void = std::ptr::null_mut();
        if (api.open)(2, std::ptr::null_mut(), &mut negotiated, &mut handle) != 0 || handle.is_null() {
            return false;
        }
        let mut list: *mut InterfaceInfoList = std::ptr::null_mut();
        let mut any = false;
        if (api.enum_ifaces)(handle, std::ptr::null_mut(), &mut list) == 0 && !list.is_null() {
            let count = (*list).count as usize;
            let infos = (list as *const u8).add(std::mem::size_of::<InterfaceInfoList>()) as *const InterfaceInfo;
            for i in 0..count.min(8) {
                let info = &*infos.add(i);
                if f(&api, handle, &info.guid) {
                    any = true;
                }
            }
        }
        if !list.is_null() {
            (api.free)(list as *mut c_void);
        }
        (api.close)(handle, std::ptr::null_mut());
        any
    }
}

/// ¿Está encendida la radio Wi-Fi? `None` si no hay adaptador Wi-Fi.
pub fn wifi_on() -> Option<bool> {
    let mut found: Option<bool> = None;
    for_each_wifi(|api, handle, guid| unsafe {
        let mut size = 0u32;
        let mut data: *mut c_void = std::ptr::null_mut();
        if (api.query)(handle, guid, OPCODE_RADIO_STATE, std::ptr::null_mut(), &mut size,
                       &mut data, std::ptr::null_mut()) == 0 && !data.is_null()
        {
            let list = &*(data as *const RadioStateList);
            let n = (list.count as usize).min(64);
            // Encendida si alguna de sus radios lo está (por software y hardware).
            let on = (0..n).any(|i| {
                list.phys[i].software != DOT11_RADIO_STATE_OFF
                    && list.phys[i].hardware != DOT11_RADIO_STATE_OFF
            });
            found = Some(found.unwrap_or(false) || on);
            (api.free)(data);
            return true;
        }
        false
    });
    found
}

/// Enciende o apaga la radio Wi-Fi de todos los adaptadores.
pub fn set_wifi(on: bool) -> bool {
    for_each_wifi(|api, handle, guid| unsafe {
        let st = PhyRadioState {
            phy_index: 0,
            software: if on { DOT11_RADIO_STATE_ON } else { DOT11_RADIO_STATE_OFF },
            hardware: 0,
        };
        (api.set)(handle, guid, OPCODE_RADIO_STATE,
                  std::mem::size_of::<PhyRadioState>() as u32,
                  &st as *const _ as *const c_void, std::ptr::null_mut()) == 0
    })
}

// ============================================================ WinRT (Bluetooth)
type ComFn = unsafe extern "system" fn();

unsafe fn vtable(obj: *mut c_void) -> *const *const ComFn {
    *(obj as *const *const *const ComFn)
}

unsafe fn release(obj: *mut c_void) {
    if obj.is_null() {
        return;
    }
    let f: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(*vtable(obj).add(2));
    f(obj);
}

/// IID_IRadioStatics {5FB6A12E-67CB-46AE-AAE9-65919F86EFF4}
const IID_RADIO_STATICS: GUID = GUID::from_u128(0x5FB6A12E_67CB_46AE_AAE9_65919F86EFF4);

/// Ranuras del vtable tras IInspectable (QI, AddRef, Release, GetIids,
/// GetRuntimeClassName, GetTrustLevel).
const V_RADIOS_GET_RADIOS_ASYNC: usize = 6; // IRadioStatics
const V_ASYNC_GET_RESULTS: usize = 8; // IAsyncOperation<T>
const V_VECTOR_GET_AT: usize = 6; // IVectorView<T>
const V_VECTOR_GET_SIZE: usize = 7;
const V_RADIO_SET_STATE_ASYNC: usize = 6; // IRadio
const V_RADIO_GET_STATE: usize = 9;
const V_RADIO_GET_KIND: usize = 11;
/// IAsyncInfo::Status (IInspectable + Id)
const V_ASYNCINFO_STATUS: usize = 7;

/// IID_IAsyncInfo {00000036-0000-0000-C000-000000000046}
const IID_ASYNC_INFO: GUID = GUID::from_u128(0x00000036_0000_0000_C000_000000000046);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Other = 0,
    WiFi = 1,
    MobileBroadband = 2,
    Bluetooth = 3,
    Fm = 4,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RadioState {
    Unknown = 0,
    On = 1,
    Off = 2,
    Disabled = 3,
}

impl RadioState {
    fn from_code(c: i32) -> RadioState {
        match c {
            1 => RadioState::On,
            2 => RadioState::Off,
            3 => RadioState::Disabled,
            _ => RadioState::Unknown,
        }
    }
}

/// Funciones de combase.dll, cargadas bajo demanda: así no hace falta enlazar
/// con WinRT para que el binario arranque en equipos donde no esté.
struct WinRt {
    module: HMODULE,
    ro_initialize: unsafe extern "system" fn(i32) -> i32,
    ro_get_activation_factory: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    create_string: unsafe extern "system" fn(*const u16, u32, *mut *mut c_void) -> i32,
    delete_string: unsafe extern "system" fn(*mut c_void) -> i32,
}

impl WinRt {
    fn load() -> Option<WinRt> {
        unsafe {
            let module = LoadLibraryW(wide("combase.dll").as_ptr());
            if module.is_null() {
                return None;
            }
            let get = |n: &str| {
                let c = format!("{}\0", n);
                GetProcAddress(module, c.as_ptr())
            };
            let rt = WinRt {
                module,
                ro_initialize: std::mem::transmute(get("RoInitialize")?),
                ro_get_activation_factory: std::mem::transmute(get("RoGetActivationFactory")?),
                create_string: std::mem::transmute(get("WindowsCreateString")?),
                delete_string: std::mem::transmute(get("WindowsDeleteString")?),
            };
            // RO_INIT_MULTITHREADED = 1. Si ya estaba inicializado devuelve
            // S_FALSE o RPC_E_CHANGED_MODE: ambos valen.
            (rt.ro_initialize)(1);
            Some(rt)
        }
    }

    /// Espera a que termine una operación asíncrona de WinRT.
    unsafe fn await_async(&self, op: *mut c_void, ms: u32) -> bool {
        if op.is_null() {
            return false;
        }
        let qi: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32 =
            std::mem::transmute(*vtable(op).add(0));
        let mut info: *mut c_void = std::ptr::null_mut();
        if qi(op, &IID_ASYNC_INFO, &mut info) != 0 || info.is_null() {
            return false;
        }
        let status: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32 =
            std::mem::transmute(*vtable(info).add(V_ASYNCINFO_STATUS));
        let mut ok = false;
        let step = 10u32;
        let mut waited = 0u32;
        while waited < ms {
            let mut s = 0i32;
            if status(info, &mut s) != 0 {
                break;
            }
            // 0 = Started, 1 = Completed, 2 = Canceled, 3 = Error.
            if s == 1 {
                ok = true;
                break;
            }
            if s >= 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(step as u64));
            waited += step;
        }
        release(info);
        ok
    }
}

impl Drop for WinRt {
    fn drop(&mut self) {
        unsafe { FreeLibrary(self.module) };
    }
}

/// Una radio del equipo según WinRT.
pub struct Radio {
    obj: *mut c_void,
    pub kind: i32,
    pub state: RadioState,
}

impl Drop for Radio {
    fn drop(&mut self) {
        unsafe { release(self.obj) };
    }
}

/// Lista las radios del equipo. Vacío si WinRT no está disponible o si el
/// usuario no ha dado permiso de acceso a las radios.
pub fn radios() -> Vec<Radio> {
    let mut out = Vec::new();
    let Some(rt) = WinRt::load() else { return out };
    unsafe {
        // HSTRING con el nombre de la clase de WinRT.
        let name = wide("Windows.Devices.Radios.Radio");
        let mut hstr: *mut c_void = std::ptr::null_mut();
        if (rt.create_string)(name.as_ptr(), (name.len() - 1) as u32, &mut hstr) != 0 {
            return out;
        }
        let mut statics: *mut c_void = std::ptr::null_mut();
        let hr = (rt.ro_get_activation_factory)(hstr, &IID_RADIO_STATICS, &mut statics);
        (rt.delete_string)(hstr);
        if hr != 0 || statics.is_null() {
            return out;
        }
        let get_radios: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
            std::mem::transmute(*vtable(statics).add(V_RADIOS_GET_RADIOS_ASYNC));
        let mut op: *mut c_void = std::ptr::null_mut();
        if get_radios(statics, &mut op) == 0 && rt.await_async(op, 1500) {
            let results: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
                std::mem::transmute(*vtable(op).add(V_ASYNC_GET_RESULTS));
            let mut vec: *mut c_void = std::ptr::null_mut();
            if results(op, &mut vec) == 0 && !vec.is_null() {
                let get_size: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 =
                    std::mem::transmute(*vtable(vec).add(V_VECTOR_GET_SIZE));
                let get_at: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32 =
                    std::mem::transmute(*vtable(vec).add(V_VECTOR_GET_AT));
                let mut n = 0u32;
                get_size(vec, &mut n);
                for i in 0..n.min(16) {
                    let mut r: *mut c_void = std::ptr::null_mut();
                    if get_at(vec, i, &mut r) != 0 || r.is_null() {
                        continue;
                    }
                    let get_kind: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32 =
                        std::mem::transmute(*vtable(r).add(V_RADIO_GET_KIND));
                    let get_state: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32 =
                        std::mem::transmute(*vtable(r).add(V_RADIO_GET_STATE));
                    let mut kind = 0i32;
                    let mut state = 0i32;
                    get_kind(r, &mut kind);
                    get_state(r, &mut state);
                    out.push(Radio { obj: r, kind, state: RadioState::from_code(state) });
                }
                release(vec);
            }
        }
        if !op.is_null() {
            release(op);
        }
        release(statics);
    }
    out
}

/// Estado de la radio Bluetooth según WinRT.
pub fn bluetooth_state() -> RadioState {
    radios()
        .iter()
        .find(|r| r.kind == Kind::Bluetooth as i32)
        .map(|r| r.state)
        .unwrap_or(RadioState::Unknown)
}

/// Enciende o apaga la radio Bluetooth. Devuelve false si no hay radio, si
/// WinRT no está disponible o si el sistema deniega el acceso.
pub fn set_bluetooth(on: bool) -> bool {
    let Some(rt) = WinRt::load() else { return false };
    let list = radios();
    let Some(r) = list.iter().find(|r| r.kind == Kind::Bluetooth as i32) else { return false };
    unsafe {
        let set_state: unsafe extern "system" fn(*mut c_void, i32, *mut *mut c_void) -> i32 =
            std::mem::transmute(*vtable(r.obj).add(V_RADIO_SET_STATE_ASYNC));
        let mut op: *mut c_void = std::ptr::null_mut();
        let want = if on { RadioState::On } else { RadioState::Off } as i32;
        if set_state(r.obj, want, &mut op) != 0 {
            return false;
        }
        let ok = rt.await_async(op, 3000);
        if !op.is_null() {
            release(op);
        }
        ok
    }
}

/// Estado conjunto para la interfaz: `None` si no hay radio de ese tipo.
pub fn bluetooth_on() -> Option<bool> {
    match bluetooth_state() {
        RadioState::Unknown => None,
        s => Some(s == RadioState::On),
    }
}
