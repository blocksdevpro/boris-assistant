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

use std::{
    sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicU8, Ordering},
    time::Duration,
};

use boris_pipeline::{AppSettings, EngineState, Phase, StatusPicture};
use serde::Serialize;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, Runtime, WebviewUrl,
    WebviewWindowBuilder,
};

/// Fraction of monitor height from the top edge where the island is parked.
const OVERLAY_TOP_MARGIN_FRAC: f64 = 0.02;

/// Horizontal inset used by the left/right anchor presets.
const OVERLAY_SIDE_MARGIN: i32 = 18;

/// The React stage's logical dimensions. It contains the island itself.
const OVERLAY_STAGE_WIDTH: f64 = 380.0;
const OVERLAY_STAGE_HEIGHT: f64 = 120.0;

/// Clear WebView space around the stage. CSS shadows cannot draw beyond the
/// native window, so this gutter prevents their blurred edges being cropped
/// into a visible rectangular slab.
const OVERLAY_SHADOW_GUTTER: f64 = 32.0;

/// Base logical window size, including the transparent shadow gutter.
const OVERLAY_BASE_WIDTH: f64 = OVERLAY_STAGE_WIDTH + OVERLAY_SHADOW_GUTTER * 2.0;
const OVERLAY_BASE_HEIGHT: f64 = OVERLAY_STAGE_HEIGHT + OVERLAY_SHADOW_GUTTER * 2.0;
/// Glance card stage — clipped body, not a document editor.
const OVERLAY_CARD_STAGE_WIDTH: f64 = 400.0;
const OVERLAY_CARD_STAGE_HEIGHT: f64 = 300.0;
const OVERLAY_CARD_BASE_WIDTH: f64 = OVERLAY_CARD_STAGE_WIDTH + OVERLAY_SHADOW_GUTTER * 2.0;
const OVERLAY_CARD_BASE_HEIGHT: f64 = OVERLAY_CARD_STAGE_HEIGHT + OVERLAY_SHADOW_GUTTER * 2.0;
const OVERLAY_MAX_SCALE: f64 = 1.25;
const OVERLAY_MAX_WIDTH: f64 = OVERLAY_CARD_BASE_WIDTH * OVERLAY_MAX_SCALE;
const OVERLAY_MAX_HEIGHT: f64 = OVERLAY_CARD_BASE_HEIGHT * OVERLAY_MAX_SCALE;

/// Keep the final answer around long enough to read, then get out of the game.
const READY_LINGER: Duration = Duration::from_millis(6_500);
const CARD_LINGER: Duration = Duration::from_millis(15_000);
const FAULT_LINGER: Duration = Duration::from_millis(8_000);

/// Cancels stale delayed hides when another wake/status arrives.
static VISIBILITY_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Cached `show_overlay_on_wake` so the status mirror never re-reads
/// `config.toml` / `auth.json` on every engine snapshot (that path ran on the
/// hot status thread and could lag the UI under high update rates).
///
/// Updated by [`remember_overlay_prefs`] / [`apply_preferences`]. Default
/// matches [`AppSettings`] (off until prefs are loaded).
static SHOW_OVERLAY_ON_WAKE: AtomicBool = AtomicBool::new(false);

/// Set when the UI persists prefs so a deferred boot `load_settings` cannot
/// apply stale overlay geometry over a newer save.
static OVERLAY_PREFS_DIRTY: AtomicBool = AtomicBool::new(false);

/// Mark overlay prefs as newer than whatever is still being loaded at boot.
pub fn mark_overlay_prefs_dirty() {
    OVERLAY_PREFS_DIRTY.store(true, Ordering::Release);
}

/// True after a save (or other live prefs write) beat the deferred boot load.
pub fn overlay_prefs_dirty() -> bool {
    OVERLAY_PREFS_DIRTY.load(Ordering::Acquire)
}

/// Last layout the host applied so prefs/scale changes keep a live card sized.
static OVERLAY_CARD_LAYOUT: AtomicBool = AtomicBool::new(false);
static OVERLAY_SCALE_PERCENT: AtomicU16 = AtomicU16::new(100);
/// 0 = top_center, 1 = top_left, 2 = top_right
static OVERLAY_POSITION: AtomicU8 = AtomicU8::new(0);

