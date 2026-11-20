// Operaciones de archivo y menú contextual del shell, por COM.
//
// Copiar, mover, eliminar, renombrar y crear carpetas se hacen con
// `IFileOperation`, que es lo que usa el Explorador de Windows: así se obtiene
// la papelera de reciclaje, el diálogo de progreso, la confirmación de
// sobrescritura, la elevación cuando hace falta y el poder deshacer con Ctrl+Z.
// Reimplementarlo con CopyFileW sería peor en todo eso.
//
// El menú contextual es el del propio shell (`IContextMenu`): es la única
// forma de ofrecer «Abrir con», «Enviar a», «Propiedades» y lo que añadan los
// programas instalados. Ese menú sí es un HMENU de Windows, porque es el shell
// quien lo rellena.
//
// Como en `perf`, se llama al vtable a mano en vez de arrastrar una capa de
// COM generado.
#![allow(dead_code)]

use std::ffi::c_void;

use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetMenuItemCount, TrackPopupMenuEx, TPM_RETURNCMD, TPM_RIGHTBUTTON,
};

use crate::ui::wide;

// ---------------------------------------------------------------- COM básico
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

/// COM se inicializa una vez por hilo de interfaz.
pub fn init_com() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static READY: AtomicBool = AtomicBool::new(false);
    if READY.swap(true, Ordering::Relaxed) {
        return;
    }
    unsafe {
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
    }
}

// CLSID_FileOperation {3AD05575-8857-4850-9277-11B85BDB8E09}
const CLSID_FILE_OPERATION: GUID = GUID::from_u128(0x3AD05575_8857_4850_9277_11B85BDB8E09);
// IID_IFileOperation {947AAB5F-0A5C-4C13-B4D6-4BF7836FC9F8}
const IID_FILE_OPERATION: GUID = GUID::from_u128(0x947AAB5F_0A5C_4C13_B4D6_4BF7836FC9F8);
// IID_IShellItem {43826D1E-E718-42EE-BC55-A1E261C37BFE}
const IID_SHELL_ITEM: GUID = GUID::from_u128(0x43826D1E_E718_42EE_BC55_A1E261C37BFE);
// IID_IShellFolder {000214E6-0000-0000-C000-000000000046}
const IID_SHELL_FOLDER: GUID = GUID::from_u128(0x000214E6_0000_0000_C000_000000000046);
// IID_IContextMenu {000214E4-0000-0000-C000-000000000046}
const IID_CONTEXT_MENU: GUID = GUID::from_u128(0x000214E4_0000_0000_C000_000000000046);

// Ranuras del vtable de IFileOperation (tras IUnknown).
const V_SET_OPERATION_FLAGS: usize = 5;
const V_SET_OWNER_WINDOW: usize = 9;
const V_RENAME_ITEM: usize = 12;
const V_MOVE_ITEM: usize = 14;
const V_COPY_ITEM: usize = 16;
const V_DELETE_ITEM: usize = 18;
const V_NEW_ITEM: usize = 20;
const V_PERFORM_OPERATIONS: usize = 21;
const V_ANY_ABORTED: usize = 22;

// Banderas de las operaciones.
const FOF_SILENT: u32 = 0x0004;
const FOF_NOCONFIRMATION: u32 = 0x0010;
const FOF_ALLOWUNDO: u32 = 0x0040;
const FOF_NOERRORUI: u32 = 0x0400;
const FOFX_RECYCLEONDELETE: u32 = 0x0008_0000;
const FOFX_ADDUNDORECORD: u32 = 0x2000_0000;

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

#[link(name = "shell32")]
extern "system" {
    fn SHCreateItemFromParsingName(path: *const u16, bc: *mut c_void, riid: *const GUID,
                                   out: *mut *mut c_void) -> i32;
    fn SHParseDisplayName(name: *const u16, bc: *mut c_void, pidl: *mut *mut c_void,
                          attrs: u32, out_attrs: *mut u32) -> i32;
    fn SHBindToParent(pidl: *const c_void, riid: *const GUID, out: *mut *mut c_void,
                      child: *mut *const c_void) -> i32;
}

