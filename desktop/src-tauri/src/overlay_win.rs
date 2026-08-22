//! Always-on-top transparent overlay window (the “island”).
//!
//! # Responsibility (host shell)
//!
//! Create and position a click-through webview. React paints phase UI inside;
//! this module only owns **window chrome** (transparency, ignore-cursor, park).
//! First show waits for the overlay frontend to emit [`EVENT_OVERLAY_READY`]
//! so a still-loading WebView2 is never parked on-screen as a rectangle.
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
    sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, AtomicU8, Ordering},
    time::Duration,
};

use boris_pipeline::{AppSettings, EngineState, Phase, StatusPicture};
use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Listener, LogicalSize, Manager, PhysicalPosition, Runtime, WebviewUrl,
    WebviewWindowBuilder,
};

/// Fraction of monitor height from the top edge where the island is parked.
const OVERLAY_TOP_MARGIN_FRAC: f64 = 0.02;

/// Horizontal inset used by the left/right anchor presets.
const OVERLAY_SIDE_MARGIN: i32 = 18;

/// Presence-sized island, used only to park the card-budget HWND so a
/// compact pill sits where the old 120px stage used to.
const OVERLAY_STAGE_HEIGHT: f64 = 120.0;

/// Clear WebView space around the stage. CSS shadows cannot draw beyond the
/// native window, so this gutter prevents their blurred edges being cropped
/// into a visible rectangular slab.
const OVERLAY_SHADOW_GUTTER: f64 = 32.0;

/// Presence window height (stage + gutter). Park math keeps the island
/// center here even though the HWND is always the card budget.
const OVERLAY_BASE_HEIGHT: f64 = OVERLAY_STAGE_HEIGHT + OVERLAY_SHADOW_GUTTER * 2.0;
/// Glance card stage — the stable CSS box the island morphs inside.
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

/// React keeps outgoing content mounted while the island material morphs.
/// The native viewport must not contract underneath that exit animation.
const LAYOUT_CONTRACTION_SETTLE: Duration = Duration::from_millis(360);
const SCALE_CONTRACTION_SETTLE: Duration = Duration::from_millis(460);
const HIDE_EXIT_SETTLE: Duration = Duration::from_millis(220);

/// Cancels stale delayed hides when another wake/status arrives.
static VISIBILITY_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Cancels stale delayed geometry independently of visibility timers.
static GEOMETRY_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Encoded layout/scale target waiting for a contraction timer. Zero is none.
static PENDING_GEOMETRY: AtomicU32 = AtomicU32::new(0);

/// Cached `show_overlay_on_wake` so the status mirror never re-reads
/// `config.toml` / `auth.json` on every engine snapshot (that path ran on the
/// hot status thread and could lag the UI under high update rates).
///
/// Updated by [`remember_overlay_prefs`] / [`apply_preferences`]. Default
/// matches [`AppSettings`] (off until prefs are loaded).
static SHOW_OVERLAY_ON_WAKE: AtomicBool = AtomicBool::new(false);

/// Privacy-hidden captions render presence only; native geometry must match.
static OVERLAY_CONTENT_HIDDEN: AtomicBool = AtomicBool::new(false);

/// Set when the UI persists prefs so a deferred boot `load_settings` cannot
/// apply stale overlay geometry over a newer save.
static OVERLAY_PREFS_DIRTY: AtomicBool = AtomicBool::new(false);

/// First [`apply_preferences`] only seeds the cache. Treating that as
/// "show_overlay_on_wake flipped on" made launch/`onStart` save call
/// [`sync_visibility`] → `hide()` and flash a decorated pane in release.
static PREFS_SEEDED: AtomicBool = AtomicBool::new(false);

/// `hide()` / `is_visible()` on a never-shown WebView2 HWND restyles it on
/// Windows packaged builds and pops the empty title-bar window.
static OVERLAY_EVER_SHOWN: AtomicBool = AtomicBool::new(false);

/// Overlay React has painted (or the paint-wait fallback fired).
static OVERLAY_PAINTED: AtomicBool = AtomicBool::new(false);

/// A wake/fault/unlock asked to show before the island's first paint.
static OVERLAY_REVEAL_PENDING: AtomicBool = AtomicBool::new(false);

