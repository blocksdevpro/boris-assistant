//! System tray — keep Boris running when the main window is closed.
//!
//! # Responsibility (host shell)
//!
//! Close on the console window hides it (app + overlay keep running).
//! The tray is the way back: left-click or "Open Boris" shows the console;
//! "Quit Boris" stops the engine (via [`AppState`]) and exits fully.
//!
//! Overlay input lock is delegated to [`crate::overlay_win`] — tray only
//! exposes menu actions. No voice/engine policy lives here.

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Runtime,
};

use crate::orchestrator::AppState;
use crate::overlay_win;

const TRAY_ID: &str = "boris-tray";
const TRAY_TOOLTIP: &str = "Boris — voice assistant";

const MENU_OPEN: &str = "open";
const MENU_OVERLAY_UNLOCK: &str = "overlay_unlock";
const MENU_OVERLAY_LOCK: &str = "overlay_lock";
const MENU_QUIT: &str = "quit";

/// Build the tray icon and attach show / quit / overlay-lock handlers.
pub fn setup_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, MENU_OPEN, "Open Boris", true, None::<&str>)?;
    let unlock_overlay = MenuItem::with_id(
        app,
        MENU_OVERLAY_UNLOCK,
        "Unlock overlay to move",
        true,
        None::<&str>,
    )?;
    let lock_overlay = MenuItem::with_id(
        app,
        MENU_OVERLAY_LOCK,
        "Lock overlay (click-through)",
        true,
        None::<&str>,
    )?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit Boris", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&open, &sep, &unlock_overlay, &lock_overlay, &sep, &quit],
    )?;

    let icon = app.default_window_icon().cloned().ok_or_else(|| {
        tauri::Error::AssetNotFound("default window icon required for tray".into())
    })?;

    let _tray = TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .menu(&menu)
        .tooltip(TRAY_TOOLTIP)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            MENU_OPEN => show_main_window(app),
            MENU_OVERLAY_UNLOCK => set_overlay_locked(app, false),
            MENU_OVERLAY_LOCK => set_overlay_locked(app, true),
            MENU_QUIT => quit_app(app),
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

fn set_overlay_locked<R: Runtime>(app: &AppHandle<R>, locked: bool) {
    match overlay_win::set_overlay_input_locked(app, locked) {
        Ok(()) => {
            if locked {
                tracing::info!("tray: overlay locked (click-through) — safe for games");
            } else {
                tracing::info!("tray: overlay unlocked — drag the island, then lock again");
            }
        }
        Err(e) => tracing::warn!(error = %e, locked, "tray: failed to set overlay input lock"),
    }

    if locked {
        if let (Ok(settings), Some(state)) =
            (boris_pipeline::load_settings(), app.try_state::<AppState>())
        {
            overlay_win::sync_visibility(app, &settings, &state.status());
        }
    }
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
