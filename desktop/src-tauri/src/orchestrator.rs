//! Desktop host for [`boris_pipeline::Engine`].
//!
//! Owns lifecycle (start/stop), mirrors status for the UI, and maps Tauri
//! commands onto the engine. No voice policy lives here.

use std::sync::{Arc, Mutex};
use std::thread;

use boris_pipeline::{
    devices, paths, DeviceDto, Engine, EngineCommand, EngineHandle, PipelineConfig,
    PreflightReport, StatusPicture,
};
use boris_tts_supertone::SUPERTONE_SAMPLE_RATE;
use tracing::{debug, error, info, warn};

/// Embedded wake model (path relative to this crate → workspace `assets/`).
static WAKEWORD_MODEL_BYTES: &[u8] =
    include_bytes!("../../../assets/models/livekit/boris-large.onnx");

/// Shared app state: engine handle + latest status snapshot.
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

    /// Model paths / readiness for the UI Start gate.
    pub fn preflight() -> PreflightReport {
        paths::preflight()
    }

    /// Spawn the engine on first call, then send Start, then apply preferred devices.
    ///
    /// `on_status` is invoked on every status snapshot (use to `emit` to the UI).
    pub fn start(
        &self,
        api_key: String,
        model: Option<String>,
        on_status: impl Fn(StatusPicture) + Send + 'static,
    ) -> Result<(), String> {
        let mut handle_g = self.handle.lock().unwrap();
        let already_spawned = handle_g.is_some();
        info!(already_spawned, model = ?model, "AppState::start");

        if handle_g.is_none() {
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

            let config = PipelineConfig::with_defaults(
                api_key,
                model,
                SUPERTONE_SAMPLE_RATE,
                WAKEWORD_MODEL_BYTES.to_vec(),
            );

            let (engine, handle, status_rx) = Engine::spawn(config);
            *self._engine.lock().unwrap() = Some(engine);
            *handle_g = Some(handle.clone());
            info!("engine thread spawned");

            let status_cache = self.status.clone();
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

        // Apply any devices the user picked before/while Off. The engine now
        // processes Switch* while Armed, so these take effect immediately.
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

        info!("AppState::start complete");
        Ok(())
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