/// Create the HWND far off-screen so a first-frame style flash cannot appear.
const OVERLAY_OFFSCREEN_X: f64 = -32_000.0;
const OVERLAY_OFFSCREEN_Y: f64 = -32_000.0;

/// If the frontend never emits [`EVENT_OVERLAY_READY`], show anyway.
const OVERLAY_PAINT_FALLBACK: Duration = Duration::from_millis(1_200);

/// Mark overlay prefs as newer than whatever is still being loaded at boot.
pub fn mark_overlay_prefs_dirty() {
    OVERLAY_PREFS_DIRTY.store(true, Ordering::Release);
}

/// True after a save (or other live prefs write) beat the deferred boot load.
pub fn overlay_prefs_dirty() -> bool {
    OVERLAY_PREFS_DIRTY.load(Ordering::Acquire)
}

/// Desired layout so prefs/scale changes keep a live card sized.
/// 0 = presence, 1 = thought, 2 = card.
static OVERLAY_LAYOUT: AtomicU8 = AtomicU8::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverlayLayout {
    Presence = 0,
    Thought = 1,
    Card = 2,
}

impl OverlayLayout {
    fn from_u8(v: u8) -> Self {
        match v {
            2 => Self::Card,
            1 => Self::Thought,
            _ => Self::Presence,
        }
    }
}
static OVERLAY_SCALE_PERCENT: AtomicU16 = AtomicU16::new(100);
/// 0 = top_center, 1 = top_left, 2 = top_right
static OVERLAY_POSITION: AtomicU8 = AtomicU8::new(0);

pub const EVENT_OVERLAY_PREFERENCES: &str = "overlay-preferences";

/// UI → host: overlay React has painted; the HWND may be shown.
pub const EVENT_OVERLAY_READY: &str = "overlay-ready";

/// Host → UI: fade the painted island before the HWND is hidden.
pub const EVENT_OVERLAY_WILL_HIDE: &str = "overlay-will-hide";

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
    var html = document.documentElement;
    html.classList.add('overlay-mode');
    html.classList.remove('dark');
    html.style.setProperty('background', 'transparent', 'important');
    html.style.setProperty('background-color', 'transparent', 'important');
    var meta = document.querySelector('meta[name="color-scheme"]');
    if (meta) meta.setAttribute('content', 'only light');
    function clearChrome() {
      if (document.body) {
        document.body.style.setProperty('background', 'transparent', 'important');
        document.body.style.setProperty('background-color', 'transparent', 'important');
      }
      var root = document.getElementById('root');
      if (root) {
        root.style.setProperty('background', 'transparent', 'important');
        root.style.setProperty('background-color', 'transparent', 'important');
      }
      var splashes = document.querySelectorAll('.startup-splash');
      for (var i = 0; i < splashes.length; i++) {
        splashes[i].remove();
      }
    }
    clearChrome();
    if (document.readyState === 'loading') {
      document.addEventListener('DOMContentLoaded', clearChrome);
    }
  } catch (e) {}
})();
"#;

