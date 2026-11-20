// Autoactualización desde GitHub.
//
// Mira las releases del repositorio (la API pública de GitHub, sin credenciales),
// compara la etiqueta `vX.Y.Z` con la versión compilada, descarga el
// `OptimizedShell.exe` publicado, comprueba su SHA-256 contra el fichero
// `OptimizedShell.exe.sha256` de la misma release y sustituye el ejecutable en
// caliente.
//
// Todo con FFI propia (WinHTTP para la red y BCrypt para el hash): nada de
// dependencias añadidas, igual que el resto del programa.
//
// Modos:
//   --update              busca, pregunta y actualiza (con ventanas de aviso)
//   --update --silent     busca en silencio y solo avisa por notificación
//   --update --forzar     reinstala aunque la versión publicada no sea mayor
//
// Un ejecutable en uso no se puede borrar, pero sí renombrar: se aparta a
// `OptimizedShell.exe.old`, se escribe el nuevo en su sitio y el `.old` se
// borra en el siguiente arranque (`cleanup_old`).
#![allow(dead_code)]

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use crate::notify;
use crate::ui;

/// Repositorio del que se descargan las versiones.
pub const REPO: &str = "isaachhk02/OptimizedShell";
/// Nombre del archivo que se publica en la release y que hay que sustituir.
const ASSET: &str = "OptimizedShell.exe";
const API_HOST: &str = "api.github.com";
const AGENT: &str = "OptimizedShell-Updater";

/// Versión compilada (la de Cargo.toml).
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// ============================================================ WinHTTP (FFI)
type HInternet = *mut c_void;

const WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY: u32 = 4;
const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;
const WINHTTP_HEADER_NAME_BY_INDEX: *const u16 = std::ptr::null();

#[link(name = "winhttp")]
extern "system" {
    fn WinHttpOpen(agent: *const u16, access: u32, proxy: *const u16,
                   bypass: *const u16, flags: u32) -> HInternet;
    fn WinHttpConnect(session: HInternet, server: *const u16, port: u16,
                      reserved: u32) -> HInternet;
    fn WinHttpOpenRequest(connect: HInternet, verb: *const u16, object: *const u16,
                          version: *const u16, referrer: *const u16,
                          accept_types: *const *const u16, flags: u32) -> HInternet;
    fn WinHttpSendRequest(request: HInternet, headers: *const u16, headers_len: u32,
                          optional: *const c_void, optional_len: u32,
                          total_len: u32, context: usize) -> i32;
    fn WinHttpReceiveResponse(request: HInternet, reserved: *mut c_void) -> i32;
    fn WinHttpQueryHeaders(request: HInternet, info_level: u32, name: *const u16,
                           buffer: *mut c_void, buffer_len: *mut u32,
                           index: *mut u32) -> i32;
    fn WinHttpReadData(request: HInternet, buffer: *mut c_void, to_read: u32,
                       read: *mut u32) -> i32;
    fn WinHttpCloseHandle(h: HInternet) -> i32;
}

/// Partes de una URL https (solo se habla https con GitHub).
struct Url {
    host: String,
    path: String,
}

fn split_url(url: &str) -> Option<Url> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if host.is_empty() {
        return None;
    }
    Some(Url { host: host.to_string(), path: path.to_string() })
}