pub const EVENT_OVERLAY_PREFERENCES: &str = "overlay-preferences";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayPreferences {
    caption_mode: String,
    scale_percent: u16,
}

impl From<&AppSettings> for OverlayPreferences {
    fn from(settings: &AppSettings) -> Self {
        Self {
            caption_mode: settings.overlay_caption_mode.clone(),
            scale_percent: settings.overlay_scale_percent.clamp(75, 125),
        }
    }
}

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
            .inner_size(OVERLAY_BASE_WIDTH, OVERLAY_BASE_HEIGHT)
            .max_inner_size(OVERLAY_MAX_WIDTH, OVERLAY_MAX_HEIGHT)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .visible(false)
    };

    let overlay = builder
        // Override older config templates whose maximum omitted the shadow
        // gutter at the 125% accessibility scale.
        .max_inner_size(OVERLAY_MAX_WIDTH, OVERLAY_MAX_HEIGHT)
        .visible(false)
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

    tracing::info!("overlay window ready hidden (transparent + a=0 + click-through)");
    Ok(())
}

/// Cache overlay-related prefs for the hot status path.
///
/// Call whenever settings are loaded or saved so [`sync_visibility`] stays
/// correct without disk I/O.
pub fn remember_overlay_prefs(settings: &AppSettings) {
    SHOW_OVERLAY_ON_WAKE.store(settings.show_overlay_on_wake, Ordering::Relaxed);
    OVERLAY_SCALE_PERCENT.store(
        settings.overlay_scale_percent.clamp(75, 125),
        Ordering::Relaxed,
    );
    let pos = match settings.overlay_position.as_str() {
        "top_left" => 1,
        "top_right" => 2,
        _ => 0,
    };
    OVERLAY_POSITION.store(pos, Ordering::Relaxed);
}

/// Cache prefs, notify the React surface, and resize only if the island is up.
///
/// `set_size` / `set_position` on a **hidden** WebView2 window flashes a blank
/// always-on-top pane for a frame on Windows — that is the empty window that
/// popped on every Settings save. Hidden overlays just keep the cache; the
/// next [`sync_visibility`] show applies geometry.
pub fn apply_preferences<R: Runtime>(app: &AppHandle<R>, settings: &AppSettings) {
    remember_overlay_prefs(settings);

    let _ = app.emit(
        EVENT_OVERLAY_PREFERENCES,
        OverlayPreferences::from(settings),
    );

    let Some(overlay) = app.get_webview_window("overlay") else {
        return;
    };
    if !overlay.is_visible().unwrap_or(false) {
        return;
    }

    let scale = f64::from(OVERLAY_SCALE_PERCENT.load(Ordering::Relaxed)) / 100.0;
    apply_overlay_size(&overlay, scale, OVERLAY_CARD_LAYOUT.load(Ordering::Relaxed));
    place_overlay(&overlay, cached_position());
}

/// Show only during a wake/turn. Ready captions linger briefly; Off and a
/// disabled preference hide immediately. Delayed hides are generation-guarded
/// so a new wake cannot be hidden by an older timer.
///
/// Uses the cached wake-overlay preference (see [`remember_overlay_prefs`]) —
/// do not re-load settings here; status updates can fire many times per second.
pub fn sync_visibility<R: Runtime>(app: &AppHandle<R>, status: &StatusPicture) {
    let Some(overlay) = app.get_webview_window("overlay") else {
        return;
    };
    let epoch = VISIBILITY_EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    let show_on_wake = SHOW_OVERLAY_ON_WAKE.load(Ordering::Relaxed);

    if !show_on_wake || status.engine == EngineState::Off {
        let _ = overlay.hide();
        return;
    }

    if status.engine == EngineState::Fault {
        show_without_focus(&overlay);
        schedule_hide(app.clone(), epoch, FAULT_LINGER);
        return;
    }

    if status.phase == Phase::Off {
        let _ = overlay.hide();
        return;
    }

    let card = wants_card_layout(status);
    let layout_changed = OVERLAY_CARD_LAYOUT.swap(card, Ordering::Relaxed) != card;
    let scale = f64::from(OVERLAY_SCALE_PERCENT.load(Ordering::Relaxed)) / 100.0;

    let active = matches!(
        status.phase,
        Phase::Hearing
            | Phase::Reading
            | Phase::Thinking
            | Phase::Talking
            | Phase::AwaitingReply
            | Phase::AwaitingConfirm
    );

    if active {
        apply_overlay_size(&overlay, scale, card);
        if layout_changed {
            place_overlay(&overlay, cached_position());
        }
        show_without_focus(&overlay);
        return;
    }

    // Armed/Quiet after a completed turn: do not resurrect an already-hidden
    // overlay at startup, but let the current response remain readable.
    // Skip set_size on a hidden window — that flashes a blank pane on Windows.
    if overlay.is_visible().unwrap_or(false) {
        apply_overlay_size(&overlay, scale, card);
        if layout_changed {
            place_overlay(&overlay, cached_position());
        }
        let linger = if status.artifact.is_some() {
            CARD_LINGER
        } else {
            READY_LINGER
        };
        schedule_hide(app.clone(), epoch, linger);
    }
}

