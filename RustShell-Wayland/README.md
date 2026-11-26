# RustShell-Wayland

Puerto del shell de escritorio **OptimizedShell** a **Wayland** (Linux),
construido con [iced](https://iced.rs) 0.14 y
[iced_layershell](https://crates.io/crates/iced_layershell) (wlroots
layer-shell). El lenguaje visual es el del proyecto Windows, como se
extrajo en [`docs/rustshell-style.css`](../docs/rustshell-style.css) y
[`docs/WAYLAND_STYLE.md`](../docs/WAYLAND_STYLE.md) — paleta oscura
predeterminada, acento "Azul", esquinas de 10 px, reloj, menú Iniciо.

## Estado actual

- [x] **Barra de tareas (shell)**: fondo de la barra, botón Iniciо
  con el acento de OptimizedShell, reloj (24 h) y fecha.
- [x] **Menú Iniciо estilo Windows 7**: campo de búsqueda, programas
  frecuentes, lista completa (desde entradas `.desktop` del sistema),
  columna derecha con "Bloquear", "Reiniciar" y "Apagar".
- [ ] Botones de ventana abiertas (en desarrollo: requiere acceso al
  compositor).
- [ ] Explorador de archivos.
- [ ] Administrador de tareas.
- [ ] Notificaciones y centro de notificaciones.
- [ ] Centro de control rápido (Wi-Fi, volumen...).

## Construir

```sh
cargo build --release    # o: cargo run --release para probarlo
```

## Ejecutar

Requiere una superficie de capa del compositor (wlroots):

```sh
WAYLAND_DISPLAY=… cargo run --release    # en una sesión de Wayland
```

## Estructura

- `src/main.rs` — arranque del shell (`iced_layershell::application`).
- `src/panel.rs` — barra de tareas (`shell.rs` de OptimizedShell).
- `src/start_menu.rs` — menú Iniciо estilo Windows 7.
- `src/apps.rs` — escaneo de entradas `.desktop` (equivalente a los
  accesos del menú Iniciо de OptimizedShell).
- `src/clock.rs` — reloj y fecha (`clock_text`).
- `src/theme.rs` — fichas visuales, portadas de `ui::theme` + `config.rs`.
- `vendor/iced_exdevtools/` — copia de `iced_exdevtools` parcheada: no
  compilaba contra `winit-core 0.31.0-beta.4` (faltaba un `_` en el match
  de `keymap.rs`).

## Notas de desarrollo

- `Message` tiene un `impl TryInto<LayerShellCustomActionWithId>`
  trivial (`Err(self)`): la macro `to_layer_message` de `iced_layershell`
  genera variantes de anchor que no usamos; con el `impl` trivial
  todos los mensajes se tratan como normales.
- Ajuste por escaler: se usa la escala base de 48 px de la barra; añade
  escalabilidad si lo necesitas (e.g. `theme::BAR_HEIGHT * scale`).

## Licencia

GPL-3.0-or-later, como OptimizedShell.