/// Token de acceso a GitHub. El repositorio es público, así que normalmente no
/// hace falta ninguno; sirve para dos casos: subir el límite de peticiones
/// anónimas de la API (60 por hora y por IP) y poder actualizar si algún día el
/// repositorio vuelve a ser privado. Se busca, por orden, en la variable de
/// entorno `OPTIMIZEDSHELL_TOKEN`, en `GH_TOKEN`, en `GITHUB_TOKEN` y en el
/// archivo `%APPDATA%\OptimizedShell\github.token` (una sola línea).
///
/// Basta un token de solo lectura (classic con `repo`, o fine-grained con
/// permiso de lectura de «Contents» sobre este repositorio).
pub fn token() -> Option<String> {
    for var in ["OPTIMIZEDSHELL_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(v) = std::env::var(var) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    let f = token_file();
    let v = std::fs::read_to_string(f).ok()?.trim().to_string();
    if v.is_empty() { None } else { Some(v) }
}

/// Archivo donde se puede dejar el token, junto al config.ini.
pub fn token_file() -> PathBuf {
    crate::ui::config::config_file().with_file_name("github.token")
}

/// GET de una URL https. Devuelve el cuerpo si el servidor responde 2xx.
/// WinHTTP sigue las redirecciones por su cuenta, que es lo que hace falta
/// para las descargas de GitHub (acaban en objects.githubusercontent.com).
///
/// `accept` elige qué se pide: JSON para la API y `application/octet-stream`
/// para bajarse un archivo adjunto por la API, que es la vía que funciona con
/// y sin token (en un repositorio privado es la única).
fn http_get(url: &str) -> Option<Vec<u8>> {
    http_get_as(url, "application/vnd.github+json")
}

fn http_get_as(url: &str, accept: &str) -> Option<Vec<u8>> {
    let u = split_url(url)?;
    let mut headers = format!("Accept: {}\r\nX-GitHub-Api-Version: 2022-11-28\r\n", accept);
    if let Some(t) = token() {
        headers.push_str(&format!("Authorization: Bearer {}\r\n", t));
    }
    let accept_json = headers;
    unsafe {
        let session = WinHttpOpen(ui::wide(AGENT).as_ptr(),
                                  WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                                  std::ptr::null(), std::ptr::null(), 0);
        if session.is_null() {
            return None;
        }
        let conn = WinHttpConnect(session, ui::wide(&u.host).as_ptr(), 443, 0);
        if conn.is_null() {
            WinHttpCloseHandle(session);
            return None;
        }
        let req = WinHttpOpenRequest(conn, ui::wide("GET").as_ptr(),
                                     ui::wide(&u.path).as_ptr(), std::ptr::null(),
                                     std::ptr::null(), std::ptr::null(),
                                     WINHTTP_FLAG_SECURE);
        let mut body: Option<Vec<u8>> = None;
        if !req.is_null() {
            let hdr = ui::wide(&accept_json);
            let sent = WinHttpSendRequest(req, hdr.as_ptr(), u32::MAX,
                                          std::ptr::null(), 0, 0, 0) != 0
                && WinHttpReceiveResponse(req, std::ptr::null_mut()) != 0;
            if sent && status_code(req).map_or(false, |s| (200..300).contains(&s)) {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 16 * 1024];
                loop {
                    let mut read: u32 = 0;
                    if WinHttpReadData(req, chunk.as_mut_ptr() as *mut c_void,
                                       chunk.len() as u32, &mut read) == 0 {
                        buf.clear();
                        break;
                    }
                    if read == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..read as usize]);
                    // Un ejecutable de 1 MB cabe de sobra; corta descargas raras.
                    if buf.len() > 64 * 1024 * 1024 {
                        buf.clear();
                        break;
                    }
                }
                if !buf.is_empty() {
                    body = Some(buf);
                }
            }
            WinHttpCloseHandle(req);
        }
        WinHttpCloseHandle(conn);
        WinHttpCloseHandle(session);
        body
    }
}

fn status_code(req: HInternet) -> Option<u32> {
    let mut code: u32 = 0;
    let mut len = std::mem::size_of::<u32>() as u32;
    let ok = unsafe {
        WinHttpQueryHeaders(req, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                            WINHTTP_HEADER_NAME_BY_INDEX, &mut code as *mut _ as *mut c_void,
                            &mut len, std::ptr::null_mut())
    };
    if ok != 0 { Some(code) } else { None }
}

// ============================================================ SHA-256 (BCrypt)
#[link(name = "bcrypt")]
extern "system" {
    fn BCryptOpenAlgorithmProvider(alg: *mut *mut c_void, id: *const u16,
                                   implementation: *const u16, flags: u32) -> i32;
    fn BCryptCloseAlgorithmProvider(alg: *mut c_void, flags: u32) -> i32;
    fn BCryptHash(alg: *mut c_void, secret: *mut u8, secret_len: u32,
                  input: *mut u8, input_len: u32, output: *mut u8,
                  output_len: u32) -> i32;
}