/// Envoltura de IShellItem.
struct ShellItem(*mut c_void);

impl ShellItem {
    fn from_path(path: &str) -> Option<ShellItem> {
        unsafe {
            let mut item: *mut c_void = std::ptr::null_mut();
            if SHCreateItemFromParsingName(wide(path).as_ptr(), std::ptr::null_mut(),
                                           &IID_SHELL_ITEM, &mut item) != 0 || item.is_null() {
                return None;
            }
            Some(ShellItem(item))
        }
    }
}

impl Drop for ShellItem {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}

/// Qué hacer con los archivos.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Copy,
    Move,
    /// Eliminar: a la papelera o para siempre.
    Delete { permanent: bool },
}

/// Resultado de una operación: `Ok(true)` si se hizo, `Ok(false)` si el
/// usuario la canceló, `Err` con el motivo si falló.
pub type OpResult = Result<bool, String>;

fn hr_text(hr: i32) -> String {
    match hr as u32 {
        0x8007_0005 => "Acceso denegado.".to_string(),
        0x8007_0002 => "No se encuentra el archivo.".to_string(),
        0x8007_0050 => "Ya existe un archivo con ese nombre.".to_string(),
        0x8027_0000..=0x8027_FFFF => "El shell rechazó la operación.".to_string(),
        _ => format!("Error 0x{:08X} al realizar la operación.", hr),
    }
}

/// Crea el objeto IFileOperation con las banderas y la ventana dueña puestas.
unsafe fn new_operation(owner: HWND, flags: u32) -> Result<*mut c_void, String> {
    init_com();
    let mut op: *mut c_void = std::ptr::null_mut();
    let hr = CoCreateInstance(&CLSID_FILE_OPERATION, std::ptr::null_mut(), CLSCTX_INPROC_SERVER,
                              &IID_FILE_OPERATION, &mut op);
    if hr != 0 || op.is_null() {
        return Err(hr_text(hr));
    }
    let set_flags: unsafe extern "system" fn(*mut c_void, u32) -> i32 =
        std::mem::transmute(*vtable(op).add(V_SET_OPERATION_FLAGS));
    set_flags(op, flags);
    let set_owner: unsafe extern "system" fn(*mut c_void, HWND) -> i32 =
        std::mem::transmute(*vtable(op).add(V_SET_OWNER_WINDOW));
    set_owner(op, owner);
    Ok(op)
}

unsafe fn perform(op: *mut c_void) -> OpResult {
    let run: unsafe extern "system" fn(*mut c_void) -> i32 =
        std::mem::transmute(*vtable(op).add(V_PERFORM_OPERATIONS));
    let hr = run(op);
    let aborted: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32 =
        std::mem::transmute(*vtable(op).add(V_ANY_ABORTED));
    let mut ab = 0i32;
    aborted(op, &mut ab);
    release(op);
    if hr != 0 {
        // COPYENGINE_E_USER_CANCELLED y similares llegan como error.
        if ab != 0 {
            return Ok(false);
        }
        return Err(hr_text(hr));
    }
    Ok(ab == 0)
}

/// Copia o mueve archivos a una carpeta.
pub fn transfer(owner: HWND, paths: &[String], dest_dir: &str, op_kind: Op) -> OpResult {
    if paths.is_empty() {
        return Ok(false);
    }
    unsafe {
        let flags = FOF_ALLOWUNDO | FOFX_ADDUNDORECORD;
        let op = new_operation(owner, flags)?;
        let Some(dest) = ShellItem::from_path(dest_dir) else {
            release(op);
            return Err("La carpeta de destino no es válida.".into());
        };
        let slot = if op_kind == Op::Move { V_MOVE_ITEM } else { V_COPY_ITEM };
        let add: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *const u16, *mut c_void) -> i32 =
            std::mem::transmute(*vtable(op).add(slot));
        let mut any = false;
        for p in paths {
            if let Some(item) = ShellItem::from_path(p) {
                if add(op, item.0, dest.0, std::ptr::null(), std::ptr::null_mut()) == 0 {
                    any = true;
                }
            }
        }
        if !any {
            release(op);
            return Err("No se pudo preparar ningún archivo.".into());
        }
        perform(op)
    }
}

