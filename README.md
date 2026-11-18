# OptimizedShell

Shell y utilidades ligeras para Windows 10/11 escritas en C++ (Win32 + GDI+), sin dependencias externas. El objetivo es sustituir a `explorer.exe` y a las herramientas del sistema por versiones que consuman mucha menos CPU y RAM.

Un solo ejecutable, `OptimizedShellCpp.exe` (unos 580 KB), con varios modos:

| Modo | Comando | Qué hace |
|---|---|---|
| Shell | `OptimizedShellCpp.exe` | Escritorio, barra de tareas, menú Inicio, bandeja y flyouts de volumen, Wi‑Fi y batería |
| Administrador de tareas | `--taskmgr [--tab procesos\|rendimiento\|inicio\|servicios]` | Procesos, gráficas de rendimiento, programas de inicio y servicios |
| Explorador de archivos | `--explorer [ruta]` · `--explorer --select <archivo>` | Explorador con navegación, búsqueda y operaciones de archivo |
| Bloqueo / inicio de sesión | `--lock` · `--logon [--demo]` | Pantalla de bloqueo propia |
| Personalización | `--settings` | Modo claro u oscuro, color de acento y transparencia |
| Instalación | `--install` · `--uninstall` | Usa OptimizedShell como shell del usuario actual, o vuelve a Explorer |

## Componentes

### Shell
- **Barra de tareas:** funciona por eventos (shell hook de Windows), sin sondeos periódicos. Los iconos se piden de forma asíncrona, así que una aplicación colgada no la bloquea.
- **Reloj:** solo se despierta una vez por minuto.
- **Menú Inicio:** muestra las aplicaciones de escritorio y de la Store (`shell:AppsFolder`), con búsqueda mientras se escribe, accesos rápidos y menú de apagado.
- **Flyouts:** volumen, Wi‑Fi (escaneo y conexión a redes con perfil) y batería. Solo trabajan mientras están visibles.
- **Escritorio:** el fondo lo pinta el sistema (`PaintDesktop`), sin cargar la imagen en memoria. Los iconos se actualizan solos cuando cambia la carpeta.
- **Modo shell real:** se activa cuando Explorer no está en ejecución. En ese modo OptimizedShell:
  - se registra como shell;
  - reserva el área de trabajo de la barra;
  - aloja los iconos de bandeja de otras aplicaciones;
  - ejecuta los programas de inicio;
  - atiende la tecla Windows, Win+E, Win+R, Win+D y Ctrl+Mayús+Esc;
  - gestiona las teclas de volumen.

### Administrador de tareas
- **Procesos:**
  - Los datos salen de una sola llamada a `NtQuerySystemInformation`: CPU, memoria privada, E/S y hilos.
  - Vista en árbol, búsqueda y orden por columnas.
  - Menú de acciones: finalizar tarea o árbol, prioridad, afinidad, **modo eficiencia** (EcoQoS), liberar la memoria de un proceso y abrir la ubicación del archivo.
- **Rendimiento:** gráficas de 60 s de CPU (total y por núcleo), memoria, disco y red.
  - El botón **Liberar RAM** recorta la memoria en uso de todos los procesos.
- **Inicio:** habilitar o deshabilitar programas de inicio. Usa el mismo formato `StartupApproved` que Windows, así que es compatible con su Administrador de tareas.
- **Servicios:** iniciar, detener y reiniciar servicios, y cambiar su tipo de inicio. Puede reiniciarse como administrador si hace falta.
- **Minimizado:** no tiene timers, pasa a EcoQoS y devuelve su memoria al sistema.

### Explorador de archivos
- Un solo proceso para todas las ventanas.
- Las carpetas se leen en segundo plano y por lotes, con una lista virtualizada: carpetas como `System32` se abren sin congelarse.
- La carpeta abierta se vigila con `ReadDirectoryChangesW` y se actualiza sola.
- Copiar, cortar, pegar, eliminar a la Papelera, renombrar y crear carpetas. Usa `IFileOperation` (diálogos estándar y deshacer) y es compatible con el portapapeles de Explorer.
- Arrastrar y soltar en ambas direcciones, menú contextual nativo bajo demanda ("Más opciones…"), elementos ocultos, "Abrir en Terminal" y atajos de teclado como los de Explorer.

### Tema propio
- **Opciones:** modo **oscuro** o **claro**, 11 colores de acento (o el color de acento de Windows) y transparencia acrílica, que se puede desactivar para ahorrar GPU.
- **Dónde se cambia:** menú Inicio > **Personalización**, o `--settings`.
- **Dónde se guarda:** `%APPDATA%\OptimizedShell\theme.ini`.
- **Aplicación al momento:** los cambios llegan a todas las ventanas abiertas (shell, explorador, Administrador de tareas, pantalla de bloqueo) sin reiniciarlas.
- **Alcance:** los menús contextuales, los cuadros de mensaje, los diálogos, las cajas de texto y los tooltips también usan el tema propio. La única excepción es el menú nativo "Más opciones…" del explorador, porque lo dibujan las extensiones de shell.

## Compilar

Requisitos: Visual Studio 2019 o posterior con las herramientas de C++ (MSVC) y el Windows SDK.

```powershell
powershell -ExecutionPolicy Bypass -File CppShell\build.ps1
```

El ejecutable queda en `CppShell\bin\OptimizedShellCpp.exe`.

## Probar sin instalar

El Administrador de tareas y el explorador se pueden usar directamente, con Explorer abierto:

```powershell
CppShell\bin\OptimizedShellCpp.exe --taskmgr
CppShell\bin\OptimizedShellCpp.exe --explorer C:\
```

Para probar el shell completo sin instalarlo, cierra Explorer y lánzalo a mano:

```powershell
taskkill /f /im explorer.exe
CppShell\bin\OptimizedShellCpp.exe
```

Para volver, usa Inicio > botón de apagado > **Restaurar Explorer**.

## Instalar como shell

```powershell
CppShell\bin\OptimizedShellCpp.exe --install     # se aplica en el próximo inicio de sesión
CppShell\bin\OptimizedShellCpp.exe --uninstall   # vuelve a Explorer
```

La instalación solo afecta al usuario actual: escribe el valor `Shell` en `HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon`.

**Recuperación:** si algo falla, pulsa Ctrl+Alt+Supr > Administrador de tareas > Ejecutar nueva tarea > `explorer.exe`, o ejecuta `OptimizedShellCpp.exe --uninstall`.

## Medir el consumo

```powershell
CppShell\measure.ps1 -Mode taskmgr              # lanza un modo y lo mide 60 s
CppShell\measure.ps1 -Mode taskmgr -Minimize    # medido con la ventana minimizada
CppShell\measure.ps1 -Attach explorer           # mide un proceso existente para comparar
```

Muestra el working set, la memoria privada, la CPU usada, los handles y los objetos GDI/USER.

## Estado

Proyecto en desarrollo.

**Probado y funcionando** (con Explorer en ejecución):
- las cuatro pestañas del Administrador de tareas;
- la navegación del explorador;
- la barra de tareas y el menú Inicio en modo superpuesto.

**Pendiente de probar:**
- el modo shell real: bandeja de otras aplicaciones, teclas globales, programas de inicio, `--install`;
- las operaciones de archivo del explorador.

**Limitaciones conocidas:**
- La barra de tareas y el escritorio solo cubren el monitor principal.
- No hay vista de iconos en el explorador (solo detalles).
- La búsqueda del explorador filtra solo la carpeta actual.