/// SHA-256 en hexadecimal minúsculo, o `None` si CNG falla.
pub fn sha256_hex(data: &[u8]) -> Option<String> {
    unsafe {
        let mut alg: *mut c_void = std::ptr::null_mut();
        if BCryptOpenAlgorithmProvider(&mut alg, ui::wide("SHA256").as_ptr(),
                                       std::ptr::null(), 0) < 0 {
            return None;
        }
        let mut out = [0u8; 32];
        let hr = BCryptHash(alg, std::ptr::null_mut(), 0,
                            data.as_ptr() as *mut u8, data.len() as u32,
                            out.as_mut_ptr(), out.len() as u32);
        BCryptCloseAlgorithmProvider(alg, 0);
        if hr < 0 {
            return None;
        }
        let mut s = String::with_capacity(64);
        for b in out {
            s.push_str(&format!("{:02x}", b));
        }
        Some(s)
    }
}

// ============================================================ releases
pub struct Release {
    /// Etiqueta tal cual («v0.6.0»).
    pub tag: String,
    /// Versión sin la «v» («0.6.0»).
    pub version: String,
    /// URL del `OptimizedShell.exe` publicado.
    pub exe_url: String,
    /// URL del `.sha256` que acompaña al ejecutable, si está publicado.
    pub sha_url: Option<String>,
}

/// Última release publicada en GitHub (`releases/latest`, que ya omite los
/// borradores y las prereleases).
pub fn latest() -> Option<Release> {
    let url = format!("https://{}/repos/{}/releases/latest", API_HOST, REPO);
    let body = http_get(&url)?;
    let json = String::from_utf8_lossy(&body);
    let tag = json_string(&json, "tag_name")?;
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    let assets = assets(&json);
    let find = |name: &str| {
        assets
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
            .map(|a| a.url.clone())
    };
    let exe_url = find(ASSET)?;
    let sha_url = find(&format!("{}.sha256", ASSET));
    Some(Release { tag, version, exe_url, sha_url })
}

/// Un archivo adjunto de la release.
struct Asset {
    name: String,
    /// URL de la API (`.../releases/assets/<id>`). Se usa esta y no
    /// `browser_download_url` porque la de la API vale en los dos casos: en un
    /// repositorio privado es la única que acepta el token.
    url: String,
}

/// Adjuntos de la release. El objeto de cada adjunto empieza por su `"url"`,
/// así que se parte el array por ahí y de cada trozo se saca la URL y el
/// nombre; nada de traer un parser de JSON entero para esto.
fn assets(json: &str) -> Vec<Asset> {
    let mut out = Vec::new();
    let Some(arr) = json.split("\"assets\":").nth(1) else {
        return out;
    };
    // El array acaba donde empieza la clave siguiente de la release.
    let arr = arr.split("\"tarball_url\"").next().unwrap_or(arr);
    for chunk in arr.split("\"url\":").skip(1) {
        let url = chunk
            .trim_start()
            .strip_prefix('"')
            .and_then(|v| v.split('"').next())
            .unwrap_or("");
        if !url.contains("/releases/assets/") {
            continue;
        }
        if let Some(name) = json_string(chunk, "name") {
            out.push(Asset { name, url: unescape(url) });
        }
    }
    out
}

/// Valor de una clave de cadena del JSON, sin traer un parser entero: las
/// respuestas de la API de GitHub son planas y basta con buscar `"clave": "..."`.
fn json_string(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let mut rest = json;
    loop {
        let i = rest.find(&needle)? + needle.len();
        let after = rest[i..].trim_start();
        if let Some(v) = after.strip_prefix(':') {
            let v = v.trim_start();
            if let Some(v) = v.strip_prefix('"') {
                let end = v.find('"')?;
                return Some(unescape(&v[..end]));
            }
            // El valor no es una cadena (null, número...): sigue buscando.
        }
        rest = &rest[i..];
    }
}

/// Todas las `browser_download_url` de la release.
fn download_urls(json: &str) -> Vec<String> {
    let needle = "\"browser_download_url\"";
    let mut out = Vec::new();
    let mut rest = json;
    while let Some(i) = rest.find(needle) {
        rest = &rest[i + needle.len()..];
        let v = rest.trim_start();
        if let Some(v) = v.strip_prefix(':') {
            let v = v.trim_start();
            if let Some(v) = v.strip_prefix('"') {
                if let Some(end) = v.find('"') {
                    out.push(unescape(&v[..end]));
                }
            }
        }
    }
    out
}

fn unescape(s: &str) -> String {
    s.replace("\\/", "/").replace("\\\"", "\"")
}

