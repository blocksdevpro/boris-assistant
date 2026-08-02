//! Build the always-on-top overlay the Tauri-documented way.
//!
//! From [`WebviewWindowBuilder::transparent`]:
//! > If this is true, writing colors with alpha values different than `1.0`
//! > will produce a transparent window.
//!
//! From [`WebviewWindowBuilder::background_color`] (Windows):
//! > On Windows 8 and newer, if alpha channel is not `0`, it will be ignored
//! > (for the webview layer). So we pass alpha **0**.

use tauri::{AppHandle, Runtime, WebviewUrl, WebviewWindowBuilder};

/// Early script so the first paint is clear (before React mounts).
const OVERLAY_INIT: &str = r#"
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
        .initialization_script(OVERLAY_INIT)
        .build()
        .map_err(|e| {
            tracing::error!(error = %e, "overlay WebviewWindowBuilder::build failed");
            e
        })?;

    // Belt-and-suspenders: re-assert webview clear after create.
    let webview: &tauri::Webview<R> = overlay.as_ref();
    if let Err(e) = webview.set_background_color(Some(tauri::window::Color(0, 0, 0, 0))) {
        tracing::warn!(error = %e, "overlay webview clear color");
    } else {
        tracing::info!("overlay window ready (transparent + a=0 background)");
    }

    Ok(())
}