/// Create the overlay HWND. Idempotent. Built off-screen and never shown here.
///
/// Do **not** create this at process start. Packaged WebView2 on Windows
/// briefly realizes a decorated transparent frame during `build()` — that is
/// the flash on open. Call only when the island must actually appear.
///
/// Avoid [`WebviewWindowBuilder::from_config`]: the JSON template applies
/// min/max size (SetWindowPos) during create and flashes even worse.
pub fn spawn_overlay_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if app.get_webview_window("overlay").is_some() {
        return Ok(());
    }

    tracing::info!("building overlay window");
    let overlay = WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("index.html".into()))
        .title("")
        .inner_size(OVERLAY_CARD_BASE_WIDTH, OVERLAY_CARD_BASE_HEIGHT)
        .max_inner_size(OVERLAY_MAX_WIDTH, OVERLAY_MAX_HEIGHT)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false)
        .transparent(true)
        .shadow(false)
        .focused(false)
        .position(OVERLAY_OFFSCREEN_X, OVERLAY_OFFSCREEN_Y)
        .background_color(tauri::window::Color(0, 0, 0, 0))
        .initialization_script(OVERLAY_INIT_SCRIPT)
        .build()
        .map_err(|e| {
            tracing::error!(error = %e, "overlay WebviewWindowBuilder::build failed");
            e
        })?;

    let webview: &tauri::Webview<R> = overlay.as_ref();
    if let Err(e) = webview.set_background_color(Some(tauri::window::Color(0, 0, 0, 0))) {
        tracing::warn!(error = %e, "overlay webview clear color");
    }
    if let Err(e) = overlay.set_ignore_cursor_events(true) {
        tracing::error!(error = %e, "overlay set_ignore_cursor_events(true) failed");
    }

    OVERLAY_EVER_SHOWN.store(false, Ordering::Relaxed);
    OVERLAY_PAINTED.store(false, Ordering::Relaxed);
    OVERLAY_REVEAL_PENDING.store(false, Ordering::Relaxed);

    let handle = app.clone();
    let _ = app.once(EVENT_OVERLAY_READY, move |_| {
        on_overlay_painted(&handle);
    });

    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(OVERLAY_PAINT_FALLBACK);
        if OVERLAY_PAINTED.load(Ordering::Acquire) {
            return;
        }
        tracing::warn!("overlay paint signal timed out — revealing if pending");
        on_overlay_painted(&handle);
    });

    tracing::info!("overlay window ready off-screen (not shown)");
    Ok(())
}

fn ensure_overlay<R: Runtime>(app: &AppHandle<R>) -> Option<tauri::WebviewWindow<R>> {
    if let Some(existing) = app.get_webview_window("overlay") {
        return Some(existing);
    }
    if let Err(e) = spawn_overlay_window(app) {
        tracing::error!(error = %e, "ensure_overlay spawn failed");
        return None;
    }
    app.get_webview_window("overlay")
}

fn hide_if_present<R: Runtime>(app: &AppHandle<R>) {
    OVERLAY_REVEAL_PENDING.store(false, Ordering::Release);
    GEOMETRY_EPOCH.fetch_add(1, Ordering::SeqCst);
    PENDING_GEOMETRY.store(0, Ordering::Release);
    let Some(overlay) = app.get_webview_window("overlay") else {
        return;
    };
    hide_overlay(&overlay);
}

fn hide_overlay<R: Runtime>(overlay: &tauri::WebviewWindow<R>) {
    if !OVERLAY_EVER_SHOWN.load(Ordering::Relaxed) {
        return;
    }
    if !overlay.is_visible().unwrap_or(false) {
        return;
    }
    let _ = overlay.hide();
}

fn overlay_is_visible<R: Runtime>(overlay: &tauri::WebviewWindow<R>) -> bool {
    OVERLAY_EVER_SHOWN.load(Ordering::Relaxed) && overlay.is_visible().unwrap_or(false)
}

