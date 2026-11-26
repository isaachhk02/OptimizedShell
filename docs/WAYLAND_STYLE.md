# Estilo visual de OptimizedShell exportado a Wayland/Linux

Este documento lleva el lenguaje visual de OptimizedShell (v0.8.3, Win32 +
GDI+) a un contexto de Wayland y Linux. Los valores son los que el código
produce en `src/ui/theme.rs` (`ui::theme::compute`) con la configuración por
defecto de `src/ui/config.rs`:

- **Modo:** Oscuro (predeterminado, `Modo = Oscuro`).
- **Color de acento:** "Azul" (`Acento = 0x3B82F6`).
- **Opacidad de la barra:** 60 % (`OpacidadBarra`), **de las ventanas:** 96 %
  (`OpacidadVentanas`).
- **Radio de esquinas:** 10 px (`Radio`).
- **Desenfoque (acrílico):** activado (`Desenfoque`).
- **Fuente:** Segoe UI (`Fuente`).

El archivo `rustshell-style.css` contiene los mismos tokens como variables
CSS (`--os-*`) para aplicarlos directamente en un shell Wayland: importa ese
CSS, pon el atributo `os-theme-light` o un equivalente al lado de
`os-theme-dark` para el tema claro, y usa los valores donde diga "opcional"
(altura de barra, iconos, radio...).

## Paleta oscura (predeterminada)

| Token | Valor | Uso principal |
|---|---|---|
| `--os-window-bg` | `#11141B` | fondo de las ventanas (opac. 96 %) |
| `--os-title-bg` | `#171B24` | barra de título de las ventanas |
| `--os-card-bg` | `#1B1F29` | tarjetas, controles, menús |
| `--os-card-border` | `rgba(255,255,255,0.157)` | borde de tarjetas |
| `--os-text` | `#E8ECF4` | texto principal |
| `--os-text-dim` | `#94A0B2` | texto secundario |
| `--os-text-faint` | `#697488` | textos resaltados |
| `--os-taskbar-bg` | `#101218` | barra de tareas (opac. 60 %) |
| `--os-task-button` | `#1C1F28` | botones de ventana |
| `--os-task-button-hover` | `#262A36` | hover en botones de ventana |
| `--os-task-button-active` | `#2E3342` | ventana en primer plano |
| `--os-menu-bg` | `#1E222C` | menú Inicio |
| `--os-menu-hover` | `#303644` | ítem seleccionado en el menú |
| `--os-flyout-bg` | `#151820` | paneles desplegables |
| `--os-graph-bg` | `#161A23` | áreas de gráficas |
| `--os-row-hover` | `#222733` | filas al pasar el puntero |
| `--os-control-bg` | `#232834` | controles (interruptores, botones) |
| `--os-control-hover` | `#2C3240` | hover en controles |
| `--os-separator` | `rgba(255,255,255,0.118)` | líneas finas |
| `--os-grid-line` | `rgba(255,255,255,0.071)` | rejilla de gráficas |
| `--os-track` | `rgba(255,255,255,0.235)` | barras de progreso |

### Acento

