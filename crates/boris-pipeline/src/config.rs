//! Host spawn configuration for [`crate::Engine`].
//!
//! Built from desktop args, `~/.boris` settings, and env overrides
//! (`OPENROUTER_*`, `BORIS_*`). Model dirs default under `~/.boris/models`.
use std::path::PathBuf;

use boris_agent::CapabilityPreset;

use crate::env_util::{env_opt, env_truthy, nonempty};
use crate::paths;
use crate::prompt::BORIS_SYSTEM_PROMPT;
use crate::settings::{self, AppSettings};

/// Explicit LLM / OpenRouter preferences passed into [`PipelineConfig::with_llm`].
///
/// Priority for each field (when building the final config):
/// explicit value → env → `config.toml` → engine default.
#[derive(Debug, Clone, Default)]
pub struct LlmPrefs {
    pub openrouter_api_key: String,
    /// Strong / primary model id (multi-step agent work).
    pub openrouter_model: Option<String>,
    /// Fast / cheap model id for simple turns.
    pub openrouter_fast_model: Option<String>,
    /// OpenRouter **model-provider** preference for strong (e.g. `coreweave`).
    pub openrouter_model_provider: Option<String>,
    /// OpenRouter model-provider preference for fast.
    pub openrouter_fast_provider: Option<String>,
    /// When `Some`, overrides pin; when `None`, falls back to env / saved.
    pub openrouter_pin_provider: Option<bool>,
}

impl LlmPrefs {
    pub fn new(openrouter_api_key: impl Into<String>) -> Self {
        Self {
            openrouter_api_key: openrouter_api_key.into(),
            ..Default::default()
        }
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.openrouter_model = Some(model.into());
        self
    }

    pub fn fast_model(mut self, model: impl Into<String>) -> Self {
        self.openrouter_fast_model = Some(model.into());
        self
    }

    pub fn model_provider(mut self, provider: impl Into<String>) -> Self {
        self.openrouter_model_provider = Some(provider.into());
        self
    }

    pub fn fast_provider(mut self, provider: impl Into<String>) -> Self {
        self.openrouter_fast_provider = Some(provider.into());
        self
    }

    pub fn pin_provider(mut self, pin: bool) -> Self {
        self.openrouter_pin_provider = Some(pin);
        self
    }
}

/// Host-supplied configuration for [`crate::Engine::spawn`].
///
/// Model dirs default to `~/.boris/models/...` (see [`crate::paths`]).
/// User prefs/secrets load from `config.toml` + `auth.json` via [`crate::settings`].
pub struct PipelineConfig {
    pub openrouter_api_key: String,
    /// Strong / primary model id (multi-step agent work).
    pub openrouter_model: Option<String>,
    /// Fast / cheap model id for simple turns. `None` → resolve from settings/env/default.
    pub openrouter_fast_model: Option<String>,
    /// OpenRouter **model-provider** preference for strong (e.g. `coreweave`).
    pub openrouter_model_provider: Option<String>,
    /// OpenRouter model-provider preference for fast.
    pub openrouter_fast_provider: Option<String>,
    /// When true, do not fall back to other OpenRouter hosts if preferred list fails.
    pub openrouter_pin_provider: bool,
    pub system_prompt: String,
    /// Rate of PCM passed to playback (must match TTS native rate).
    /// When `0` or unused, the engine prefers [`boris_inference::TextToSpeech::sample_rate`].
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
    /// Auto-allow moderate tools + trusted sandbox file writes.
    /// Shell and open URL still need yes (Dangerous/Critical HITL).
    pub trusted_auto_moderate: bool,
    /// Max HITL confirmations per user turn before remaining calls are denied.
    pub max_confirms_per_turn: u32,
}

impl PipelineConfig {
    /// Build with model paths under `~/.boris/models` and optional seed from workspace assets.
    ///
    /// Merges explicit args with `~/.boris/config.toml` + `auth.json` and env vars:
    /// - `OPENROUTER_MODEL` / `BORIS_STRONG_MODEL` — strong model
    /// - `BORIS_FAST_MODEL` — fast model
    /// - `BORIS_MODEL_PROVIDER` / `BORIS_STRONG_PROVIDER` — strong provider order
    /// - `BORIS_FAST_PROVIDER` — fast provider order
    /// - `BORIS_PIN_PROVIDER=1` — no fallbacks when provider list is set
    pub fn with_defaults(
        openrouter_api_key: String,
        openrouter_model: Option<String>,
        play_source_rate: u32,
        wakeword_model: Vec<u8>,
    ) -> Self {
        let mut prefs = LlmPrefs::new(openrouter_api_key);
        prefs.openrouter_model = openrouter_model;
        Self::with_llm(prefs, play_source_rate, wakeword_model)
    }

