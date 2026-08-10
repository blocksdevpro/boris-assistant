//! Engine lifecycle host for [`boris_pipeline::Engine`].
//!
//! # Responsibility
//!
//! | This module (host) | `boris_pipeline` (pipeline) |
//! |--------------------|-------------------------------|
//! | Spawn engine thread once | Sequential voice turns, wake/VAD |
//! | Mirror `StatusPicture` for UI | Produce status snapshots |
//! | Map Start/Stop + device prefs → commands | Apply `EngineCommand`s |
//! | Gate Start on empty key / preflight | Load models, run agent |
//!
//! No voice policy lives here — only process-local state and IPC-facing APIs
//! used by [`crate::commands`].

use std::sync::{Arc, Mutex};
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

/// Shared app state: engine handle + latest status snapshot + device prefs.
pub struct AppState {
    status: Arc<Mutex<StatusPicture>>,
    handle: Mutex<Option<EngineHandle>>,
    /// Keep the engine join-handle alive for the process lifetime.
    _engine: Mutex<Option<Engine>>,
    /// Last UI-selected devices (applied on Start and on every switch).
    preferred_input: Mutex<Option<String>>,
    preferred_output: Mutex<Option<String>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            status: Arc::new(Mutex::new(StatusPicture::off())),
            handle: Mutex::new(None),
            _engine: Mutex::new(None),
            preferred_input: Mutex::new(None),
            preferred_output: Mutex::new(None),
        }
    }

    pub fn status(&self) -> StatusPicture {
        self.status.lock().unwrap().clone()
    }

    /// Model paths / readiness for the UI Start gate (`boris_pipeline::paths`).
    pub fn preflight() -> PreflightReport {
        paths::preflight()
    }

    /// Spawn the engine on first call, then send Start, then apply preferred devices.
    ///
    /// `on_status` is invoked on every status snapshot (use to `emit` to the UI).
    ///
    /// `model` / `fast_model` are OpenRouter model ids. `model_provider` /
    /// `fast_provider` are OpenRouter **model-providers** (CoreWeave, Baseten, …),
    /// not API brands — comma-separated slugs for `provider.order`.
    pub fn start(
        &self,
        api_key: String,
        model: Option<String>,
        fast_model: Option<String>,
        model_provider: Option<String>,
        fast_provider: Option<String>,
        pin_provider: Option<bool>,
        on_status: impl Fn(StatusPicture) + Send + 'static,
    ) -> Result<(), String> {
        let mut handle_g = self.handle.lock().unwrap();
        let already_spawned = handle_g.is_some();
        info!(
            already_spawned,
            model = ?model,
            fast_model = ?fast_model,
            model_provider = ?model_provider,
            fast_provider = ?fast_provider,
            pin_provider = ?pin_provider,
            "AppState::start"
        );

        if handle_g.is_none() {
            self.spawn_engine_once(
                &mut handle_g,
                api_key,
                model,
                fast_model,
                model_provider,
                fast_provider,
                pin_provider,
                on_status,
            )?;
        }

        let handle = handle_g
            .as_ref()
            .ok_or_else(|| "engine missing".to_string())?
            .clone();
        drop(handle_g);

        handle
            .start()
            .map_err(|e| {
                let msg = format!("failed to start engine: {e}");
                error!(error = %msg, "EngineHandle::start failed");
                msg
            })?;

        self.apply_preferred_devices(&handle);

        info!("AppState::start complete");
        Ok(())
    }

    /// First-time engine spawn: validate key + preflight, build config, start mirror.
    fn spawn_engine_once(
        &self,
        handle_g: &mut Option<EngineHandle>,
        api_key: String,
        model: Option<String>,
        fast_model: Option<String>,
        model_provider: Option<String>,
        fast_provider: Option<String>,
        pin_provider: Option<bool>,
        on_status: impl Fn(StatusPicture) + Send + 'static,
    ) -> Result<(), String> {
        if api_key.trim().is_empty() {
            error!("start rejected: empty API key");
            return Err(
                "API key is required. Paste an OpenRouter key or set OPENROUTER_API_KEY."
                    .into(),
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

        let mut prefs = LlmPrefs::new(api_key);
        prefs.openrouter_model = model;
        prefs.openrouter_fast_model = fast_model;
        prefs.openrouter_model_provider = model_provider;
        prefs.openrouter_fast_provider = fast_provider;
        prefs.openrouter_pin_provider = pin_provider;
        let config =
            PipelineConfig::with_llm(prefs, SUPERTONE_SAMPLE_RATE, WAKEWORD_MODEL_BYTES.to_vec());

        let (engine, handle, status_rx) =
            Engine::spawn(config).map_err(|e| e.to_string())?;
        *self._engine.lock().unwrap() = Some(engine);
        *handle_g = Some(handle.clone());
        info!("engine thread spawned");

        Self::spawn_status_mirror(self.status.clone(), status_rx, on_status);
        Ok(())
    }

    /// Background thread: cache each snapshot and forward to the UI emit callback.
    fn spawn_status_mirror(
        status_cache: Arc<Mutex<StatusPicture>>,
        status_rx: std::sync::mpsc::Receiver<StatusPicture>,
        on_status: impl Fn(StatusPicture) + Send + 'static,
    ) {
        thread::spawn(move || {
            info!("status mirror thread started");
            while let Ok(picture) = status_rx.recv() {
                debug!(
                    engine = ?picture.engine,
                    phase = ?picture.phase,
                    "status snapshot"
                );
                *status_cache.lock().unwrap() = picture.clone();
                on_status(picture);
            }
            warn!("status channel closed — mirror thread exiting");
        });
    }

    /// Re-apply devices the user picked before/while Off (engine accepts Switch* while Armed).
    fn apply_preferred_devices(&self, handle: &EngineHandle) {
        if let Some(id) = self.preferred_input.lock().unwrap().clone() {
            info!(%id, "applying preferred input on start");
            if let Err(e) = handle.send(EngineCommand::SwitchInput { device_id: id }) {
                warn!(error = %e, "preferred input switch failed");
            }
        }
        if let Some(id) = self.preferred_output.lock().unwrap().clone() {
            info!(%id, "applying preferred output on start");
            if let Err(e) = handle.send(EngineCommand::SwitchOutput { device_id: id }) {
                warn!(error = %e, "preferred output switch failed");
            }
        }
    }

    pub fn stop(&self) -> Result<(), String> {
        let handle_g = self.handle.lock().unwrap();
        if let Some(handle) = handle_g.as_ref() {
            info!("stopping engine");
            handle.stop().map_err(|e| {
                let msg = format!("failed to stop engine: {e}");
                error!(error = %msg, "EngineHandle::stop failed");
                msg
            })?;
            info!("engine stop command sent");
        } else {
            debug!("stop called but engine was never spawned");
        }
        Ok(())
    }

    pub fn switch_input(&self, device_id: String) -> Result<(), String> {
        if device_id.trim().is_empty() {
            return Err("empty input device id".into());
        }
        *self.preferred_input.lock().unwrap() = Some(device_id.clone());

        let handle_g = self.handle.lock().unwrap();
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
        *self.preferred_output.lock().unwrap() = Some(device_id.clone());

        let handle_g = self.handle.lock().unwrap();
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