El acento es el único color que deriva de la selección (`Acento`); se
mezcla con blanco en modo oscuro y con negro en modo claro. Con el azul
predeterminado (#3B82F6):

| Token | Valor |
|---|---|
| `--os-accent` | `#5E99F8` |
| `--os-accent-hover` | `#5391F7` |
| `--os-accent-solid` | `#3B82F6` |
| `--os-on-accent` | `#FFFFFF` |
| `--os-row-selected` | `rgba(59, 130, 246, 0.353)` |
| `--os-focus-ring` | `rgba(94, 153, 248, 0.59)` |

### Estado

`--os-danger: #F87171`, `--os-success: #4ADE80`, `--os-warning: #FBBF24`,
`--os-close-hover: #C42B2B`.

## Paleta clara (Modo = Claro)

| Token | Valor | Uso principal |
|---|---|---|
| `--os-window-bg` | `#F7F8FA` | fondo de las ventanas |
| `--os-title-bg` | `#EDEFF3` | barra de título |
| `--os-card-bg` | `#FFFFFF` | tarjetas, menús |
| `--os-text` | `#1A1E26` | texto principal |
| `--os-text-dim` | `#4E5868` | texto secundario |
| `--os-text-faint` | `#7C8492` | textos resaltados |
| `--os-taskbar-bg` | `#EEF0F4` | barra de tareas |
| `--os-task-button` | `#E2E5EB` | botones de ventana |
| `--os-task-button-hover` | `#D6DBE3` | hover |
| `--os-task-button-active` | `#FAFBFC` | ventana activa |
| `--os-menu-bg` | `#FCFCFD` | menú Inicio |
| `--os-menu-hover` | `#E5E9F0` | ítem seleccionado |
| `--os-danger` | `#DC2626` | estado de error |
| `--os-success` | `#16A34A` | estado correcto |

Acento clara: `--os-accent: #3472D8`, `--os-accent-hover: #3575DD`,
`--os-on-accent: #FFFFFF`, `--os-row-selected: rgba(59, 130, 246, 0.235)`,
`--os-focus-ring: rgba(52, 114, 216, 0.59)`.

## Paleta de acentos (predeterminado, índice 0 = Azul)

`Azul #3B82F6` · `Cielo #0EA5E9` · `Turquesa #14B8A6` · `Verde #22C55E` ·
`Lima #84CC16` · `Ámbar #EAB308` · `Naranja #F97316` · `Rojo #EF4444` ·
`Rosa #EC4899` · `Violeta #8B5CF6` · `Pizarra #64748B`.

## Tipografía

- **Familia:** Segoe UI en Windows; en Linux: `Noto Sans` (o `DejaVu Sans`
  como alternativa). La reserva del código es la familia de interfaz
  clásica.
- **Tamaño del texto:** la escala de OptimizedShell (`EscalaTexto`,
  100 %). El reloj de la barra va a ~18 px (la fecha, si se muestra, ~16 px).

## Geometría y dimensiones (predeterminadas)

| Token | Valor | Origen en el código |
|---|---|---|
| `--os-bar-height` | `48px` | `BarraAlto` |
| `--os-icon-size` | `20px` | iconos de bandeja (`TamanoIcono`) |
| `--os-window-btn-width` | `170px` | `AnchoBoton` de los botones de ventana |
| `--os-titlebar-height` | `42px` | `AltoTitulo` |
| `--os-radius` | `10px` | `Radio` de las esquinas |
| `--os-opacity-bar` | `0.60` | `OpacidadBarra` |
| `--os-opacity-windows` | `0.96` | `OpacidadVentanas` |

## Notas para el puerto a Wayland

- **Blur:** el acrílico de Windows (`Desenfoque`) se traduce a
  `backdrop-filter: blur(...)` en CSS o a un compositor que lo soporte
  (`blur` en wlroots); en compositores sin soporte, sustituye por una
  tinta sólida.
- **Esquinas redondeadas:** disponible en prácticamente todos los toolkits
  y en GTK4 con `border-radius`.
- **No hay "barra de tareas" en Wayland:** el concepto se sustituye por un
  panel del compositor o un shell cliente (e.g. de wlroots, iced, eBPF o
  wgpu); estos tokens siguen siendo la base visual del panel.
- **Trazados y rellenos:** los equivalentes de `fill_rect`, `rounded` y
  `text_in` de `ui::gdip` son dibujos con el toolkit de turno; las métricas
  de arriba mantienen el aspecto sin necesidad de un motor GDI+.

## Origen de los valores

- Paletas y acentos: `src/ui/theme.rs` (`compute`: colores dark/light y la
  mezcla del acento).
- Valores por defecto de las variables: `src/ui/config.rs` (`ITEMS`: `def`
  de cada `Item`, `Modo`/`Acento`/`OpacidadBarra`/`OpacidadVentanas`/
  `Radio`/`Fuente`/`AltoTitulo`).
- Tamaño del reloj: `src/shell.rs` (`clock_text`, dibujado en la barra).
- Tamaños de icono y botones: `src/shell.rs` (`ui::s(20)`, `ui::s(18)`).