fn wants_card_layout(status: &StatusPicture) -> bool {
    status.artifact.is_some()
        && status.engine == EngineState::On
        && !matches!(status.phase, Phase::Hearing | Phase::Reading | Phase::Off)
}

fn apply_overlay_size<R: Runtime>(overlay: &tauri::WebviewWindow<R>, scale: f64, card: bool) {
    let (w, h) = if card {
        (
            OVERLAY_CARD_BASE_WIDTH * scale,
            OVERLAY_CARD_BASE_HEIGHT * scale,
        )
    } else {
        (OVERLAY_BASE_WIDTH * scale, OVERLAY_BASE_HEIGHT * scale)
    };
    let _ = overlay.set_max_size(Some(tauri::Size::Logical(LogicalSize::new(
        OVERLAY_MAX_WIDTH,
        OVERLAY_MAX_HEIGHT,
    ))));
    if let Err(e) = overlay.set_size(tauri::Size::Logical(LogicalSize::new(w, h))) {
        tracing::warn!(error = %e, card, "overlay resize failed");
    }
}

fn cached_position() -> &'static str {
    match OVERLAY_POSITION.load(Ordering::Relaxed) {
        1 => "top_left",
        2 => "top_right",
        _ => "top_center",
    }
}

fn show_without_focus<R: Runtime>(overlay: &tauri::WebviewWindow<R>) {
    if let Err(e) = overlay.show() {
        tracing::warn!(error = %e, "overlay show failed");
    }
    // Showing a window must never turn it into a game input target.
    if let Err(e) = overlay.set_ignore_cursor_events(true) {
        tracing::warn!(error = %e, "overlay click-through reassert failed");
    }
}

fn schedule_hide<R: Runtime>(app: AppHandle<R>, epoch: u64, delay: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        if VISIBILITY_EPOCH.load(Ordering::SeqCst) != epoch {
            return;
        }
        if let Some(overlay) = app.get_webview_window("overlay") {
            let _ = overlay.hide();
        }
    });
}

/// Park the island near the requested top edge of the current monitor.
fn place_overlay<R: Runtime>(overlay: &tauri::WebviewWindow<R>, position: &str) {
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

    let screen_right = pos.x + screen.width as i32;
    let x = match position {
        "top_left" => pos.x + OVERLAY_SIDE_MARGIN,
        "top_right" => screen_right - win_size.width as i32 - OVERLAY_SIDE_MARGIN,
        _ => pos.x + (screen.width as i32 - win_size.width as i32) / 2,
    };
    let y = pos.y + (screen.height as f64 * OVERLAY_TOP_MARGIN_FRAC) as i32;

    if let Err(e) = overlay.set_position(tauri::Position::Physical(PhysicalPosition::new(x, y))) {
        tracing::warn!(error = %e, "overlay set_position failed");
    } else {
        tracing::info!(x, y, position, "overlay parked");
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
    if !locked {
        // Unlocking is an explicit request to position the overlay. Make the
        // otherwise wake-only window visible without focusing it.
        overlay.show()?;
    }
    tracing::info!(locked, "overlay input lock updated");
    Ok(())
}
