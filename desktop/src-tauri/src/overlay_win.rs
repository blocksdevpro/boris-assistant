//! Always-on-top transparent overlay window (the “island”).
//!
//! # Responsibility (host shell)
//!
//! Create and position a click-through webview. React paints phase UI inside;
//! this module only owns **window chrome** (transparency, ignore-cursor, park).
//!
//! # Tauri notes
//!
//! From [`WebviewWindowBuilder::transparent`]:
//! > If this is true, writing colors with alpha values different than `1.0`
//! > will produce a transparent window.
//!
//! From [`WebviewWindowBuilder::background_color`] (Windows):
//! > On Windows 8 and newer, if alpha channel is not `0`, it will be ignored
//! > (for the webview layer). So we pass alpha **0**.
//!
//! ## Input
//!
//! The overlay is **click-through by default** (`set_ignore_cursor_events(true)`).
//! Without that, an always-on-top webview steals mouse input from games (Valorant
//! aim clicks land on the island and `data-tauri-drag-region` starts a drag).
//! Tray / host can temporarily unlock input so the user can reposition.

use tauri::{AppHandle, Manager, PhysicalPosition, Runtime, WebviewUrl, WebviewWindowBuilder};

/// Fraction of monitor height from the top edge where the island is parked.
const OVERLAY_TOP_MARGIN_FRAC: f64 = 0.02;

/// Early script so the first paint is clear (before React mounts).
const OVERLAY_INIT_SCRIPT: &str = r#"
(function () {
  try {
    document.documentElement.classList.add('overlay-mode');
    document.documentElement.classList.remove('dark');
    document.documentElement.style.setProperty('background', 'transparent', 'important');
    document.documentElement.style.setProperty('background-color', 'transparent', 'important');
    var meta = document.querySelector('meta[name="color-scheme"]');
    if (meta) meta.setAttribute('content', 'only light');
    document.addEventListener('DOMContentLoaded', function () {
      if (document.body) {
        document.body.style.setProperty('background', 'transparent', 'important');
        document.body.style.setProperty('background-color', 'transparent', 'important');
      }
      var root = document.getElementById('root');
      if (root) {
        root.style.setProperty('background', 'transparent', 'important');
        root.style.setProperty('background-color', 'transparent', 'important');
      }
    });
  } catch (e) {}
})();
"#;

/// Create the overlay window if it was declared with `"create": false` in config.
pub fn spawn_overlay_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    tracing::info!("building overlay window");
    // Prefer config entry (size, alwaysOnTop, etc.) then force transparency bits.
    let template = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "overlay")
        .cloned();

    let builder = if let Some(conf) = template {
        tracing::debug!("overlay: using tauri.conf window template");
        WebviewWindowBuilder::from_config(app, &conf)?
    } else {
        tracing::warn!("overlay: no config entry — using hardcoded defaults");
        WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("index.html".into()))
            .title("Boris")
            .inner_size(380.0, 120.0)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .visible(true)
    };

    let overlay = builder
        .transparent(true)
        .shadow(false)
        .focused(false)
        // Webview + window bg: alpha 0 is required on Win8+ for clear pixels.
        .background_color(tauri::window::Color(0, 0, 0, 0))
        .initialization_script(OVERLAY_INIT_SCRIPT)
        .build()
        .map_err(|e| {
            tracing::error!(error = %e, "overlay WebviewWindowBuilder::build failed");
            e
        })?;

    // Belt-and-suspenders: re-assert webview clear after create.
    let webview: &tauri::Webview<R> = overlay.as_ref();
    if let Err(e) = webview.set_background_color(Some(tauri::window::Color(0, 0, 0, 0))) {
        tracing::warn!(error = %e, "overlay webview clear color");
    }

    // Critical for gaming: never steal mouse from the focused game.
    if let Err(e) = overlay.set_ignore_cursor_events(true) {
        tracing::error!(error = %e, "overlay set_ignore_cursor_events(true) failed");
    } else {
        tracing::info!("overlay click-through enabled (ignore cursor events)");
    }

    place_overlay_top_center(&overlay);

    tracing::info!("overlay window ready (transparent + a=0 + click-through)");
    Ok(())
}

/// Park the island near the top-center of the primary (or current) monitor
/// so it sits away from typical FPS crosshair / aim areas.
fn place_overlay_top_center<R: Runtime>(overlay: &tauri::WebviewWindow<R>) {
    let monitor = match overlay.current_monitor() {
        Ok(Some(m)) => m,
        _ => match overlay.primary_monitor() {
            Ok(Some(m)) => m,
            _ => {
                tracing::debug!("overlay: no monitor info — leaving default position");
                return;
            }
        },
    };

    let screen = monitor.size();
    let pos = monitor.position();
    let Ok(win_size) = overlay.outer_size() else {
        return;
    };

    let x = pos.x + (screen.width as i32 - win_size.width as i32) / 2;
    let y = pos.y + (screen.height as f64 * OVERLAY_TOP_MARGIN_FRAC) as i32;

    if let Err(e) = overlay.set_position(tauri::Position::Physical(PhysicalPosition::new(x, y))) {
        tracing::warn!(error = %e, "overlay set_position failed");
    } else {
        tracing::info!(x, y, "overlay parked top-center");
    }
}

/// When `locked` is true, mouse passes through to the game (default).
/// When false, the user can click/drag the island to reposition it.
pub fn set_overlay_input_locked<R: Runtime>(app: &AppHandle<R>, locked: bool) -> tauri::Result<()> {
    let Some(overlay) = app.get_webview_window("overlay") else {
        tracing::warn!("set_overlay_input_locked: overlay window missing");
        return Ok(());
    };
    overlay.set_ignore_cursor_events(locked)?;
    tracing::info!(locked, "overlay input lock updated");
    Ok(())
}