/// ¿`candidate` es una versión mayor que `base`? Compara campo a campo
/// (`0.10.0` > `0.9.9`); lo que no sea número cuenta como 0.
pub fn is_newer(candidate: &str, base: &str) -> bool {
    let parts = |v: &str| -> Vec<u32> {
        v.split(['.', '-', '+'])
            .map(|p| p.trim().parse::<u32>().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parts(candidate), parts(base));
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

// ============================================================ descarga e instalación
/// Descarga el ejecutable de la release y lo deja en `%TEMP%`, comprobando el
/// SHA-256 publicado si lo hay. Devuelve la ruta del archivo descargado.
pub fn download(rel: &Release) -> Result<PathBuf, String> {
    let bytes = http_get_as(&rel.exe_url, "application/octet-stream")
        .ok_or("no se ha podido descargar el archivo")?;
    if bytes.len() < 64 * 1024 || &bytes[..2] != b"MZ" {
        return Err("el archivo descargado no es un ejecutable válido".into());
    }
    if let Some(sha_url) = &rel.sha_url {
        if let Some(raw) = http_get_as(sha_url, "application/octet-stream") {
            let txt = String::from_utf8_lossy(&raw);
            let want = txt
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            let got = sha256_hex(&bytes).ok_or("no se ha podido calcular el SHA-256")?;
            if want.len() == 64 && want != got {
                return Err(format!(
                    "el SHA-256 no coincide con el publicado (esperado {}..., obtenido {}...)",
                    &want[..8], &got[..8]
                ));
            }
        }
    }
    let tmp = std::env::temp_dir().join(format!("OptimizedShell-{}.exe", rel.version));
    std::fs::write(&tmp, &bytes).map_err(|e| format!("no se ha podido escribir {}: {}", tmp.display(), e))?;
    Ok(tmp)
}

/// Ruta del ejecutable en ejecución.
pub fn own_exe() -> PathBuf {
    let mut buf = [0u16; 1024];
    let n = unsafe {
        windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(
            std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32)
    };
    PathBuf::from(String::from_utf16_lossy(&buf[..n as usize]))
}

fn old_path(exe: &Path) -> PathBuf {
    let mut p = exe.as_os_str().to_os_string();
    p.push(".old");
    PathBuf::from(p)
}

/// Borra el ejecutable viejo que dejó una actualización anterior. Se llama al
/// arrancar: para entonces ya nadie lo tiene abierto.
pub fn cleanup_old() {
    let old = old_path(&own_exe());
    if old.exists() {
        let _ = std::fs::remove_file(&old);
    }
}

/// Sustituye el ejecutable en marcha por el descargado.
///
/// Un `.exe` en uso no se puede borrar, pero sí renombrar, así que el que está
/// corriendo se aparta a `.old` (se borrará en el siguiente arranque) y el
/// nuevo ocupa su sitio. Si había una barra de tareas en ejecución se le pide
/// que se cierre y se vuelve a lanzar ya con la versión nueva.
pub fn apply(new_exe: &Path) -> Result<bool, String> {
    let exe = own_exe();
    let old = old_path(&exe);
    let _ = std::fs::remove_file(&old);

    let was_shell = close_running_shell();

    std::fs::rename(&exe, &old)
        .map_err(|e| format!("no se ha podido apartar {}: {}", exe.display(), e))?;
    if let Err(e) = std::fs::copy(new_exe, &exe) {
        // Deshacer: mejor quedarse como se estaba que sin ejecutable.
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("no se ha podido copiar la versión nueva: {}", e));
    }
    let _ = std::fs::remove_file(new_exe);

    if was_shell {
        relaunch(&exe);
    }
    Ok(was_shell)
}

/// Cierra la barra de tareas que haya en ejecución. Devuelve `true` si había
/// una (y por tanto hay que volver a lanzarla).
fn close_running_shell() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, PostMessageW, WM_CLOSE,
    };
    unsafe {
        let bar = FindWindowW(ui::wide(crate::shell::CLASS).as_ptr(), std::ptr::null());
        if bar.is_null() {
            return false;
        }
        PostMessageW(bar, WM_CLOSE, 0, 0);
        // Darle un momento a que suelte el ejecutable y sus ventanas.
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if FindWindowW(ui::wide(crate::shell::CLASS).as_ptr(), std::ptr::null()).is_null() {
                break;
            }
        }
        true
    }
}

