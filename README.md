# OptimizedShell

Shell y utilidades ligeras para Windows 10/11 escritas en **Rust** sobre Win32 + GDI+
(crate `windows-sys`), sin frameworks de UI ni dependencias pesadas. El objetivo es
sustituir a `explorer.exe` y a las herramientas del sistema por versiones que consuman
mucha menos CPU y RAM.

Un solo ejecutable, `OptimizedShell.exe`, con varios modos:

| Modo | Comando | Qué hace |
|---|---|---|
| Shell | `OptimizedShell.exe` | Barra de tareas acoplada abajo con botón Inicio, ventanas abiertas, reloj y menú Inicio |
| Administrador de tareas | `--taskmgr` | Procesos (CPU/memoria/hilos), Inicio, Rendimiento, Servicios |
| Explorador de archivos | `--explorer [ruta]` | Navegación, panel lateral y columnas ordenables |
| Bloqueo / inicio de sesión | `--lock` · `--logon [--demo]` | Pantalla de bloqueo propia con autenticación |
| Configuración | `--settings [fondo\|pantalla\|bluetooth]` | Tema, fondo de escritorio, pantalla y Bluetooth |
| Instalación | `--install` · `--uninstall` | Usa OptimizedShell como shell del usuario actual, o vuelve a Explorer |

## Diseño orientado al mínimo consumo

- **`windows-sys`** (FFI cruda) en lugar del crate `windows`: menos código generado y
  binario más pequeño.
- **Perfil release agresivo**: `opt-level = "z"`, LTO, `codegen-units = 1`,
  `panic = "abort"`, símbolos quitados y CRT estática (`+crt-static`, sin VC++ redist).
- **Sin sondeos agresivos**: la barra de tareas y el administrador se refrescan por
  temporizadores espaciados; el administrador usa **un único barrido** de
  `NtQuerySystemInformation` (sin abrir un handle por proceso).
- **Dibujo propio con GDI+** y doble buffer (sin parpadeo), con recursos (fuentes,
  formatos) cacheados por hilo.
- **EcoQoS** y `EmptyWorkingSet` para reducir el consumo del propio proceso.

## Compilación

Requiere la toolchain de Rust con el target `x86_64-pc-windows-msvc`:

```powershell
cd RustShell
.\build.ps1          # o: cargo build --release
```

El binario queda en `RustShell\target\x86_64-pc-windows-msvc\release\OptimizedShell.exe`.

## Estado

Port en curso desde la versión original en C++ (sustituida por esta implementación en
Rust). Ya funcionan, con interfaz equivalente a la original:

- **Administrador de tareas**: marco propio (barra de título, min/max/cerrar), pestañas,
  tabla de procesos con %CPU por delta y colores por umbral, filtrado, fin de proceso y
  pestaña Inicio (habilitar/deshabilitar con las claves StartupApproved de Windows).
- **Configuración**: página de Personalización completa (modo claro/oscuro, acentos,
  desenfoque y opacidades) que guarda en `%APPDATA%\OptimizedShell\theme.ini` y se aplica
  al momento.
- **Explorador**: navegación (atrás/adelante/arriba), panel lateral de accesos, columnas
  Nombre/Fecha/Tipo/Tamaño ordenables y apertura de archivos.
- **Bloqueo**: reloj, tarjeta de usuario y autenticación con `LogonUserW`.
- **Shell**: barra de tareas con botón Inicio, botones de ventanas abiertas y reloj;
  `--install`/`--uninstall` sobre el valor `Shell` de Winlogon.

### Pendiente

- Task Manager: gráficas de Rendimiento (PDH), pestaña Servicios, iconos por fila,
  EcoQoS/prioridad/afinidad por proceso.
- Configuración: páginas Fondo (galería de miniaturas), Pantalla (DisplayConfig/WMI) y
  Bluetooth (WinRT).
- Explorador: operaciones COM (`IFileOperation`), portapapeles, arrastrar y soltar,
  refresco por `ReadDirectoryChangesW`, menú contextual del shell.
- Shell: escritorio con fondo, menú Inicio con búsqueda de apps, bandeja del sistema,
  flyouts de volumen/Wi‑Fi/batería, hotkeys globales.
