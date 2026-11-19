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

- **Administrador de tareas**: marco propio (barra de título, min/max/cerrar) y las cuatro
  pestañas: **Procesos** (tabla con %CPU por delta, colores por umbral, filtrado, fin de
  proceso), **Rendimiento** (gráficas de CPU y memoria muestreadas), **Inicio**
  (habilitar/deshabilitar con las claves StartupApproved de Windows) y **Servicios**
  (listado del SCM con estado y PID). Pestaña inicial por `--tab`.
- **Configuración**: **Personalización** completa (modo, acentos, desenfoque, opacidades,
  guardado en `theme.ini` y aplicado al momento), **Pantalla** (enumerar y aplicar
  resolución/frecuencia) y **Fondo de escritorio** (elegir imagen de Windows\Web o
  Imágenes y aplicarla).
- **Explorador**: navegación (atrás/adelante/arriba), panel lateral de accesos, columnas
  ordenables, apertura de archivos, **nueva carpeta** y **eliminar** a la papelera.
- **Bloqueo**: reloj, tarjeta de usuario y autenticación con `LogonUserW`.
- **Shell**: barra de tareas con botón Inicio y menú, botones de ventanas abiertas y
  reloj; **fondo de escritorio**, **hotkeys globales** (Win+E, Win+R, Ctrl+Shift+Esc);
  `--install`/`--uninstall` sobre el valor `Shell` de Winlogon.

### Pendiente

- Task Manager: iconos por fila, menú contextual por proceso (EcoQoS/prioridad/afinidad).
- Configuración: página Bluetooth (WinRT).
- Explorador: portapapeles y arrastrar y soltar (`IFileOperation`), refresco por
  `ReadDirectoryChangesW`, menú contextual del shell.
- Shell: menú Inicio con búsqueda de apps, bandeja del sistema y flyouts de
  volumen/Wi‑Fi/batería.