fn relaunch(exe: &Path) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), ui::wide("open").as_ptr(),
                      ui::wide(&exe.to_string_lossy()).as_ptr(), std::ptr::null(),
                      std::ptr::null(), SW_SHOWNORMAL);
    }
}

// ============================================================ modos de uso
/// `--update`: busca una versión nueva y, si la hay, la instala.
///
/// Con `--silent` no abre ninguna ventana: solo manda una notificación a la
/// barra de tareas, que es como se usa en la comprobación automática.
pub fn run() -> i32 {
    let silent = crate::args::has_arg("--silent");
    let force = crate::args::has_arg("--forzar");
    cleanup_old();

    let rel = match latest() {
        Some(r) => r,
        None => {
            let body = if token().is_none() {
                format!("No se ha podido consultar GitHub.\n\n\
                         Comprueba la conexión. Si el problema es el límite de peticiones \
                         anónimas de la API, deja un token de solo lectura en una línea en:\n{}",
                        token_file().display())
            } else {
                "No se ha podido consultar GitHub. Comprueba la conexión y que el token \
                 siga siendo válido y tenga acceso al repositorio.".to_string()
            };
            report(silent, notify::Kind::Error, "Actualizaciones", &body);
            return 1;
        }
    };

    if !force && !is_newer(&rel.version, current_version()) {
        if !silent {
            report(false, notify::Kind::Success, "OptimizedShell está al día",
                   &format!("La versión instalada ({}) es la más reciente.", current_version()));
        }
        return 0;
    }

    if !silent && !confirm(&format!(
        "Hay una versión nueva de OptimizedShell.\n\n\
         Instalada: {}\nDisponible: {}\n\n\
         ¿Descargarla e instalarla ahora? La barra de tareas se reiniciará.",
        current_version(), rel.version)) {
        return 0;
    }
    if silent {
        // En automático solo se avisa; instalar queda en manos del usuario.
        notify::post("OptimizedShell", "Hay una versión nueva",
                     &format!("OptimizedShell {} está disponible. Abre Configuración › Acerca de para instalarla.",
                              rel.version),
                     notify::Kind::Info);
        return 0;
    }

    let file = match download(&rel) {
        Ok(f) => f,
        Err(e) => {
            report(false, notify::Kind::Error, "No se ha podido actualizar", &e);
            return 1;
        }
    };
    match apply(&file) {
        Ok(_) => {
            report(false, notify::Kind::Success, "OptimizedShell actualizado",
                   &format!("Ya está instalada la versión {}.", rel.version));
            0
        }
        Err(e) => {
            report(false, notify::Kind::Error, "No se ha podido actualizar", &e);
            1
        }
    }
}

/// Comprobación automática al arrancar la barra de tareas: en un hilo aparte,
/// para no retrasar el arranque, y solo si el ajuste está activado y ha pasado
/// el intervalo elegido.
pub fn check_on_start() {
    use crate::ui::config::{self, Id};
    cleanup_old();
    let c = config::current();
    if c.get(Id::ActualizarAuto) == 0 {
        return;
    }
    let days = c.get(Id::ActualizarCadaDias).max(1) as u64;
    if !due(days) {
        return;
    }
    std::thread::spawn(move || {
        ui::set_thread_eco(None);
        // Un poco de margen: primero que la barra acabe de montarse y que la
        // red esté levantada.
        std::thread::sleep(std::time::Duration::from_secs(20));
        if let Some(rel) = latest() {
            stamp_now();
            if is_newer(&rel.version, current_version()) {
                notify::post("OptimizedShell", "Hay una versión nueva",
                             &format!("OptimizedShell {} está disponible. Abre Configuración › Acerca de para instalarla.",
                                      rel.version),
                             notify::Kind::Info);
            }
        }
    });
}

