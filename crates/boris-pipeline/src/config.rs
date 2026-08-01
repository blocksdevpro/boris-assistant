use std::path::PathBuf;

use crate::paths;
use crate::prompt::BORIS_SYSTEM_PROMPT;

/// Host-supplied configuration for [`crate::Engine::spawn`].
///
/// Model dirs default to `~/.boris/models/...` (see [`crate::paths`]).
/// Does **not** read `config.toml`.
pub struct PipelineConfig {
    pub openrouter_api_key: String,
    pub openrouter_model: Option<String>,
    pub system_prompt: String,
    /// Rate of PCM passed to playback (must match TTS native rate).
    pub play_source_rate: u32,
    /// Wake-word ONNX model bytes (host may embed or load from disk).
    pub wakeword_model: Vec<u8>,
    pub mic_label: String,
    pub speaker_label: String,
    /// Parakeet model directory.
    pub stt_model_dir: PathBuf,
    /// Supertone onnx directory.
    pub tts_model_dir: PathBuf,
    /// Supertone voices directory.
    pub tts_voice_dir: PathBuf,
    /// Voice id (filename stem), e.g. `M4`.
    pub tts_voice_id: String,
}

impl PipelineConfig {
    /// Build with model paths under `~/.boris/models` and optional seed from workspace assets.
    pub fn with_defaults(
        openrouter_api_key: String,
        openrouter_model: Option<String>,
        play_source_rate: u32,
        wakeword_model: Vec<u8>,
    ) -> Self {
        // Best-effort dev seed from workspace `assets/models` only.
        // Product path for clean installs is `download::install_models`.
        if let Err(e) = paths::bootstrap_models_if_needed() {
            tracing::warn!(error = %e, "model bootstrap into ~/.boris failed");
        }

        let home = paths::boris_home();
        tracing::info!(boris_home = %home.display(), "using Boris home");

        Self {
            openrouter_api_key,
            openrouter_model,
            system_prompt: BORIS_SYSTEM_PROMPT.to_string(),
            play_source_rate,
            wakeword_model,
            mic_label: "Default mic".into(),
            speaker_label: "Default speaker".into(),
            stt_model_dir: paths::parakeet_dir(),
            tts_model_dir: paths::supertone_onnx_dir(),
            tts_voice_dir: paths::supertone_voices_dir(),
            tts_voice_id: "M4".into(),
        }
    }
}
