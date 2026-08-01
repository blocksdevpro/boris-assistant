//! Desktop host for [`boris_pipeline::Engine`].
//!
//! Owns lifecycle (start/stop), mirrors status for the UI, and maps Tauri
//! commands onto the engine. No voice policy lives here.

use std::sync::{Arc, Mutex};
use std::thread;

use boris_pipeline::{
    devices, DeviceDto, Engine, EngineCommand, EngineHandle, PipelineConfig, StatusPicture,
};
use boris_tts_supertone::SUPERTONE_SAMPLE_RATE;

/// Embedded wake model (path relative to this crate → workspace `assets/`).
static WAKEWORD_MODEL_BYTES: &[u8] =
    include_bytes!("../../../assets/models/livekit/boris-large.onnx");

/// Shared app state: engine handle + latest status snapshot.
pub struct AppState {
    status: Arc<Mutex<StatusPicture>>,
    handle: Mutex<Option<EngineHandle>>,
    /// Keep the engine join-handle alive for the process lifetime.
    _engine: Mutex<Option<Engine>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            status: Arc::new(Mutex::new(StatusPicture::off())),
            handle: Mutex::new(None),
            _engine: Mutex::new(None),
        }
    }

    pub fn status(&self) -> StatusPicture {
        self.status.lock().unwrap().clone()
    }

    /// Spawn the engine on first call, then send Start.
    ///
    /// `on_status` is invoked on every status snapshot (use to `emit` to the UI).
    pub fn start(
        &self,
        api_key: String,
        model: Option<String>,
        on_status: impl Fn(StatusPicture) + Send + 'static,
    ) -> Result<(), String> {
        let mut handle_g = self.handle.lock().unwrap();

        if handle_g.is_none() {
            if api_key.trim().is_empty() {
                return Err("API key is required".into());
            }

            let config = PipelineConfig::with_defaults(
                api_key,
                model,
                SUPERTONE_SAMPLE_RATE,
                WAKEWORD_MODEL_BYTES.to_vec(),
            );

            let (engine, handle, status_rx) = Engine::spawn(config);
            *self._engine.lock().unwrap() = Some(engine);
            *handle_g = Some(handle.clone());

            let status_cache = self.status.clone();
            thread::spawn(move || {
                while let Ok(picture) = status_rx.recv() {
                    *status_cache.lock().unwrap() = picture.clone();
                    on_status(picture);
                }
            });
        }

        let handle = handle_g
            .as_ref()
            .ok_or_else(|| "engine missing".to_string())?
            .clone();
        drop(handle_g);

        handle
            .start()
            .map_err(|e| format!("failed to start engine: {e}"))
    }

    pub fn stop(&self) -> Result<(), String> {
        let handle_g = self.handle.lock().unwrap();
        if let Some(handle) = handle_g.as_ref() {
            handle
                .stop()
                .map_err(|e| format!("failed to stop engine: {e}"))?;
        }
        Ok(())
    }

    pub fn switch_input(&self, device_id: String) -> Result<(), String> {
        let handle_g = self.handle.lock().unwrap();
        let handle = handle_g
            .as_ref()
            .ok_or_else(|| "engine not started".to_string())?;
        handle
            .send(EngineCommand::SwitchInput { device_id })
            .map_err(|e| e.to_string())
    }

    pub fn switch_output(&self, device_id: String) -> Result<(), String> {
        let handle_g = self.handle.lock().unwrap();
        let handle = handle_g
            .as_ref()
            .ok_or_else(|| "engine not started".to_string())?;
        handle
            .send(EngineCommand::SwitchOutput { device_id })
            .map_err(|e| e.to_string())
    }

    pub fn list_inputs() -> Vec<DeviceDto> {
        devices::list_input_devices()
    }

    pub fn list_outputs() -> Vec<DeviceDto> {
        devices::list_output_devices()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
