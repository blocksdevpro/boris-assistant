//! System tray — keep Boris running when the main window is closed.
//!
//! # Responsibility (host shell)
//!
//! Close on the console window hides it (app + overlay keep running).
//! The tray is the way back: left-click or "Open Boris" shows the console;
//! "Quit Boris" hides every window at once, stops the engine (via
//! [`AppState`]) off the UI thread with a grace period, and exits fully.
//!
//! Overlay input lock is delegated to [`crate::overlay_win`] — tray only
//! exposes menu actions. No voice/engine policy lives here.

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Runtime,
};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use boris_pipeline::{EngineState, ListenHint, StatusPicture};

use crate::orchestrator::AppState;
use crate::overlay_win;

const TRAY_ID: &str = "boris-tray";
const TRAY_TOOLTIP: &str = "Boris — voice assistant";
const TRAY_TOOLTIP_MIC: &str = "Boris can't hear your mic";

/// Tooltip currently shows the mic warning (avoids a Win32 call per snapshot).
static TRAY_MIC_WARNING: AtomicBool = AtomicBool::new(false);

/// Hovering the tray says when Boris can't hear the mic, even with every
/// window hidden.
pub fn sync_tooltip<R: Runtime>(app: &AppHandle<R>, status: &StatusPicture) {
    let warn =
        status.engine == EngineState::On && status.listen_hint == Some(ListenHint::MicSilent);
    if TRAY_MIC_WARNING.swap(warn, Ordering::Relaxed) == warn {
        return;
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let text = if warn { TRAY_TOOLTIP_MIC } else { TRAY_TOOLTIP };
        if let Err(e) = tray.set_tooltip(Some(text)) {
            tracing::debug!(error = %e, "tray tooltip");
        }
    }
}

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
        if let Some(state) = app.try_state::<AppState>() {
            // Prefs already cached at boot / save — do not re-read disk here.
            overlay_win::sync_visibility(app, &state.status());
        }
    }
}

/// Bring the main console window back (unminimize + show + focus).
pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        tracing::info!("tray: showing main window");
        let _ = window.unminimize();
        let _ = window.set_skip_taskbar(false);
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        tracing::warn!("main window missing — cannot show from tray");
    }
}

/// Longest "Quit Boris" waits for the engine to stop before exiting anyway.
const QUIT_GRACE: Duration = Duration::from_secs(4);

/// Set once quit starts, so a second click cannot start a second teardown.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Hide every window now, stop the engine off the UI thread, then exit.
///
/// Runs from the tray menu handler, i.e. **on the UI thread**. Stopping joins
/// the engine thread (seconds while audio and models shut down), and status
/// delivery marshals overlay calls to the UI thread, so blocking here froze
/// the whole app. The UI thread only hides windows and returns.
fn quit_app<R: Runtime>(app: &AppHandle<R>) {
    if QUITTING.swap(true, Ordering::AcqRel) {
        return;
    }
    tracing::info!("tray: quit requested");
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.hide();
    }
    overlay_win::hide_if_present(app);

    let worker = app.clone();
    let spawned = thread::Builder::new()
        .name("boris-quit".into())
        .spawn(move || {
            stop_engine_within(&worker, QUIT_GRACE);
            tracing::info!("tray: exiting");
            worker.exit(0);
        });
    if let Err(e) = spawned {
        tracing::error!(error = %e, "quit thread not started — exiting without engine stop");
        app.exit(0);
    }
}

/// Stop the engine, waiting at most `grace`. A wedged engine thread must not
/// keep the process alive: exit tears it down regardless.
fn stop_engine_within<R: Runtime>(app: &AppHandle<R>, grace: Duration) {
    let (done_tx, done_rx) = mpsc::channel();
    let stopper = app.clone();
    let started = thread::Builder::new()
        .name("boris-quit-stop".into())
        .spawn(move || {
            if let Some(state) = stopper.try_state::<AppState>() {
                if let Err(e) = state.stop() {
                    tracing::warn!(error = %e, "stop engine on quit");
                }
            }
            let _ = done_tx.send(());
        });
    if started.is_err() {
        tracing::warn!("engine stop thread not started");
        return;
    }
    if done_rx.recv_timeout(grace).is_err() {
        tracing::warn!(?grace, "engine did not stop in time — exiting anyway");
    }
}