/// Marca de la última comprobación, junto al config.ini.
fn stamp_file() -> PathBuf {
    crate::ui::config::config_file().with_file_name("update.stamp")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn due(days: u64) -> bool {
    let last = std::fs::read_to_string(stamp_file())
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    now_secs().saturating_sub(last) >= days * 24 * 60 * 60
}

fn stamp_now() {
    let f = stamp_file();
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(f, now_secs().to_string());
}

// ------------------------------------------------------------ avisos
fn report(silent: bool, kind: notify::Kind, title: &str, body: &str) {
    if silent {
        notify::post("OptimizedShell", title, body, kind);
        return;
    }
    // Si hay barra de tareas, el aviso se ve mejor como notificación; si no
    // (o si no la hay), con un cuadro de diálogo.
    if !notify::post("OptimizedShell", title, body, kind) {
        message_box(title, body, kind);
    }
}

fn message_box(title: &str, body: &str, kind: notify::Kind) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_TOPMOST,
    };
    let icon = match kind {
        notify::Kind::Error => MB_ICONERROR,
        _ => MB_ICONINFORMATION,
    };
    unsafe {
        MessageBoxW(std::ptr::null_mut(), ui::wide(body).as_ptr(),
                    ui::wide(title).as_ptr(), MB_OK | icon | MB_TOPMOST);
    }
}

fn confirm(text: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_ICONQUESTION, MB_TOPMOST, MB_YESNO,
    };
    unsafe {
        MessageBoxW(std::ptr::null_mut(), ui::wide(text).as_ptr(),
                    ui::wide("Actualizar OptimizedShell").as_ptr(),
                    MB_YESNO | MB_ICONQUESTION | MB_TOPMOST) == IDYES
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn compara_versiones() {
        assert!(is_newer("0.6.0", "0.5.1"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.5.1", "0.5.1"));
        assert!(!is_newer("0.5.0", "0.5.1"));
        assert!(!is_newer("", "0.5.1"));
    }

    #[test]
    fn lee_el_json_de_la_release() {
        let json = r#"{"url":"https://api.github.com/repos/x/y/releases/9","tag_name":"v0.6.0",
          "draft":false,
          "assets":[{"url":"https://api.github.com/repos/x/y/releases/assets/1","id":1,
                     "node_id":"RA_1","name":"OptimizedShell.exe.sha256","label":null,
                     "uploader":{"login":"x","name":"otro"},
                     "browser_download_url":"https://github.com/x/y/releases/download/v0.6.0/OptimizedShell.exe.sha256"},
                    {"url":"https://api.github.com/repos/x/y/releases/assets/2","id":2,
                     "node_id":"RA_2","name":"OptimizedShell.exe","label":null,
                     "uploader":{"login":"x","name":"otro"},
                     "browser_download_url":"https://github.com/x/y/releases/download/v0.6.0/OptimizedShell.exe"}],
          "tarball_url":null}"#;
        assert_eq!(json_string(json, "tag_name").as_deref(), Some("v0.6.0"));
        let a = assets(json);
        assert_eq!(a.len(), 2);
        assert_eq!(a[1].name, "OptimizedShell.exe");
        assert_eq!(a[1].url, "https://api.github.com/repos/x/y/releases/assets/2");
        assert!(a.iter().all(|x| x.url.contains("/releases/assets/")));
    }

    #[test]
    fn parte_las_urls() {
        let u = split_url("https://api.github.com/repos/a/b/releases/latest").unwrap();
        assert_eq!(u.host, "api.github.com");
        assert_eq!(u.path, "/repos/a/b/releases/latest");
        assert!(split_url("http://api.github.com/x").is_none());
    }

    /// Comprobación manual contra GitHub (`cargo test -- --ignored`): no entra
    /// en la batería normal porque necesita red.
    #[test]
    #[ignore]
    fn consulta_la_ultima_release() {
        let rel = latest().expect("GitHub no ha respondido");
        assert!(rel.tag.starts_with('v'), "etiqueta inesperada: {}", rel.tag);
        assert!(rel.exe_url.contains("/releases/assets/"), "{}", rel.exe_url);
        assert!(rel.sha_url.is_some());
    }

    /// Descarga de verdad el ejecutable publicado y comprueba su SHA-256
    /// contra el de la release (`cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn descarga_y_comprueba_el_hash() {
        let Some(rel) = latest() else { return };
        let file = download(&rel).expect("la descarga ha fallado");
        let bytes = std::fs::read(&file).unwrap();
        assert!(bytes.starts_with(b"MZ"));
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn hash_conocido() {
        // SHA-256 de la cadena vacía.
        assert_eq!(sha256_hex(b"").unwrap(),
                   "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }
}