/// Copia un elemento a una carpeta dándole otro nombre. Es lo que hace falta
/// para pegar en la misma carpeta de la que se copió: el shell no duplica un
/// archivo sobre sí mismo, hay que pedirle el nombre nuevo.
pub fn copy_as(owner: HWND, path: &str, dest_dir: &str, new_name: &str) -> OpResult {
    unsafe {
        let op = new_operation(owner, FOF_ALLOWUNDO | FOFX_ADDUNDORECORD)?;
        let (Some(item), Some(dest)) = (ShellItem::from_path(path), ShellItem::from_path(dest_dir))
        else {
            release(op);
            return Err("Ruta no válida.".into());
        };
        let add: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *const u16, *mut c_void) -> i32 =
            std::mem::transmute(*vtable(op).add(V_COPY_ITEM));
        if add(op, item.0, dest.0, wide(new_name).as_ptr(), std::ptr::null_mut()) != 0 {
            release(op);
            return Err("No se pudo preparar la copia.".into());
        }
        perform(op)
    }
}

/// Nombre libre al estilo de Windows: «archivo - copia.txt», « - copia (2)»...
pub fn free_copy_name(dir: &str, name: &str) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let exists = |n: &str| std::path::Path::new(dir).join(n).exists();
    let first = format!("{} - copia{}", stem, ext);
    if !exists(&first) {
        return first;
    }
    for i in 2..1000 {
        let n = format!("{} - copia ({}){}", stem, i, ext);
        if !exists(&n) {
            return n;
        }
    }
    first
}

/// Elimina archivos: a la papelera o para siempre.
pub fn delete(owner: HWND, paths: &[String], permanent: bool) -> OpResult {
    if paths.is_empty() {
        return Ok(false);
    }
    unsafe {
        // Sin FOF_ALLOWUNDO el shell borra de verdad; con él, va a la papelera.
        let flags = if permanent {
            FOF_NOCONFIRMATION
        } else {
            FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE | FOFX_ADDUNDORECORD
        };
        let op = new_operation(owner, flags)?;
        let del: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> i32 =
            std::mem::transmute(*vtable(op).add(V_DELETE_ITEM));
        let mut any = false;
        for p in paths {
            if let Some(item) = ShellItem::from_path(p) {
                if del(op, item.0, std::ptr::null_mut()) == 0 {
                    any = true;
                }
            }
        }
        if !any {
            release(op);
            return Err("No se pudo preparar ningún archivo.".into());
        }
        perform(op)
    }
}

/// Cambia el nombre de un archivo o carpeta.
pub fn rename(owner: HWND, path: &str, new_name: &str) -> OpResult {
    if new_name.trim().is_empty() {
        return Ok(false);
    }
    unsafe {
        let op = new_operation(owner, FOF_ALLOWUNDO | FOFX_ADDUNDORECORD)?;
        let Some(item) = ShellItem::from_path(path) else {
            release(op);
            return Err("No se encuentra el archivo.".into());
        };
        let ren: unsafe extern "system" fn(*mut c_void, *mut c_void, *const u16, *mut c_void) -> i32 =
            std::mem::transmute(*vtable(op).add(V_RENAME_ITEM));
        if ren(op, item.0, wide(new_name).as_ptr(), std::ptr::null_mut()) != 0 {
            release(op);
            return Err("No se pudo preparar el cambio de nombre.".into());
        }
        perform(op)
    }
}