/// Cache overlay-related prefs for the hot status path.
///
/// Call whenever settings are loaded or saved so [`sync_visibility`] stays
/// correct without disk I/O.
pub fn remember_overlay_prefs(settings: &AppSettings) {
    SHOW_OVERLAY_ON_WAKE.store(settings.show_overlay_on_wake, Ordering::Relaxed);
    OVERLAY_CONTENT_HIDDEN.store(settings.overlay_caption_mode == "hidden", Ordering::Relaxed);
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

/// Cache prefs and notify the React surface. Touch HWND chrome only when
/// overlay geometry actually changed, or when the user just turned the island off.
///
/// Returns `true` if `show_overlay_on_wake` flipped on — the caller should then
/// [`sync_visibility`] so a live Ready/turn can appear immediately.
///
/// Windows 11 + WebView2: `set_size` / `set_max_size` / `set_position` restyle
/// the HWND and flash a **decorated** transparent pane (title bar, wallpaper
/// showing through) even when the island is already visible. That is the
/// random window on every Settings save. Unrelated toggles must not poke
/// window chrome; visibility stays status-driven.
pub fn apply_preferences<R: Runtime>(app: &AppHandle<R>, settings: &AppSettings) -> bool {
    let seeded = PREFS_SEEDED.swap(true, Ordering::AcqRel);
    let prev_show = SHOW_OVERLAY_ON_WAKE.load(Ordering::Relaxed);
    let prev_scale = OVERLAY_SCALE_PERCENT.load(Ordering::Relaxed);
    let prev_pos = OVERLAY_POSITION.load(Ordering::Relaxed);

    remember_overlay_prefs(settings);

    let _ = app.emit(
        EVENT_OVERLAY_PREFERENCES,
        OverlayPreferences::from(settings),
    );

    let show = SHOW_OVERLAY_ON_WAKE.load(Ordering::Relaxed);
    let scale = OVERLAY_SCALE_PERCENT.load(Ordering::Relaxed);
    let pos = OVERLAY_POSITION.load(Ordering::Relaxed);
    let show_changed = show != prev_show;
    let scale_changed = scale != prev_scale;
    let pos_changed = pos != prev_pos;

    // First call only fills the cache. Launch always saves (Start on open)
    // and used to look like the overlay pref flipped on.
    if !seeded {
        return false;
    }

    if !show && show_changed {
        hide_if_present(app);
        return false;
    }

    if let Some(overlay) = app.get_webview_window("overlay") {
        if overlay_is_visible(&overlay) && (scale_changed || pos_changed) {
            if scale_changed {
                coordinate_overlay_geometry(app, &overlay, scale, SCALE_CONTRACTION_SETTLE);
            } else if pos_changed {
                place_overlay(&overlay, cached_position());
            }
        }
    }

    show && show_changed
}

/// Show only during a wake/turn. Ready captions linger briefly; Off and a
/// disabled preference hide immediately. Delayed hides are generation-guarded
/// so a new wake cannot be hidden by an older timer.
///
/// Uses the cached wake-overlay preference (see [`remember_overlay_prefs`]) —
/// do not re-load settings here; status updates can fire many times per second.
pub fn sync_visibility<R: Runtime>(app: &AppHandle<R>, status: &StatusPicture) {
    let epoch = VISIBILITY_EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    let show_on_wake = SHOW_OVERLAY_ON_WAKE.load(Ordering::Relaxed);

    if !show_on_wake || status.engine == EngineState::Off {
        hide_if_present(app);
        return;
    }

    if status.engine == EngineState::Fault {
        if let Some(overlay) = ensure_overlay(app) {
            OVERLAY_LAYOUT.store(OverlayLayout::Presence as u8, Ordering::Relaxed);
            if overlay_is_visible(&overlay) {
                coordinate_overlay_geometry(
                    app,
                    &overlay,
                    OVERLAY_SCALE_PERCENT.load(Ordering::Relaxed),
                    LAYOUT_CONTRACTION_SETTLE,
                );
            } else {
                request_reveal(&overlay, OverlayLayout::Presence);
            }
            schedule_hide(app.clone(), epoch, FAULT_LINGER);
        }
        return;
    }

    if status.phase == Phase::Off {
        hide_if_present(app);
        return;
    }

    let layout = layout_for(status);
    OVERLAY_LAYOUT.store(layout as u8, Ordering::Relaxed);
    let scale_percent = OVERLAY_SCALE_PERCENT.load(Ordering::Relaxed);

    let active = matches!(
        status.phase,
        Phase::Hearing
            | Phase::Reading
            | Phase::Thinking
            | Phase::Talking
            | Phase::AwaitingReply
            | Phase::AwaitingConfirm
    );

    if !active {
        // Armed/Quiet: never create or resurrect the island at startup.
        if let Some(overlay) = app.get_webview_window("overlay") {
            if overlay_is_visible(&overlay) {
                // Check every snapshot, not just layout changes. A transient
                // WebView2 resize failure therefore self-heals.
                coordinate_overlay_geometry(
                    app,
                    &overlay,
                    scale_percent,
                    LAYOUT_CONTRACTION_SETTLE,
                );
                let linger = if status.artifact.is_some() {
                    CARD_LINGER
                } else {
                    READY_LINGER
                };
                schedule_hide(app.clone(), epoch, linger);
            } else {
                // Wake ended before first paint — do not show later.
                OVERLAY_REVEAL_PENDING.store(false, Ordering::Release);
            }
        }
        return;
    }

    let Some(overlay) = ensure_overlay(app) else {
        return;
    };
    let visible = overlay_is_visible(&overlay);
    if !visible {
        // Stay off-screen until React paints. show()+park on a still-loading
        // WebView2 is the solid rectangle on first wake.
        request_reveal(&overlay, layout);
    } else {
        coordinate_overlay_geometry(app, &overlay, scale_percent, LAYOUT_CONTRACTION_SETTLE);
    }
}

fn wants_card_layout(status: &StatusPicture) -> bool {
    // `artifact` is this-turn only (cleared on the next utterance).
    status.artifact.is_some()
        && status.engine == EngineState::On
        && !matches!(status.phase, Phase::Hearing | Phase::Reading | Phase::Off)
}

/// Must stay aligned with `overlayStageMode` in the overlay React surface.
fn layout_for(status: &StatusPicture) -> OverlayLayout {
    if OVERLAY_CONTENT_HIDDEN.load(Ordering::Relaxed) {
        OverlayLayout::Presence
    } else if wants_card_layout(status) {
        OverlayLayout::Card
    } else if status.phase == Phase::Thinking {
        OverlayLayout::Thought
    } else {
        OverlayLayout::Presence
    }
}

fn apply_overlay_size<R: Runtime>(overlay: &tauri::WebviewWindow<R>, scale: f64) -> bool {
    let (w, h) = overlay_dimensions(scale);
    // Max size is set once at spawn. Re-applying set_max_size on Windows
    // restyles the HWND and flashes a decorated transparent frame.
    if overlay_size_matches(overlay, w, h) {
        return true;
    }
    if let Err(e) = overlay.set_size(tauri::Size::Logical(LogicalSize::new(w, h))) {
        tracing::warn!(error = %e, "overlay resize failed");
        return false;
    }
    true
}

/// Native viewport is always the card budget. Presence and thought paint a
/// smaller island inside it and grow from the center, so Listening → Thinking
/// does not call `set_size`. Windows grows a HWND from the top-left, then a
/// later `set_position` slides it back, which is the "moves then grows" jump.
fn overlay_dimensions(scale: f64) -> (f64, f64) {
    (
        OVERLAY_CARD_BASE_WIDTH * scale,
        OVERLAY_CARD_BASE_HEIGHT * scale,
    )
}

fn geometry_key(scale_percent: u16) -> u32 {
    u32::from(scale_percent)
}

/// Keep the HWND on the card budget. Scale-down waits for the island's exit
/// animation so the viewport does not clip it. Layout changes never resize.
fn coordinate_overlay_geometry<R: Runtime>(
    app: &AppHandle<R>,
    overlay: &tauri::WebviewWindow<R>,
    scale_percent: u16,
    contraction_settle: Duration,
) {
    let scale_percent = scale_percent.clamp(75, 125);
    let scale = f64::from(scale_percent) / 100.0;
    let (target_w, target_h) = overlay_dimensions(scale);

    if overlay_size_matches(overlay, target_w, target_h) {
        if PENDING_GEOMETRY.swap(0, Ordering::AcqRel) != 0 {
            GEOMETRY_EPOCH.fetch_add(1, Ordering::SeqCst);
        }
        return;
    }

    let shrinking = overlay
        .inner_size()
        .ok()
        .zip(overlay.scale_factor().ok())
        .filter(|(_, factor)| *factor > 0.0)
        .is_some_and(|(current, factor)| {
            target_w < f64::from(current.width) / factor - 1.0
                || target_h < f64::from(current.height) / factor - 1.0
        });

    if !shrinking {
        GEOMETRY_EPOCH.fetch_add(1, Ordering::SeqCst);
        PENDING_GEOMETRY.store(0, Ordering::Release);
        if apply_overlay_size(overlay, scale) {
            place_overlay(overlay, cached_position());
        }
        return;
    }

    let key = geometry_key(scale_percent);
    if PENDING_GEOMETRY.swap(key, Ordering::AcqRel) == key {
        return;
    }
    let epoch = GEOMETRY_EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(contraction_settle);
        if GEOMETRY_EPOCH.load(Ordering::SeqCst) != epoch
            || PENDING_GEOMETRY.load(Ordering::Acquire) != key
            || OVERLAY_SCALE_PERCENT.load(Ordering::Relaxed) != scale_percent
        {
            return;
        }
        let Some(overlay) = app.get_webview_window("overlay") else {
            PENDING_GEOMETRY
                .compare_exchange(key, 0, Ordering::AcqRel, Ordering::Acquire)
                .ok();
            return;
        };
        if apply_overlay_size(&overlay, scale) {
            place_overlay(&overlay, cached_position());
        }
        PENDING_GEOMETRY
            .compare_exchange(key, 0, Ordering::AcqRel, Ordering::Acquire)
            .ok();
    });
}

