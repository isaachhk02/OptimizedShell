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
| Configuración | `--settings [personalizacion\|barra\|menu\|listas\|rendimiento\|fondo\|pantalla\|bluetooth]` | Toda la interfaz: tema, barra de tareas, menú Inicio, listas y rendimiento |
| Instalación | `--install` · `--uninstall` | Usa OptimizedShell como shell del usuario actual, o vuelve a Explorer |

## Interfaz configurable

Todos los ajustes viven en una única tabla declarativa (`src/ui/config.rs`): clave
de INI, etiqueta, ayuda, tipo (interruptor, entero con rango y paso, lista de
opciones, tarjetas o color) y valor por defecto. La ventana de Configuración se
dibuja recorriendo esa tabla, así que **añadir un ajuste es añadir una fila**.

- **Persistencia**: `%APPDATA%\OptimizedShell\config.ini`, agrupado por secciones
  y editable a mano (el antiguo `theme.ini` se importa la primera vez).
- **En caliente**: cada cambio se aplica al momento y se difunde al resto de
  ventanas de OptimizedShell (barra de tareas, explorador y administrador se
  reajustan sin reiniciar).
- **Qué se puede cambiar**:
  - *Personalización*: modo claro/oscuro, color de acento (paleta, R/G/B o el de
    Windows), desenfoque, tinte del fondo, contraste del texto secundario,
    opacidad de barra y ventanas, radio de las esquinas, escala de la interfaz,
    tamaño del texto, familia tipográfica y alto/ancho de la barra de título.
  - *Barra de tareas*: alto, posición (arriba/abajo), alineación de los botones,
    ancho de botón, títulos, botón Inicio, reloj (12/24 h, segundos, fecha),
    reserva del área de trabajo, siempre visible e intervalo de refresco.
  - *Menú Inicio*: ancho, alto de elemento, iconos y qué entradas incluye.
  - *Listas y explorador*: alto de fila, panel lateral, archivos ocultos,
    carpetas primero, columnas Fecha/Tipo/Tamaño y refresco del administrador.
  - *Rendimiento*: EcoQoS y liberación de memoria.
- **Atajos**: rueda del ratón para desplazar, flechas ←/→ sobre un control para
  ajustarlo, y botones «Restablecer esta página», «Restablecer todo» y «Abrir
  config.ini». El botón derecho sobre la barra de tareas abre su página.

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
- **Configuración**: cinco páginas generadas desde la tabla de ajustes
  (Personalización, Barra de tareas, Menú Inicio, Listas y explorador,
  Rendimiento), con guardado en `config.ini` y aplicación en caliente.
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