    /// Full LLM configuration (models + OpenRouter model-providers).
    pub fn with_llm(prefs: LlmPrefs, play_source_rate: u32, wakeword_model: Vec<u8>) -> Self {
        // Best-effort dev seed from workspace `assets/models` only.
        // Product path for clean installs is `download::install_models`.
        if let Err(e) = paths::bootstrap_models_if_needed() {
            tracing::warn!(error = %e, "model bootstrap into ~/.boris failed");
        }

        let home = paths::boris_home();
        tracing::info!(boris_home = %home.display(), "using Boris home");

        let saved = settings::load_settings().unwrap_or_default();
        let capability_preset = resolve_capability_preset(&saved);
        let long_term_memory = resolve_long_term_memory_flag(&saved);
        let trusted_auto_moderate = resolve_trusted_auto_moderate(&saved);
        let max_confirms_per_turn = resolve_max_confirms_per_turn(&saved);
        let tts_voice_id = resolve_tts_voice_id(&saved);

        let (strong, strong_prov, fast, fast_prov, pin) = resolve_llm_prefs(&saved, &prefs);

        Self {
            openrouter_api_key: prefs.openrouter_api_key,
            openrouter_model: strong,
            openrouter_fast_model: fast,
            openrouter_model_provider: strong_prov,
            openrouter_fast_provider: fast_prov,
            openrouter_pin_provider: pin,
            system_prompt: BORIS_SYSTEM_PROMPT.to_string(),
            play_source_rate,
            wakeword_model,
            mic_label: "Default mic".into(),
            speaker_label: "Default speaker".into(),
            stt_model_dir: paths::parakeet_dir(),
            tts_model_dir: paths::supertone_onnx_dir(),
            tts_voice_dir: paths::supertone_voices_dir(),
            tts_voice_id,
            capability_preset,
            long_term_memory,
            trusted_auto_moderate,
            max_confirms_per_turn,
        }
    }
}

/// Priority: explicit arg → env → config.toml → None (engine default).
fn resolve_llm_prefs(
    saved: &AppSettings,
    prefs: &LlmPrefs,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
) {
    let strong = first_nonempty([
        prefs.openrouter_model.clone(),
        env_opt("OPENROUTER_MODEL"),
        env_opt("BORIS_STRONG_MODEL"),
        nonempty(saved.openrouter_model.clone()),
    ]);
    let fast = first_nonempty([
        prefs.openrouter_fast_model.clone(),
        env_opt("BORIS_FAST_MODEL"),
        nonempty(saved.openrouter_fast_model.clone()),
        None,
    ]);
    let strong_prov = first_nonempty([
        prefs.openrouter_model_provider.clone(),
        env_opt("BORIS_MODEL_PROVIDER"),
        env_opt("BORIS_STRONG_PROVIDER"),
        nonempty(saved.openrouter_model_provider.clone()),
    ]);
    let fast_prov = first_nonempty([
        prefs.openrouter_fast_provider.clone(),
        env_opt("BORIS_FAST_PROVIDER"),
        nonempty(saved.openrouter_fast_provider.clone()),
        // Fall back to strong provider if only one is configured.
        strong_prov.clone(),
    ]);
    let pin = prefs.openrouter_pin_provider.unwrap_or_else(|| {
        env_truthy("BORIS_PIN_PROVIDER").unwrap_or(saved.openrouter_pin_provider)
    });
    (strong, strong_prov, fast, fast_prov, pin)
}

fn first_nonempty(candidates: [Option<String>; 4]) -> Option<String> {
    candidates.into_iter().flatten().next()
}

/// `BORIS_CAPABILITY` env, else config.toml `[capability]`, else Full.
fn resolve_capability_preset(saved: &AppSettings) -> CapabilityPreset {
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
    if !saved.capability_preset.trim().is_empty() {
        if let Some(p) = CapabilityPreset::parse(&saved.capability_preset) {
            tracing::info!(preset = p.as_str(), "capability from config.toml");
            return p;
        }
        tracing::warn!(
            value = %saved.capability_preset,
            "unknown capability_preset in config; using full"
        );
    }
    CapabilityPreset::Full
}

