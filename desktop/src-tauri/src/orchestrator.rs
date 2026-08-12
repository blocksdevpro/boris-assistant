//! Engine lifecycle host for [`boris_pipeline::Engine`].
//!
//! # Responsibility
//!
//! | This module (host) | `boris_pipeline` (pipeline) |
//! |--------------------|-------------------------------|
//! | Spawn / rebuild engine thread | Sequential voice turns, wake/VAD |
//! | Mirror `StatusPicture` for UI | Produce status snapshots |
//! | Map Start/Stop + device prefs → commands | Apply `EngineCommand`s |
//! | Gate Start on empty key / preflight | Load models, run agent |
//!
//! No voice policy lives here — only process-local state and IPC-facing APIs
//! used by [`crate::commands`].

use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use boris_pipeline::{
    devices, paths, DeviceDto, Engine, EngineCommand, EngineHandle, LlmPrefs, PipelineConfig,
    PreflightReport, StatusPicture,
};
use boris_tts_supertone::SUPERTONE_SAMPLE_RATE;
use tracing::{debug, error, info, warn};

/// Embedded wake model (path relative to this crate → workspace `assets/`).
///
/// Compile-time embed keeps always-on wake available without a first-run download.
static WAKEWORD_MODEL_BYTES: &[u8] =
    include_bytes!("../../../assets/models/livekit/boris-large.onnx");

/// LLM args that force an engine rebuild when they change while a thread is live.
///
/// Pipeline config is fixed at [`Engine::spawn`]; there is no in-thread reconfigure.
#[derive(Clone, Debug, PartialEq, Eq)]
struct LlmFingerprint {
    api_key: String,
    model: Option<String>,
    fast_model: Option<String>,
    model_provider: Option<String>,
    fast_provider: Option<String>,
    pin_provider: Option<bool>,
}

impl LlmFingerprint {
    fn from_start_args(
        api_key: &str,
        model: &Option<String>,
        fast_model: &Option<String>,
        model_provider: &Option<String>,
        fast_provider: &Option<String>,
        pin_provider: Option<bool>,
    ) -> Self {
        Self {
            api_key: api_key.trim().to_string(),
            model: normalize_opt(model.clone()),
            fast_model: normalize_opt(fast_model.clone()),
            model_provider: normalize_opt(model_provider.clone()),
            fast_provider: normalize_opt(fast_provider.clone()),
            pin_provider,
        }
    }
}

fn normalize_opt(s: Option<String>) -> Option<String> {
    s.and_then(|v| {
        let t = v.trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    })
}

/// Recover from a poisoned mutex without panicking IPC handlers.
///
/// Logs once per recovery so a prior panic on another thread is visible.
fn lock_or_recover<'a, T>(mutex: &'a Mutex<T>, name: &'static str) -> MutexGuard<'a, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        error!(
            mutex = name,
            "mutex poisoned; recovering with inner value (IPC will not panic)"
        );
        poisoned.into_inner()
    })
}