/// Crea una carpeta nueva dentro de otra.
pub fn new_folder(owner: HWND, parent: &str, name: &str) -> OpResult {
    unsafe {
        let op = new_operation(owner, FOF_ALLOWUNDO | FOFX_ADDUNDORECORD)?;
        let Some(dest) = ShellItem::from_path(parent) else {
            release(op);
            return Err("La carpeta no es válida.".into());
        };
        let new: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, *const u16, *const u16, *mut c_void) -> i32 =
            std::mem::transmute(*vtable(op).add(V_NEW_ITEM));
        if new(op, dest.0, FILE_ATTRIBUTE_DIRECTORY, wide(name).as_ptr(),
               std::ptr::null(), std::ptr::null_mut()) != 0 {
            release(op);
            return Err("No se pudo preparar la carpeta.".into());
        }
        perform(op)
    }
}

// ---------------------------------------------------------------- menú del shell
/// Enseña el menú contextual del shell para los archivos dados y ejecuta lo
/// que elija el usuario.
///
/// Este menú sí es un HMENU del sistema: lo rellenan el shell y las
/// extensiones instaladas, así que no se puede dibujar con el tema propio.
/// Devuelve true si se mostró.
pub fn shell_context_menu(owner: HWND, paths: &[String], x: i32, y: i32) -> bool {
    if paths.is_empty() {
        return false;
    }
    init_com();
    unsafe {
        // Lista de identificadores (PIDL) de los archivos, todos de la misma carpeta.
        let mut pidls: Vec<*mut c_void> = Vec::with_capacity(paths.len());
        for p in paths {
            let mut pidl: *mut c_void = std::ptr::null_mut();
            if SHParseDisplayName(wide(p).as_ptr(), std::ptr::null_mut(), &mut pidl, 0,
                                  std::ptr::null_mut()) == 0 && !pidl.is_null() {
                pidls.push(pidl);
            }
        }
        if pidls.is_empty() {
            return false;
        }
        // La carpeta que los contiene y los identificadores relativos a ella.
        let mut folder: *mut c_void = std::ptr::null_mut();
        let mut children: Vec<*const c_void> = Vec::with_capacity(pidls.len());
        let mut child: *const c_void = std::ptr::null();
        let ok = SHBindToParent(pidls[0], &IID_SHELL_FOLDER, &mut folder, &mut child) == 0
            && !folder.is_null();
        if ok {
            children.push(child);
            for p in pidls.iter().skip(1) {
                let mut f2: *mut c_void = std::ptr::null_mut();
                let mut c2: *const c_void = std::ptr::null();
                if SHBindToParent(*p, &IID_SHELL_FOLDER, &mut f2, &mut c2) == 0 {
                    children.push(c2);
                    release(f2);
                }
            }
        }
        let mut shown = false;
        if ok {
            let get_ui: unsafe extern "system" fn(*mut c_void, HWND, u32, *const *const c_void,
                                                  *const GUID, *mut u32, *mut *mut c_void) -> i32 =
                std::mem::transmute(*vtable(folder).add(10));
            let mut cm: *mut c_void = std::ptr::null_mut();
            if get_ui(folder, owner, children.len() as u32, children.as_ptr(), &IID_CONTEXT_MENU,
                      std::ptr::null_mut(), &mut cm) == 0 && !cm.is_null()
            {
                let hmenu = CreatePopupMenu();
                if !hmenu.is_null() {
                    let query: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, u32, u32, u32) -> i32 =
                        std::mem::transmute(*vtable(cm).add(3));
                    // CMF_NORMAL = 0; CMF_EXPLORE = 4 añade lo propio del explorador.
                    if query(cm, hmenu as *mut c_void, 0, 1, 0x7FFF, 4) >= 0
                        && GetMenuItemCount(hmenu) > 0
                    {
                        let cmd = TrackPopupMenuEx(hmenu, TPM_RETURNCMD | TPM_RIGHTBUTTON, x, y,
                                                   owner, std::ptr::null());
                        shown = true;
                        if cmd > 0 {
                            invoke(cm, owner, (cmd - 1) as u32);
                        }
                    }
                    DestroyMenu(hmenu);
                }
                release(cm);
            }
            release(folder);
        }
        for p in pidls {
            CoTaskMemFree(p);
        }
        shown
    }
}

