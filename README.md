# OptimizedShell

Shell y utilidades ligeras para Windows 10/11 escritas en **Rust** sobre Win32 + GDI+
(crate `windows-sys`), sin frameworks de UI ni dependencias pesadas. El objetivo es
sustituir a `explorer.exe` y a las herramientas del sistema por versiones que consuman
mucha menos CPU y RAM.

Un solo ejecutable, `OptimizedShell.exe`, con varios modos:

| Modo | Comando | Qué hace |
|---|---|---|
| Shell | `OptimizedShell.exe` | Barra de tareas acoplada: botón Inicio (logotipo de Windows, a la izquierda o centrado), ventanas abiertas con su icono, área de notificación, reloj, menú Inicio estilo Windows 7 y notificaciones |
| Administrador de tareas | `--taskmgr [rendimiento\|inicio\|servicios]` | Procesos (con icono, prioridad, afinidad y EcoQoS), Rendimiento (CPU, GPU, RAM, temperatura), Inicio y Servicios |
| Explorador de archivos | `--explorer [ruta]` | Navegación, panel lateral, columnas ordenables, operaciones de archivo, portapapeles y actualización automática |
| Bloqueo / inicio de sesión | `--lock` · `--logon [--demo]` | Pantalla de bloqueo propia con autenticación |
| Configuración | `--settings [inicio\|sistema\|bluetooth\|red\|personalizacion\|barra\|menu\|aplicaciones\|accesibilidad\|acerca]` | Configuración del sistema al estilo de Windows 11: buscador, categorías, información del equipo y accesos a las páginas de Windows |
| Notificación | `--notify "Título\|Mensaje" [--ok\|--aviso\|--error] [--app <nombre>]` | Manda un aviso a la barra de tareas en ejecución (para scripts y tareas programadas) |
| Instalación | `--install` · `--uninstall` | Usa OptimizedShell como shell del usuario actual, o vuelve a Explorer |

## Escritorio y atajos

- **Escritorio propio** con el papel tapiz del usuario (el de
  `HKCU\Control Panel\Desktop`, así que se comparte con Windows), con ajuste
  Rellenar/Ajustar/Expandir/Mosaico/Centrar, color de fondo y menú contextual.
  Se omite solo mientras el Explorador de Windows siga siendo el shell, para no
  taparle el escritorio real.
- **Atajos globales**: `Win+E` explorador, `Win+R` ejecutar, `Win+I`
  configuración, `Win+D` mostrar el escritorio, `Win+N` centro de
  notificaciones, `Win+L` bloquear, `Ctrl+Mayús+Esc` administrador de tareas y
  `Ctrl+Esc` menú Inicio. Se pueden desactivar en un solo ajuste.
- **Anclar programas al menú Inicio**: botón derecho sobre cualquier programa
  para anclarlo, abrir su ubicación, copiar su ruta o ejecutarlo como
  administrador. Los anclados salen primero, con su chincheta, y se guardan en
  `%APPDATA%\OptimizedShell\pinned.ini`.

## Barra de tareas

- **Botón Inicio** con el logotipo de Windows extraído de `shell32.dll` (o de
  `imageres.dll`, un glifo o un logotipo dibujado con el color de acento), a la
  izquierda o **centrado junto a los botones de ventana**, como en Windows 11.
- **Botones de ventana** con el icono real de cada aplicación (`WM_GETICON`, y si
  la ventana no declara ninguno, el del ejecutable), con o sin título, subrayado
  de la ventana en primer plano y minimizar al volver a pulsarla.
- **Área de notificación** (bandeja): volumen, red, Bluetooth, batería y la
  campana de avisos, con descripción emergente al pasar el puntero.
  - *Volumen*: nivel del dispositivo predeterminado (Core Audio). La rueda del
    ratón sobre el icono lo sube y lo baja.
  - *Red*: Wi-Fi con su intensidad en barras y el SSID, o Ethernet con su
    velocidad; aspa roja cuando no hay ninguna interfaz operativa.
  - *Batería*: dibujo con el nivel real, rayo al cargar, rojo por debajo del 20 %
    y porcentaje opcional. Se oculta sola en equipos de escritorio.
  - *Bluetooth*: se oculta solo si el equipo no tiene radio.
  - *Iconos de las aplicaciones*: OptimizedShell implementa el protocolo de
    `Shell_NotifyIcon` (ventana `Shell_TrayWnd` y `WM_COPYDATA`), así que los
    programas ponen ahí su icono y reciben sus clics. Solo llegan cuando
    OptimizedShell es el shell: si el Explorador está en marcha, es su barra la
    que los recoge.
