//! System tray — keep Boris running when the main window is closed.
//!
//! Close on the console window hides it (app + overlay keep running).
//! The tray is the way back: left-click or "Open Boris" shows the console;
//! "Quit Boris" stops the engine and exits fully.

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Runtime,
};

use crate::orchestrator::AppState;

const TRAY_TOOLTIP: &str = "Boris — voice assistant";

/// Build the tray icon and attach show / quit handlers.
pub fn setup_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Boris", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Boris", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &sep, &quit])?;

    let icon = app.default_window_icon().cloned().ok_or_else(|| {
        tauri::Error::AssetNotFound("default window icon required for tray".into())
    })?;

    let _tray = TrayIconBuilder::with_id("boris-tray")
        .icon(icon)
        .menu(&menu)
        .tooltip(TRAY_TOOLTIP)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main_window(app),
            "quit" => quit_app(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    tracing::info!("system tray ready");
    Ok(())
}

/// Bring the main console window back (unminimize + show + focus).
pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        tracing::info!("tray: showing main window");
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        tracing::warn!("main window missing — cannot show from tray");
    }
}

/// Stop the voice engine if running, then exit the process.
fn quit_app<R: Runtime>(app: &AppHandle<R>) {
    tracing::info!("tray: quit requested");
    if let Some(state) = app.try_state::<AppState>() {
        if let Err(e) = state.stop() {
            tracing::warn!(error = %e, "stop engine on quit");
        }
    }
    app.exit(0);
}