/// Ejecuta el comando elegido del menú contextual.
unsafe fn invoke(cm: *mut c_void, owner: HWND, id: u32) {
    // CMINVOKECOMMANDINFOEX, en su versión ancha.
    #[repr(C)]
    struct InvokeInfo {
        cb_size: u32,
        mask: u32,
        hwnd: HWND,
        verb: *const u8,
        parameters: *const u8,
        directory: *const u8,
        show: i32,
        hotkey: u32,
        icon: *mut c_void,
        title: *const u8,
        verb_w: *const u16,
        parameters_w: *const u16,
        directory_w: *const u16,
        title_w: *const u16,
        pt: windows_sys::Win32::Foundation::POINT,
    }
    const CMIC_MASK_UNICODE: u32 = 0x0000_4000;
    let mut info: InvokeInfo = std::mem::zeroed();
    info.cb_size = std::mem::size_of::<InvokeInfo>() as u32;
    info.mask = CMIC_MASK_UNICODE;
    info.hwnd = owner;
    // El verbo puede ser el identificador numérico del comando.
    info.verb = id as usize as *const u8;
    info.verb_w = id as usize as *const u16;
    info.show = 1; // SW_SHOWNORMAL
    let run: unsafe extern "system" fn(*mut c_void, *const c_void) -> i32 =
        std::mem::transmute(*vtable(cm).add(4));
    run(cm, &info as *const _ as *const c_void);
}

// ---------------------------------------------------------------- propiedades
/// Abre el cuadro de propiedades del shell para un archivo.
pub fn show_properties(owner: HWND, path: &str) -> bool {
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW};
    unsafe {
        let verb = wide("properties");
        let file = wide(path);
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_INVOKEIDLIST;
        info.hwnd = owner;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.nShow = 1;
        ShellExecuteExW(&mut info) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    /// Ejercita de verdad IFileOperation en una carpeta temporal: crear,
    /// copiar, renombrar y eliminar. Si el shell cambiara la interfaz, esto
    /// lo avisa antes de que lo note el usuario.
    fn operaciones_reales() {
        let base = std::env::temp_dir().join("oshell_fileops_test");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let b = base.to_string_lossy().to_string();

        // Carpeta nueva
        let r = new_folder(std::ptr::null_mut(), &b, "destino");
        assert!(r.is_ok(), "new_folder: {:?}", r.err());
        assert!(base.join("destino").is_dir(), "no se creo la carpeta");

        // Copiar un archivo a esa carpeta
        let src = base.join("origen.txt");
        std::fs::write(&src, b"hola").unwrap();
        let r = transfer(std::ptr::null_mut(), &[src.to_string_lossy().to_string()],
                         &base.join("destino").to_string_lossy().to_string(), Op::Copy);
        assert!(r.is_ok(), "copy: {:?}", r.err());
        assert!(base.join("destino").join("origen.txt").is_file(), "no se copio");

        // Renombrar
        let r = rename(std::ptr::null_mut(),
                       &base.join("destino").join("origen.txt").to_string_lossy().to_string(),
                       "renombrado.txt");
        assert!(r.is_ok(), "rename: {:?}", r.err());
        assert!(base.join("destino").join("renombrado.txt").is_file(), "no se renombro");

        // Eliminar permanentemente
        let r = delete(std::ptr::null_mut(),
                       &[base.join("destino").join("renombrado.txt").to_string_lossy().to_string()],
                       true);
        assert!(r.is_ok(), "delete: {:?}", r.err());
        assert!(!base.join("destino").join("renombrado.txt").exists(), "no se elimino");

        let _ = std::fs::remove_dir_all(&base);
    }
}