fn overlay_size_matches<R: Runtime>(
    overlay: &tauri::WebviewWindow<R>,
    logical_w: f64,
    logical_h: f64,
) -> bool {
    let Ok(current) = overlay.inner_size() else {
        return false;
    };
    let Ok(factor) = overlay.scale_factor() else {
        return false;
    };
    if factor <= 0.0 {
        return false;
    }
    let cw = f64::from(current.width) / factor;
    let ch = f64::from(current.height) / factor;
    (cw - logical_w).abs() < 1.0 && (ch - logical_h).abs() < 1.0
}

fn cached_position() -> &'static str {
    match OVERLAY_POSITION.load(Ordering::Relaxed) {
        1 => "top_left",
        2 => "top_right",
        _ => "top_center",
    }
}

/// Ask to park+show. If the island has not painted yet, remember the request
/// and wait for [`EVENT_OVERLAY_READY`] (or the spawn fallback).
fn request_reveal<R: Runtime>(overlay: &tauri::WebviewWindow<R>, layout: OverlayLayout) {
    OVERLAY_LAYOUT.store(layout as u8, Ordering::Relaxed);
    OVERLAY_REVEAL_PENDING.store(true, Ordering::Release);
    if OVERLAY_PAINTED.load(Ordering::Acquire) {
        flush_pending_reveal(overlay);
    }
}