- **Configuración rápida**: al pulsar cualquier icono se abre un panel al estilo
  de Windows 11 con mosaicos (Wi-Fi, Bluetooth, No molestar, Tema claro/oscuro),
  deslizador de volumen con silencio, estado de la red y de la batería, y
  accesos a Configuración y al Administrador de tareas. El botón derecho sobre
  cada icono abre directamente la página de Windows correspondiente.
  - Los mosaicos de **Wi-Fi y Bluetooth encienden y apagan la radio de verdad**:
    el Wi-Fi con `WlanSetInterface` y el Bluetooth con la API Radio de WinRT.
    Si el equipo no tiene esa radio, el mosaico abre la página de Windows.

## Notificaciones

- **Avisos emergentes** apilados sobre la barra, con icono por tipo (información,
  correcto, aviso, error), barra de tiempo restante y cierre automático; el aviso
  bajo el puntero no caduca, para dar tiempo a leerlo.
- **Centro de notificaciones**: historial con hora, borrado individual o
  completo, «No molestar» y acceso a sus ajustes.
- **Avisos propios del sistema**: batería baja y crítica, conexión y desconexión
  del cargador, y caída o vuelta de la red.
- **Desde fuera**: `OptimizedShell.exe --notify "Título|Mensaje"` se entrega a la
  barra por `WM_COPYDATA`, así que cualquier script puede avisar al usuario.
- Configurable: activarlas, No molestar, sonido, tiempo en pantalla, cuántas se
  ven a la vez, ancho, esquina y tamaño del historial.

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
    ancho de botón, títulos, iconos de las aplicaciones y su tamaño, botón
    Inicio (centrado o no, y de qué DLL sale su icono), reloj (12/24 h,
    segundos, fecha), reserva del área de trabajo, siempre visible e intervalo
    de refresco.
  - *Área de notificación*: mostrarla o no y qué iconos incluye (volumen, red,
    batería con porcentaje, Bluetooth, campana) y el separador antes del reloj.
  - *Notificaciones*: activarlas, No molestar, sonido, tiempo en pantalla,
    avisos visibles a la vez, ancho, esquina e historial guardado.
  - *Menú Inicio*: ancho de la lista, alto del menú y de cada elemento, columna
    derecha, cuadro de búsqueda, cuántos programas frecuentes se muestran,
    iconos de los programas y qué entradas incluye.
  - *Listas y explorador*: alto de fila, panel lateral, archivos ocultos,
    carpetas primero, columnas Fecha/Tipo/Tamaño, refresco del administrador y,
    para la pestaña Rendimiento, gráficas por núcleo y segundos de historial.
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
- **Rendimiento** (`--taskmgr rendimiento`): una tarjeta con mini gráfica por
  dispositivo y, a la derecha, la gráfica grande y los datos del seleccionado.
  - *CPU*: uso total y por núcleo (`NtQuerySystemInformation` clase 8), nombre y
    velocidad base del procesador, núcleos lógicos/físicos, procesos e hilos.
  - *Memoria*: en uso, disponible, total y confirmación (`GlobalMemoryStatusEx`).
  - *GPUs*: uso y memoria dedicada por adaptador, con los contadores PDH
    `GPU Engine` / `GPU Adapter Memory` agrupados por LUID y el nombre y la VRAM
    que da DXGI (`EnumAdapters1`).
  - *Temperatura*: zonas térmicas ACPI (`Thermal Zone Information`) en °C.
  - El muestreador solo se abre mientras la pestaña está a la vista, y si una
    fuente no existe (sin contadores de GPU, sin sensores ACPI) se dice por qué
    en vez de enseñar un cero.
- **Servicios** (`--taskmgr servicios`): todos los servicios del equipo con su
  identificador, PID, estado y tipo de inicio, en una sola consulta al SCM.
  Iniciar, detener y reiniciar, y cambiar el tipo de inicio desde su menú
  contextual; si falta elevación se dice en vez de fallar en silencio.
- **Acciones por proceso** (botón derecho en Procesos): finalizar, abrir la
  ubicación del ejecutable, copiar su ruta, cambiar la prioridad, activar el
  modo de eficiencia (EcoQoS) y elegir la afinidad de procesadores con una
  casilla por núcleo. Cada fila lleva además el icono de su ejecutable.
