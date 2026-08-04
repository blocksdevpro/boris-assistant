use std::path::PathBuf;

use boris_agent::CapabilityPreset;

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
    /// Tool surface preset (VoiceSafe / LocalPower / Full).
    pub capability_preset: CapabilityPreset,
    /// Enable markdown long-term memory tools + session logs.
    pub long_term_memory: bool,
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

        let capability_preset = resolve_capability_preset();
        let long_term_memory = resolve_long_term_memory_flag();

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
            capability_preset,
            long_term_memory,
        }
    }
}

/// `BORIS_CAPABILITY` env, else settings.json, else Full.
fn resolve_capability_preset() -> CapabilityPreset {
    if let Ok(raw) = std::env::var("BORIS_CAPABILITY") {
        if let Some(p) = CapabilityPreset::parse(&raw) {
            tracing::info!(preset = p.as_str(), "capability from BORIS_CAPABILITY");
            return p;
        }
        tracing::warn!(
            value = %raw,
            "unknown BORIS_CAPABILITY; expected voice_safe|local_power|full"
        );
    }
    match crate::settings::load_settings() {
        Ok(s) if !s.capability_preset.trim().is_empty() => {
            if let Some(p) = CapabilityPreset::parse(&s.capability_preset) {
                tracing::info!(preset = p.as_str(), "capability from settings.json");
                return p;
            }
            tracing::warn!(
                value = %s.capability_preset,
                "unknown capability_preset in settings; using full"
            );
        }
        Ok(_) => {}
        Err(e) => tracing::debug!(error = %e, "settings load for capability skipped"),
    }
    CapabilityPreset::Full
}

/// `BORIS_MEMORY=0` disables; default on.
fn resolve_long_term_memory_flag() -> bool {
    match std::env::var("BORIS_MEMORY") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            !(v == "0" || v == "false" || v == "off" || v == "no")
        }
        Err(_) => true,
    }
}