/// `BORIS_MEMORY` env overrides `config.toml` `[agent].long_term_memory`.
fn resolve_long_term_memory_flag(saved: &AppSettings) -> bool {
    env_truthy("BORIS_MEMORY").unwrap_or(saved.long_term_memory)
}

/// `BORIS_TRUSTED` env overrides `config.toml` `[agent].trusted_auto_moderate`.
fn resolve_trusted_auto_moderate(saved: &AppSettings) -> bool {
    env_truthy("BORIS_TRUSTED").unwrap_or(saved.trusted_auto_moderate)
}

/// Optional `BORIS_MAX_CONFIRMS` env overrides `config.toml` `[agent].max_confirms_per_turn`.
/// Clamped to at least 1; invalid/missing env falls back to saved (default 12).
fn resolve_max_confirms_per_turn(saved: &AppSettings) -> u32 {
    if let Some(raw) = env_opt("BORIS_MAX_CONFIRMS") {
        if let Ok(n) = raw.trim().parse::<u32>() {
            return n.max(1);
        }
        tracing::warn!(
            value = %raw,
            "invalid BORIS_MAX_CONFIRMS; using config/default"
        );
    }
    saved.max_confirms_per_turn.max(1)
}

/// Voice from config, default `M4`. Optional `BORIS_TTS_VOICE` override.
fn resolve_tts_voice_id(saved: &AppSettings) -> String {
    if let Some(v) = env_opt("BORIS_TTS_VOICE") {
        return v;
    }
    let t = saved.tts_voice_id.trim();
    if t.is_empty() {
        "M4".into()
    } else {
        t.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_nonempty_picks_first() {
        assert_eq!(
            first_nonempty([None, Some("a".into()), Some("b".into()), None]).as_deref(),
            Some("a")
        );
        assert_eq!(first_nonempty([None, None, None, None]), None);
    }

    #[test]
    fn resolve_llm_prefs_prefers_explicit_over_saved() {
        let saved = AppSettings {
            openrouter_model: "saved-strong".into(),
            openrouter_fast_model: "saved-fast".into(),
            openrouter_model_provider: "saved-prov".into(),
            openrouter_fast_provider: "saved-fast-prov".into(),
            openrouter_pin_provider: false,
            ..Default::default()
        };
        let prefs = LlmPrefs {
            openrouter_api_key: String::new(),
            openrouter_model: Some("arg-strong".into()),
            openrouter_fast_model: Some("arg-fast".into()),
            openrouter_model_provider: Some("arg-prov".into()),
            openrouter_fast_provider: Some("arg-fast-prov".into()),
            openrouter_pin_provider: Some(true),
        };
        let (strong, strong_p, fast, fast_p, pin) = resolve_llm_prefs(&saved, &prefs);
        assert_eq!(strong.as_deref(), Some("arg-strong"));
        assert_eq!(fast.as_deref(), Some("arg-fast"));
        assert_eq!(strong_p.as_deref(), Some("arg-prov"));
        assert_eq!(fast_p.as_deref(), Some("arg-fast-prov"));
        assert!(pin);
    }

    #[test]
    fn resolve_llm_prefs_uses_saved_when_args_empty() {
        let saved = AppSettings {
            openrouter_model: "saved-strong".into(),
            openrouter_fast_model: "saved-fast".into(),
            openrouter_model_provider: "only-strong-prov".into(),
            openrouter_fast_provider: String::new(),
            openrouter_pin_provider: true,
            ..Default::default()
        };
        let prefs = LlmPrefs::default();
        let (strong, strong_p, fast, fast_p, pin) = resolve_llm_prefs(&saved, &prefs);
        assert_eq!(strong.as_deref(), Some("saved-strong"));
        assert_eq!(fast.as_deref(), Some("saved-fast"));
        assert_eq!(strong_p.as_deref(), Some("only-strong-prov"));
        // fast provider falls back to strong when unset
        assert_eq!(fast_p.as_deref(), Some("only-strong-prov"));
        assert!(pin);
    }

    #[test]
    fn llm_prefs_builder_chain() {
        let p = LlmPrefs::new("sk")
            .model("m")
            .fast_model("f")
            .model_provider("p")
            .pin_provider(true);
        assert_eq!(p.openrouter_api_key, "sk");
        assert_eq!(p.openrouter_model.as_deref(), Some("m"));
        assert_eq!(p.openrouter_fast_model.as_deref(), Some("f"));
        assert_eq!(p.openrouter_model_provider.as_deref(), Some("p"));
        assert_eq!(p.openrouter_pin_provider, Some(true));
    }
}