fn on_overlay_painted<R: Runtime>(app: &AppHandle<R>) {
    OVERLAY_PAINTED.store(true, Ordering::Release);
    let Some(overlay) = app.get_webview_window("overlay") else {
        return;
    };
    if OVERLAY_REVEAL_PENDING.load(Ordering::Acquire) {
        flush_pending_reveal(&overlay);
    }
}

fn flush_pending_reveal<R: Runtime>(overlay: &tauri::WebviewWindow<R>) {
    if !OVERLAY_REVEAL_PENDING.swap(false, Ordering::AcqRel) {
        return;
    }
    let scale = f64::from(OVERLAY_SCALE_PERCENT.load(Ordering::Relaxed)) / 100.0;
    GEOMETRY_EPOCH.fetch_add(1, Ordering::SeqCst);
    PENDING_GEOMETRY.store(0, Ordering::Release);
    apply_overlay_size(overlay, scale);
    if !overlay_is_visible(overlay) {
        show_without_focus(overlay);
        place_overlay(overlay, cached_position());
    } else {
        place_overlay(overlay, cached_position());
    }
}

fn show_without_focus<R: Runtime>(overlay: &tauri::WebviewWindow<R>) {
    OVERLAY_EVER_SHOWN.store(true, Ordering::Relaxed);
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
        std::thread::sleep(delay.saturating_sub(HIDE_EXIT_SETTLE));
        if VISIBILITY_EPOCH.load(Ordering::SeqCst) != epoch {
            return;
        }
        let _ = app.emit(EVENT_OVERLAY_WILL_HIDE, ());
        std::thread::sleep(HIDE_EXIT_SETTLE);
        if VISIBILITY_EPOCH.load(Ordering::SeqCst) != epoch {
            return;
        }
        hide_if_present(&app);
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

    let scale = f64::from(OVERLAY_SCALE_PERCENT.load(Ordering::Relaxed).clamp(75, 125)) / 100.0;
    let screen_right = pos.x + screen.width as i32;
    let x = match position {
        "top_left" => pos.x + OVERLAY_SIDE_MARGIN,
        "top_right" => screen_right - win_size.width as i32 - OVERLAY_SIDE_MARGIN,
        _ => pos.x + (screen.width as i32 - win_size.width as i32) / 2,
    };
    let y = overlay_park_y(pos.y, screen.height, scale);

    if let Ok(current) = overlay.outer_position() {
        if current.x == x && current.y == y {
            return;
        }
    }

    if let Err(e) = overlay.set_position(tauri::Position::Physical(PhysicalPosition::new(x, y))) {
        tracing::warn!(error = %e, "overlay set_position failed");
    } else {
        tracing::info!(x, y, position, "overlay parked");
    }
}