/// Shared app state: engine handle + latest status snapshot + device prefs.
pub struct AppState {
    status: Arc<Mutex<StatusPicture>>,
    handle: Mutex<Option<EngineHandle>>,
    /// Join handle + shutdown sender; taken on teardown / rebuild.
    engine: Mutex<Option<Engine>>,
    /// Fingerprint of LLM prefs baked into the live engine thread (if any).
    live_llm: Mutex<Option<LlmFingerprint>>,
    /// Last UI-selected devices (applied on Start and on every switch).
    preferred_input: Mutex<Option<String>>,
    preferred_output: Mutex<Option<String>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            status: Arc::new(Mutex::new(StatusPicture::off())),
            handle: Mutex::new(None),
            engine: Mutex::new(None),
            live_llm: Mutex::new(None),
            preferred_input: Mutex::new(None),
            preferred_output: Mutex::new(None),
        }
    }

    pub fn status(&self) -> StatusPicture {
        lock_or_recover(&self.status, "status").clone()
    }

    /// Model paths / readiness for the UI Start gate (`boris_pipeline::paths`).
    pub fn preflight() -> PreflightReport {
        paths::preflight()
    }

    /// Ensure an engine thread exists for these LLM prefs, then send Start.
    ///
    /// Spawns on first call. If a thread is already live with a **different**
    /// LLM fingerprint (api_key / models / providers), tears it down and
    /// respawns with the new config. Same fingerprint reuses the thread.
    ///
    /// `on_status` is invoked on every status snapshot (use to `emit` to the UI).
    ///
    /// `model` / `fast_model` are OpenRouter model ids. `model_provider` /
    /// `fast_provider` are OpenRouter **model-providers** (CoreWeave, Baseten, …),
    /// not API brands — comma-separated slugs for `provider.order`.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        api_key: String,
        model: Option<String>,
        fast_model: Option<String>,
        model_provider: Option<String>,
        fast_provider: Option<String>,
        pin_provider: Option<bool>,
        on_status: impl Fn(StatusPicture) + Send + Sync + 'static,
    ) -> Result<(), String> {
        // Arc so a rebuild path (dead channel / fingerprint change) can attach
        // a fresh status mirror without requiring Clone on the UI emit closure.
        let on_status: Arc<dyn Fn(StatusPicture) + Send + Sync + 'static> = Arc::new(on_status);
        let fingerprint = LlmFingerprint::from_start_args(
            &api_key,
            &model,
            &fast_model,
            &model_provider,
            &fast_provider,
            pin_provider,
        );

        // Serialize Start/Stop against each other (handle lock is the gate).
        let mut handle_g = lock_or_recover(&self.handle, "handle");
        let live = lock_or_recover(&self.live_llm, "live_llm").clone();
        let already_spawned = handle_g.is_some();
        let fingerprint_match = live.as_ref() == Some(&fingerprint);

        info!(
            already_spawned,
            fingerprint_match,
            model = ?fingerprint.model,
            fast_model = ?fingerprint.fast_model,
            model_provider = ?fingerprint.model_provider,
            fast_provider = ?fingerprint.fast_provider,
            pin_provider = ?fingerprint.pin_provider,
            "AppState::start"
        );

        if already_spawned && !fingerprint_match {
            info!("LLM fingerprint changed — tearing down engine before respawn");
        }
        let handle = self.spawn_and_ensure_handle(
            &mut handle_g,
            already_spawned && !fingerprint_match,
            fingerprint.clone(),
            Arc::clone(&on_status),
        )?;
        // Release gate before Start send so Stop can interleave after spawn;
        // Start uses a cloned channel sender and remains safe.
        drop(handle_g);

        if let Err(e) = handle.start() {
            // Dead command channel (engine exited without host teardown).
            warn!(error = %e, "EngineHandle::start failed — rebuilding engine");
            let mut handle_g = lock_or_recover(&self.handle, "handle");
            let handle =
                self.spawn_and_ensure_handle(&mut handle_g, true, fingerprint, on_status)?;
            drop(handle_g);
            handle.start().map_err(|e| {
                let msg = format!("failed to start engine after rebuild: {e}");
                error!(error = %msg, "EngineHandle::start failed");
                msg
            })?;
            self.apply_preferred_devices(&handle);
            info!("AppState::start complete (after rebuild)");
            return Ok(());
        }

        self.apply_preferred_devices(&handle);

        info!("AppState::start complete");
        Ok(())
    }

    /// Teardown-if-needed → spawn (if not already live) → return a cloned handle.
    ///
    /// Shared by the main Start path (teardown only on fingerprint change) and
    /// the dead-channel retry path (unconditional teardown) in [`Self::start`].
    /// Caller holds `handle_g`; pure refactor, no behavior change.
    fn spawn_and_ensure_handle(
        &self,
        handle_g: &mut MutexGuard<'_, Option<EngineHandle>>,
        force_teardown: bool,
        fingerprint: LlmFingerprint,
        on_status: Arc<dyn Fn(StatusPicture) + Send + Sync + 'static>,
    ) -> Result<EngineHandle, String> {
        if force_teardown {
            self.teardown_engine(handle_g);
        }
        if handle_g.is_none() {
            self.spawn_engine(handle_g, fingerprint, on_status)?;
        }
        handle_g
            .as_ref()
            .cloned()
            .ok_or_else(|| "engine missing".to_string())
    }

    /// Validate key + preflight, spawn engine thread, store handle + fingerprint.
    ///
    /// Caller must hold `handle_g` (empty).
    fn spawn_engine(
        &self,
        handle_g: &mut Option<EngineHandle>,
        fingerprint: LlmFingerprint,
        on_status: Arc<dyn Fn(StatusPicture) + Send + Sync + 'static>,
    ) -> Result<(), String> {
        debug_assert!(handle_g.is_none(), "spawn_engine requires empty handle");

        if fingerprint.api_key.is_empty() {
            error!("start rejected: empty API key");
            return Err(
                "API key is required. Paste an OpenRouter key or set OPENROUTER_API_KEY.".into(),
            );
        }

        // Defense in depth: block engine spawn when models are missing.
        let report = paths::preflight();
        if !report.ok {
            error!(messages = ?report.messages, "start rejected: preflight failed");
            return Err(format!(
                "Cannot start — models not ready. {}",
                report.messages.join(" ")
            ));
        }

        info!(
            boris_home = %paths::boris_home().display(),
            wake_model_bytes = WAKEWORD_MODEL_BYTES.len(),
            "spawning pipeline engine"
        );

        let mut prefs = LlmPrefs::new(fingerprint.api_key.clone());
        prefs.openrouter_model = fingerprint.model.clone();
        prefs.openrouter_fast_model = fingerprint.fast_model.clone();
        prefs.openrouter_model_provider = fingerprint.model_provider.clone();
        prefs.openrouter_fast_provider = fingerprint.fast_provider.clone();
        prefs.openrouter_pin_provider = fingerprint.pin_provider;
        let config =
            PipelineConfig::with_llm(prefs, SUPERTONE_SAMPLE_RATE, WAKEWORD_MODEL_BYTES.to_vec());

        let (engine, handle, status_rx) = Engine::spawn(config).map_err(|e| e.to_string())?;
        *lock_or_recover(&self.engine, "engine") = Some(engine);
        *handle_g = Some(handle.clone());
        *lock_or_recover(&self.live_llm, "live_llm") = Some(fingerprint);
        info!("engine thread spawned");

        Self::spawn_status_mirror(self.status.clone(), status_rx, on_status);
        Ok(())
    }

    /// Shutdown + join the engine thread and clear host state.
    ///
    /// Holds `handle_g` so concurrent Start/Stop cannot observe a half-torn
    /// state. Safe if nothing is running.
    fn teardown_engine(&self, handle_g: &mut Option<EngineHandle>) {
        *handle_g = None;
        *lock_or_recover(&self.live_llm, "live_llm") = None;

        let engine = lock_or_recover(&self.engine, "engine").take();
        if let Some(engine) = engine {
            info!("shutting down engine thread");
            engine.shutdown_and_join();
            info!("engine thread joined");
        } else {
            debug!("teardown: no engine join handle");
        }

        // Channel is closed after join; mirror thread exits. Reset UI snapshot.
        *lock_or_recover(&self.status, "status") = StatusPicture::off();
    }

    /// Background thread: cache each snapshot and forward to the UI emit callback.
    fn spawn_status_mirror(
        status_cache: Arc<Mutex<StatusPicture>>,
        status_rx: std::sync::mpsc::Receiver<StatusPicture>,
        on_status: Arc<dyn Fn(StatusPicture) + Send + Sync + 'static>,
    ) {
        thread::spawn(move || {
            info!("status mirror thread started");
            while let Ok(picture) = status_rx.recv() {
                debug!(
                    engine = ?picture.engine,
                    phase = ?picture.phase,
                    "status snapshot"
                );
                *lock_or_recover(&status_cache, "status") = picture.clone();
                on_status(picture);
            }
            warn!("status channel closed — mirror thread exiting");
        });
    }

    /// Re-apply devices the user picked before/while Off (engine accepts Switch* while Armed).
    fn apply_preferred_devices(&self, handle: &EngineHandle) {
        if let Some(id) = lock_or_recover(&self.preferred_input, "preferred_input").clone() {
            info!(%id, "applying preferred input on start");
            if let Err(e) = handle.send(EngineCommand::SwitchInput { device_id: id }) {
                warn!(error = %e, "preferred input switch failed");
            }
        }
        if let Some(id) = lock_or_recover(&self.preferred_output, "preferred_output").clone() {
            info!(%id, "applying preferred output on start");
            if let Err(e) = handle.send(EngineCommand::SwitchOutput { device_id: id }) {
                warn!(error = %e, "preferred output switch failed");
            }
        }
    }

    /// Fully tear down the engine so a later Start can respawn cleanly.
    ///
    /// Sends Shutdown (via [`Engine::shutdown_and_join`]), joins the thread,
    /// and clears handle / fingerprint / status. Soft `EngineCommand::Stop`
    /// alone would leave the old LLM config baked into a live thread.
    pub fn stop(&self) -> Result<(), String> {
        let mut handle_g = lock_or_recover(&self.handle, "handle");
        if handle_g.is_none() && lock_or_recover(&self.engine, "engine").is_none() {
            debug!("stop called but engine was never spawned");
            return Ok(());
        }
        info!("stopping engine (full teardown)");
        self.teardown_engine(&mut handle_g);
        info!("AppState::stop complete");
        Ok(())
    }

    pub fn switch_input(&self, device_id: String) -> Result<(), String> {
        if device_id.trim().is_empty() {
            return Err("empty input device id".into());
        }
        *lock_or_recover(&self.preferred_input, "preferred_input") = Some(device_id.clone());

        let handle_g = lock_or_recover(&self.handle, "handle");
        if let Some(handle) = handle_g.as_ref() {
            info!(%device_id, "forwarding SwitchInput to engine");
            handle
                .send(EngineCommand::SwitchInput { device_id })
                .map_err(|e| {
                    error!(error = %e, "SwitchInput send failed");
                    e.to_string()
                })?;
        } else {
            debug!(%device_id, "input preference stored (engine not running yet)");
        }
        // If engine not spawned yet, preference is applied on Start.
        Ok(())
    }

    pub fn switch_output(&self, device_id: String) -> Result<(), String> {
        if device_id.trim().is_empty() {
            return Err("empty output device id".into());
        }
        *lock_or_recover(&self.preferred_output, "preferred_output") = Some(device_id.clone());

        let handle_g = lock_or_recover(&self.handle, "handle");
        if let Some(handle) = handle_g.as_ref() {
            info!(%device_id, "forwarding SwitchOutput to engine");
            handle
                .send(EngineCommand::SwitchOutput { device_id })
                .map_err(|e| {
                    error!(error = %e, "SwitchOutput send failed");
                    e.to_string()
                })?;
        } else {
            debug!(%device_id, "output preference stored (engine not running yet)");
        }
        Ok(())
    }

    pub fn list_inputs() -> Vec<DeviceDto> {
        let list = devices::list_input_devices();
        debug!(count = list.len(), "list_input_devices");
        list
    }

    pub fn list_outputs() -> Vec<DeviceDto> {
        let list = devices::list_output_devices();
        debug!(count = list.len(), "list_output_devices");
        list
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