- **Configuración del sistema al estilo de Windows 11**: barra lateral con
  buscador que filtra **todos** los ajustes a la vez, página de inicio con la
  tarjeta del equipo (usuario, nombre del equipo y edición de Windows) y
  mosaicos de categoría, y páginas formadas por grupos de tarjetas: Sistema,
  Bluetooth y dispositivos, Red e Internet, Personalización, Barra de tareas,
  Menú Inicio, Aplicaciones, Accesibilidad y Acerca de. Junto a los ajustes
  propios hay filas de información en vivo (procesador, RAM, conexión actual,
  batería, Bluetooth, salida de audio) y accesos a las páginas de Windows
  (`ms-settings:`) que OptimizedShell no reimplementa. Todo se guarda en
  `config.ini` y se aplica en caliente.
  - *Fondo de escritorio*: galería con las miniaturas de las imágenes de
    Windows y de las del usuario (las genera GDI+ solo para las que se ven),
    botón «Examinar...» y ajuste de encaje.
  - *Pantalla*: monitores detectados con `EnumDisplayDevices`, resoluciones y
    frecuencias que admite cada uno y orientación. El modo se prueba antes de
    aplicarlo (`CDS_TEST`), así que un modo que el monitor no soporte no deja
    la pantalla en negro.
- **Explorador**: navegación (atrás/adelante/arriba), panel lateral de accesos,
  columnas Nombre/Fecha/Tipo/Tamaño ordenables, iconos reales del shell y
  selección múltiple (Ctrl, Mayús, Ctrl+A).
  - *Operaciones de archivo* con `IFileOperation`, igual que el Explorador de
    Windows: papelera de reciclaje, diálogo de progreso, confirmaciones,
    elevación cuando hace falta y deshacer con Ctrl+Z. Copiar, cortar, pegar
    (Ctrl+C/X/V), eliminar (Supr, o Mayús+Supr para saltarse la papelera),
    cambiar nombre en la propia fila (F2) y carpeta nueva (Ctrl+N). Pegar en la
    carpeta de origen duplica con « - copia», como Windows.
  - *Portapapeles* en formato CF_HDROP: lo copiado aquí se pega en el
    Explorador de Windows y al revés.
  - *Arrastrar y soltar* archivos sobre la ventana (sobre una carpeta los mete
    dentro; con Mayús mueve en vez de copiar).
  - *Actualización automática* con `ReadDirectoryChangesW` en un hilo aparte:
    lo que cambie en la carpeta aparece solo, sin perder la selección.
  - *Menú contextual propio* con el tema de la aplicación y, dentro de él,
    acceso al menú del shell (`IContextMenu`) con «Abrir con», «Enviar a» y lo
    que añadan los programas instalados.
- **Bloqueo**: reloj, tarjeta de usuario y autenticación con `LogonUserW`.
- **Shell**: barra de tareas con botón Inicio, botones de ventanas abiertas con
  su icono, área de notificación, configuración rápida, notificaciones y reloj;
  `--install`/`--uninstall` sobre el valor `Shell` de Winlogon.
- **Menú Inicio estilo Windows 7**: a la izquierda los programas más usados y el
  botón «Todos los programas» (lista completa con desplazamiento), abajo un
  cuadro de búsqueda que filtra mientras se escribe (Esc cierra, Entrar abre el
  seleccionado, flechas y AvPág/RePág mueven); a la derecha el usuario, las
  carpetas personales, Equipo, Panel de control, las herramientas propias y los
  botones de bloquear/reiniciar/apagar. Los programas son los accesos directos
  reales de los menús Inicio del usuario y de todos los usuarios, con su icono
  del shell y un contador de uso en `%APPDATA%\OptimizedShell\recent.ini`.

### Pendiente

- Explorador: arrastrar archivos *hacia fuera* de la ventana (hace falta
  implementar `IDataObject` y `IDropSource`; soltar dentro ya funciona), vista
  de iconos grandes y árbol de carpetas en el panel lateral.
- Escritorio: iconos del escritorio (requiere la vista del shell o una lista
  propia sobre `%USERPROFILE%\Desktop`).
- Barra de tareas: agrupar varias ventanas de un mismo programa en un botón y
  listas de accesos rápidos (Jump Lists).
- Administrador de tareas: columna de E/S real por proceso (ahora es 0) y
  árbol de procesos padre/hijo.
- Varios monitores: la barra y el escritorio solo se dibujan en el principal.