/// HWND top. The window is the card budget and the island is centered in it,
/// so shift up by half the extra height. That keeps a compact pill where the
/// old presence window sat instead of dropping it into the middle of the card.
fn overlay_park_y(monitor_y: i32, screen_height: u32, scale: f64) -> i32 {
    let margin = (f64::from(screen_height) * OVERLAY_TOP_MARGIN_FRAC) as i32;
    let presence_h = OVERLAY_BASE_HEIGHT * scale;
    let max_h = OVERLAY_CARD_BASE_HEIGHT * scale;
    monitor_y + margin + ((presence_h - max_h) / 2.0).round() as i32
}

/// When `locked` is true, mouse passes through to the game (default).
/// When false, the user can click/drag the island to reposition it.
pub fn set_overlay_input_locked<R: Runtime>(app: &AppHandle<R>, locked: bool) -> tauri::Result<()> {
    let Some(overlay) = ensure_overlay(app) else {
        tracing::warn!("set_overlay_input_locked: overlay window missing");
        return Ok(());
    };
    overlay.set_ignore_cursor_events(locked)?;
    if !locked {
        // Unlocking is an explicit request to position the overlay. Make the
        // otherwise wake-only window visible without focusing it.
        request_reveal(
            &overlay,
            OverlayLayout::from_u8(OVERLAY_LAYOUT.load(Ordering::Relaxed)),
        );
    }
    tracing::info!(locked, "overlay input lock updated");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use boris_pipeline::{ArtifactPeek, EngineState, Phase, StatusPicture};

    fn on() -> StatusPicture {
        let mut picture = StatusPicture::off();
        picture.engine = EngineState::On;
        picture
    }

    #[test]
    fn thinking_phase_uses_thought_layout() {
        let mut picture = on();
        picture.phase = Phase::Thinking;
        picture.thinking = Some("Need to search first.".into());
        assert_eq!(layout_for(&picture), OverlayLayout::Thought);
    }

    #[test]
    fn artifact_wins_over_thought_layout() {
        let mut picture = on();
        picture.phase = Phase::Thinking;
        picture.thinking = Some("Still thinking.".into());
        picture.artifact = Some(ArtifactPeek {
            id: "a1".into(),
            title: "Notes".into(),
            kind: "markdown".into(),
            language: None,
            path: "notes.md".into(),
        });
        assert_eq!(layout_for(&picture), OverlayLayout::Card);
    }

    #[test]
    fn hearing_stays_presence() {
        let mut picture = on();
        picture.phase = Phase::Hearing;
        assert_eq!(layout_for(&picture), OverlayLayout::Presence);
    }

    #[test]
    fn hwnd_stays_on_the_card_budget() {
        assert_eq!(overlay_dimensions(1.0), (464.0, 364.0));
        assert_eq!(overlay_dimensions(1.25), (580.0, 455.0));
    }

    #[test]
    fn geometry_targets_follow_scale_only() {
        assert_eq!(geometry_key(100), 100);
        assert_ne!(geometry_key(75), geometry_key(125));
    }

    #[test]
    fn park_y_keeps_presence_center_on_a_card_hwnd() {
        let margin = (1080.0 * OVERLAY_TOP_MARGIN_FRAC) as i32;
        let y = overlay_park_y(0, 1080, 1.0);
        assert_eq!(y, margin - 90);
        assert_eq!(margin + 184 / 2, y + 364 / 2);

        let y125 = overlay_park_y(0, 1080, 1.25);
        assert_eq!(
            margin + (184.0_f64 * 1.25 / 2.0).round() as i32,
            y125 + (364.0_f64 * 1.25 / 2.0).round() as i32
        );
    }
}
