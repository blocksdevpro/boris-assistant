//! User prefs + secrets under `~/.boris`, Grok-style.
//!
//! | File | Role |
//! |------|------|
//! | `config.toml` | Prefs only (models, capability, audio, ui, logging) |
//! | `auth.json`   | Secrets only (`openrouter_api_key`, `exa_api_key`) |
//!
//! Desktop IPC still uses [`AppSettings`] (JSON-shaped). On disk we split like
//! `~/.grok/config.toml` + `~/.grok/auth.json`.
//!
//! Migration: if legacy `settings.json` exists, merge into the new pair and
//! rename the old file to `settings.json.bak`.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{PipelineError, Result};
use crate::paths::{self, auth_path, boris_home, config_path, legacy_settings_path};

/// Path to the prefs file (`~/.boris/config.toml`).
pub fn settings_path() -> PathBuf {
    config_path()
}

/// Path to secrets (`~/.boris/auth.json`).
pub fn secrets_path() -> PathBuf {
    auth_path()
}

fn default_true() -> bool {
    true
}

/// Product default when no model is saved (OpenRouter id).
pub const DEFAULT_OPENROUTER_MODEL: &str = "deepseek/deepseek-v4-flash-0731";

/// Product default model-provider when none is saved.
pub const DEFAULT_OPENROUTER_PROVIDER: &str = "digitalocean";

fn default_tts_voice() -> String {
    "M4".into()
}

/// Default HITL confirm budget per user turn (multi-tool friendly).
fn default_max_confirms_per_turn() -> u32 {
    12
}

fn default_overlay_caption_mode() -> String {
    "full".into()
}

fn default_overlay_position() -> String {
    "top_center".into()
}

fn default_overlay_scale_percent() -> u16 {
    100
}

fn default_update_channel() -> String {
    "stable".into()
}

fn normalize_update_channel(raw: String) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "beta" => "beta".into(),
        _ => default_update_channel(),
    }
}

/// Desktop prefs restored into the main window on launch.
///
/// **Model vs model-provider**
/// - `openrouter_model` / `openrouter_fast_model` — OpenRouter model ids
/// - `openrouter_model_provider` / `openrouter_fast_provider` — inference hosts
///
/// Disk layout: prefs in `config.toml`, API key in `auth.json`.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSettings {
    #[serde(default)]
    pub openrouter_api_key: String,
    /// Exa web search API key (also via `EXA_API_KEY` / `BORIS_EXA_API_KEY` env).
    #[serde(default)]
    pub exa_api_key: String,
    #[serde(default)]
    pub openrouter_model: String,
    #[serde(default)]
    pub openrouter_fast_model: String,
    #[serde(default)]
    pub openrouter_model_provider: String,
    #[serde(default)]
    pub openrouter_fast_provider: String,
    #[serde(default)]
    pub openrouter_pin_provider: bool,
    /// Tool capability preset: `full` | `local_power` | `voice_safe`.
    #[serde(default)]
    pub capability_preset: String,
    /// Preferred mic device id (`DeviceDto.id`), empty = OS default.
    #[serde(default)]
    pub input_device: String,
    /// Preferred speaker device id, empty = OS default.
    #[serde(default)]
    pub output_device: String,
    /// Supertone voice stem (e.g. `M4`).
    #[serde(default = "default_tts_voice")]
    pub tts_voice_id: String,
    /// Markdown long-term memory tools + session logs.
    #[serde(default = "default_true")]
    pub long_term_memory: bool,
    /// Auto-allow moderate tools + trusted sandbox file writes.
    /// Shell and open URL still need yes (Dangerous/Critical HITL).
    #[serde(default = "default_true")]
    pub trusted_auto_moderate: bool,
    /// Max HITL confirmations per user turn before remaining calls are denied.
    #[serde(default = "default_max_confirms_per_turn")]
    pub max_confirms_per_turn: u32,
    /// Show floating island when wake fires (host may honor later).
    #[serde(default)]
    pub show_overlay_on_wake: bool,
    /// Caption privacy: `full` | `assistant` | `hidden`.
    #[serde(default = "default_overlay_caption_mode")]
    pub overlay_caption_mode: String,
    /// Preferred monitor anchor: `top_center` | `top_left` | `top_right`.
    #[serde(default = "default_overlay_position")]
    pub overlay_position: String,
    /// Overlay scale percentage, clamped to 75..=125.
    #[serde(default = "default_overlay_scale_percent")]
    pub overlay_scale_percent: u16,
    /// Auto-start the engine when the desktop app opens.
    #[serde(default)]
    pub start_engine_on_launch: bool,
    /// Launch Boris at Windows sign-in (host writes the HKCU Run key).
    #[serde(default)]
    pub start_with_windows: bool,
    /// App-update feed: `stable` (GitHub latest) or `beta` (pre-release tag `beta`).
    #[serde(default = "default_update_channel")]
    pub update_channel: String,
    /// Optional log filter hint (`info`, `boris=debug`, …). Host may apply on restart.
    #[serde(default)]
    pub logging_filter: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            openrouter_api_key: String::new(),
            exa_api_key: String::new(),
            openrouter_model: String::new(),
            openrouter_fast_model: String::new(),
            openrouter_model_provider: String::new(),
            openrouter_fast_provider: String::new(),
            openrouter_pin_provider: false,
            capability_preset: String::new(),
            input_device: String::new(),
            output_device: String::new(),
            tts_voice_id: default_tts_voice(),
            long_term_memory: true,
            trusted_auto_moderate: true,
            max_confirms_per_turn: default_max_confirms_per_turn(),
            show_overlay_on_wake: false,
            overlay_caption_mode: default_overlay_caption_mode(),
            overlay_position: default_overlay_position(),
            overlay_scale_percent: default_overlay_scale_percent(),
            start_engine_on_launch: false,
            start_with_windows: false,
            update_channel: default_update_channel(),
            logging_filter: String::new(),
        }
    }
}

impl std::fmt::Debug for AppSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppSettings")
            .field(
                "openrouter_api_key",
                &if self.openrouter_api_key.is_empty() {
                    "<empty>"
                } else {
                    "<redacted>"
                },
            )
            .field(
                "exa_api_key",
                &if self.exa_api_key.is_empty() {
                    "<empty>"
                } else {
                    "<redacted>"
                },
            )
            .field("openrouter_model", &self.openrouter_model)
            .field("openrouter_fast_model", &self.openrouter_fast_model)
            .field("openrouter_model_provider", &self.openrouter_model_provider)
            .field("openrouter_fast_provider", &self.openrouter_fast_provider)
            .field("openrouter_pin_provider", &self.openrouter_pin_provider)
            .field("capability_preset", &self.capability_preset)
            .field("input_device", &self.input_device)
            .field("output_device", &self.output_device)
            .field("tts_voice_id", &self.tts_voice_id)
            .field("long_term_memory", &self.long_term_memory)
            .field("trusted_auto_moderate", &self.trusted_auto_moderate)
            .field("max_confirms_per_turn", &self.max_confirms_per_turn)
            .field("show_overlay_on_wake", &self.show_overlay_on_wake)
            .field("overlay_caption_mode", &self.overlay_caption_mode)
            .field("overlay_position", &self.overlay_position)
            .field("overlay_scale_percent", &self.overlay_scale_percent)
            .field("start_engine_on_launch", &self.start_engine_on_launch)
            .field("start_with_windows", &self.start_with_windows)
            .field("update_channel", &self.update_channel)
            .field("logging_filter", &self.logging_filter)
            .finish()
    }
}

// ── On-disk TOML / auth shapes (Grok-like) ───────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct ConfigFile {
    #[serde(default)]
    models: ModelsSection,
    #[serde(default)]
    capability: CapabilitySection,
    #[serde(default)]
    audio: AudioSection,
    #[serde(default)]
    speech: SpeechSection,
    #[serde(default)]
    agent: AgentSection,
    #[serde(default)]
    ui: UiSection,
    #[serde(default)]
    logging: LoggingSection,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct ModelsSection {
    #[serde(default)]
    strong: String,
    #[serde(default)]
    fast: String,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    fast_provider: String,
    #[serde(default)]
    pin_provider: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct CapabilitySection {
    #[serde(default)]
    preset: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct AudioSection {
    /// Device id or empty / `"default"` for OS default.
    #[serde(default)]
    input_device: String,
    #[serde(default)]
    output_device: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SpeechSection {
    #[serde(default = "default_tts_voice")]
    tts_voice_id: String,
}

impl Default for SpeechSection {
    fn default() -> Self {
        Self {
            tts_voice_id: default_tts_voice(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AgentSection {
    #[serde(default = "default_true")]
    long_term_memory: bool,
    #[serde(default = "default_true")]
    trusted_auto_moderate: bool,
    #[serde(default = "default_max_confirms_per_turn")]
    max_confirms_per_turn: u32,
}

impl Default for AgentSection {
    fn default() -> Self {
        Self {
            long_term_memory: true,
            trusted_auto_moderate: true,
            max_confirms_per_turn: default_max_confirms_per_turn(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct UiSection {
    #[serde(default)]
    show_overlay_on_wake: bool,
    #[serde(default = "default_overlay_caption_mode")]
    overlay_caption_mode: String,
    #[serde(default = "default_overlay_position")]
    overlay_position: String,
    #[serde(default = "default_overlay_scale_percent")]
    overlay_scale_percent: u16,
    #[serde(default)]
    start_engine_on_launch: bool,
    #[serde(default)]
    start_with_windows: bool,
    #[serde(default = "default_update_channel")]
    update_channel: String,
}

impl Default for UiSection {
    fn default() -> Self {
        Self {
            show_overlay_on_wake: false,
            overlay_caption_mode: default_overlay_caption_mode(),
            overlay_position: default_overlay_position(),
            overlay_scale_percent: default_overlay_scale_percent(),
            start_engine_on_launch: false,
            start_with_windows: false,
            update_channel: default_update_channel(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct LoggingSection {
    #[serde(default)]
    filter: String,
}

/// Apply managed TOML sections onto an [`AppSettings`] (mutates in place).
fn apply_config_file(out: &mut AppSettings, cfg: ConfigFile) {
    out.openrouter_model = cfg.models.strong;
    out.openrouter_fast_model = cfg.models.fast;
    out.openrouter_model_provider = cfg.models.provider;
    out.openrouter_fast_provider = cfg.models.fast_provider;
    out.openrouter_pin_provider = cfg.models.pin_provider;
    out.capability_preset = cfg.capability.preset;
    out.input_device = normalize_device_id(cfg.audio.input_device);
    out.output_device = normalize_device_id(cfg.audio.output_device);
    out.tts_voice_id = if cfg.speech.tts_voice_id.trim().is_empty() {
        default_tts_voice()
    } else {
        cfg.speech.tts_voice_id
    };
    out.long_term_memory = cfg.agent.long_term_memory;
    out.trusted_auto_moderate = cfg.agent.trusted_auto_moderate;
    out.max_confirms_per_turn = cfg.agent.max_confirms_per_turn.max(1);
    out.show_overlay_on_wake = cfg.ui.show_overlay_on_wake;
    out.overlay_caption_mode = normalize_overlay_caption_mode(cfg.ui.overlay_caption_mode);
    out.overlay_position = normalize_overlay_position(cfg.ui.overlay_position);
    out.overlay_scale_percent = cfg.ui.overlay_scale_percent.clamp(75, 125);
    out.start_engine_on_launch = cfg.ui.start_engine_on_launch;
    out.start_with_windows = cfg.ui.start_with_windows;
    out.update_channel = normalize_update_channel(cfg.ui.update_channel);
    out.logging_filter = cfg.logging.filter;
}

fn normalize_overlay_caption_mode(raw: String) -> String {
    match raw.trim() {
        "assistant" => "assistant".into(),
        "hidden" => "hidden".into(),
        _ => default_overlay_caption_mode(),
    }
}

fn normalize_overlay_position(raw: String) -> String {
    match raw.trim() {
        "top_left" => "top_left".into(),
        "top_right" => "top_right".into(),
        _ => default_overlay_position(),
    }
}

fn normalize_device_id(raw: String) -> String {
    let t = raw.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("default") {
        String::new()
    } else {
        t.to_string()
    }
}

fn device_for_toml(id: &str) -> String {
    let t = id.trim();
    if t.is_empty() {
        "default".into()
    } else {
        t.to_string()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct AuthFile {
    #[serde(default)]
    openrouter_api_key: String,
    /// Exa Search API key for `web_search`.
    #[serde(default)]
    exa_api_key: String,
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Load settings from disk. Missing files → defaults.
/// Migrates legacy `settings.json` on first successful load.
pub fn load_settings() -> Result<AppSettings> {
    paths::migrate_home_if_needed();
    migrate_legacy_settings_if_needed()?;

    let mut out = AppSettings::default();

    let cfg_path = config_path();
    if cfg_path.is_file() {
        let raw = fs::read_to_string(&cfg_path)
            .map_err(|e| PipelineError::settings(format!("read config.toml: {e}")))?;
        if !raw.trim().is_empty() {
            // Reject the *old* dead config.toml that pointed at workspace models.
            if looks_like_legacy_config_toml(&raw) {
                tracing::warn!(
                    path = %cfg_path.display(),
                    "legacy config.toml (model paths) ignored; use settings migration"
                );
            } else {
                let cfg: ConfigFile = toml::from_str(&raw)
                    .map_err(|e| PipelineError::settings(format!("parse config.toml: {e}")))?;
                apply_config_file(&mut out, cfg);
            }
        }
    }

    let auth_p = auth_path();
    if auth_p.is_file() {
        let raw = fs::read_to_string(&auth_p)
            .map_err(|e| PipelineError::settings(format!("read auth.json: {e}")))?;
        if !raw.trim().is_empty() {
            let auth: AuthFile = serde_json::from_str(&raw)
                .map_err(|e| PipelineError::settings(format!("parse auth.json: {e}")))?;
            out.openrouter_api_key = auth.openrouter_api_key;
            out.exa_api_key = auth.exa_api_key;
        }
    }

    Ok(out)
}

/// Persist prefs to `config.toml` and secrets to `auth.json`.
///
/// **Merge policy:** managed sections (`models`, `capability`, `audio`,
/// `speech`, `agent`, `ui`, `logging`) are rewritten from [`AppSettings`].
/// Unknown root tables and unknown keys *outside* those sections are preserved.
pub fn save_settings(settings: &AppSettings) -> Result<()> {
    paths::migrate_home_if_needed();
    let home = boris_home();
    fs::create_dir_all(&home)
        .map_err(|e| PipelineError::settings(format!("create ~/.boris: {e}")))?;

    // If an old path-style config.toml exists, archive it once.
    archive_legacy_config_toml_if_needed()?;

    let mut root = load_config_value_for_merge()?;

    let models = ModelsSection {
        strong: settings.openrouter_model.clone(),
        fast: settings.openrouter_fast_model.clone(),
        provider: settings.openrouter_model_provider.clone(),
        fast_provider: settings.openrouter_fast_provider.clone(),
        pin_provider: settings.openrouter_pin_provider,
    };
    let capability = CapabilitySection {
        preset: settings.capability_preset.clone(),
    };
    let audio = AudioSection {
        input_device: device_for_toml(&settings.input_device),
        output_device: device_for_toml(&settings.output_device),
    };
    let speech = SpeechSection {
        tts_voice_id: if settings.tts_voice_id.trim().is_empty() {
            default_tts_voice()
        } else {
            settings.tts_voice_id.trim().to_string()
        },
    };
    let agent = AgentSection {
        long_term_memory: settings.long_term_memory,
        trusted_auto_moderate: settings.trusted_auto_moderate,
        max_confirms_per_turn: settings.max_confirms_per_turn.max(1),
    };
    let ui = UiSection {
        show_overlay_on_wake: settings.show_overlay_on_wake,
        overlay_caption_mode: normalize_overlay_caption_mode(settings.overlay_caption_mode.clone()),
        overlay_position: normalize_overlay_position(settings.overlay_position.clone()),
        overlay_scale_percent: settings.overlay_scale_percent.clamp(75, 125),
        start_engine_on_launch: settings.start_engine_on_launch,
        start_with_windows: settings.start_with_windows,
        update_channel: normalize_update_channel(settings.update_channel.clone()),
    };
    let logging = LoggingSection {
        filter: settings.logging_filter.clone(),
    };

    let models_val = toml::Value::try_from(&models)
        .map_err(|e| PipelineError::settings(format!("serialize models: {e}")))?;
    let capability_val = toml::Value::try_from(&capability)
        .map_err(|e| PipelineError::settings(format!("serialize capability: {e}")))?;
    let audio_val = toml::Value::try_from(&audio)
        .map_err(|e| PipelineError::settings(format!("serialize audio: {e}")))?;
    let speech_val = toml::Value::try_from(&speech)
        .map_err(|e| PipelineError::settings(format!("serialize speech: {e}")))?;
    let agent_val = toml::Value::try_from(&agent)
        .map_err(|e| PipelineError::settings(format!("serialize agent: {e}")))?;
    let ui_val = toml::Value::try_from(&ui)
        .map_err(|e| PipelineError::settings(format!("serialize ui: {e}")))?;
    let logging_val = toml::Value::try_from(&logging)
        .map_err(|e| PipelineError::settings(format!("serialize logging: {e}")))?;

    let table = root
        .as_table_mut()
        .ok_or_else(|| PipelineError::settings("config.toml root is not a table"))?;
    table.insert("models".into(), models_val);
    table.insert("capability".into(), capability_val);
    table.insert("audio".into(), audio_val);
    table.insert("speech".into(), speech_val);
    table.insert("agent".into(), agent_val);
    table.insert("ui".into(), ui_val);
    // Only write [logging] when the user set a filter (avoid noise on fresh installs).
    if !settings.logging_filter.trim().is_empty() {
        table.insert("logging".into(), logging_val);
    }

    let toml_body = toml::to_string_pretty(&root)
        .map_err(|e| PipelineError::settings(format!("serialize config.toml: {e}")))?;
    let mut body = String::from(
        "# Boris user config (prefs only — secrets live in auth.json)\n\
         # Managed: [models], [capability], [audio], [speech], [agent], [ui]\n\
         # Optional: [logging]  ·  unknown tables are preserved on save\n\n",
    );
    body.push_str(&toml_body);

    write_atomic(&config_path(), body.as_bytes(), false)
        .map_err(|e| PipelineError::settings(format!("write config.toml: {e}")))?;

    let auth = AuthFile {
        openrouter_api_key: settings.openrouter_api_key.clone(),
        exa_api_key: settings.exa_api_key.clone(),
    };
    let json = serde_json::to_string_pretty(&auth)
        .map_err(|e| PipelineError::settings(format!("serialize auth.json: {e}")))?;
    // Secrets: owner-read/write only on Unix (mode applied to temp + final path).
    write_atomic(&auth_path(), json.as_bytes(), true)
        .map_err(|e| PipelineError::settings(format!("write auth.json: {e}")))?;

    tracing::debug!(
        config = %config_path().display(),
        auth = %auth_path().display(),
        "settings saved (config.toml + auth.json)"
    );
    Ok(())
}

/// Load existing config as a TOML value for merge, or an empty table.
fn load_config_value_for_merge() -> Result<toml::Value> {
    let path = config_path();
    if !path.is_file() {
        return Ok(toml::Value::Table(toml::map::Map::new()));
    }
    let raw = fs::read_to_string(&path)
        .map_err(|e| PipelineError::settings(format!("read config.toml: {e}")))?;
    if raw.trim().is_empty() || looks_like_legacy_config_toml(&raw) {
        return Ok(toml::Value::Table(toml::map::Map::new()));
    }
    match raw.parse::<toml::Value>() {
        Ok(v) if v.is_table() => Ok(v),
        Ok(_) => Ok(toml::Value::Table(toml::map::Map::new())),
        Err(e) => {
            tracing::warn!(error = %e, "config.toml parse failed on save merge; rewriting managed sections only");
            Ok(toml::Value::Table(toml::map::Map::new()))
        }
    }
}

/// One-shot: `settings.json` → config.toml + auth.json, then `.bak`.
fn migrate_legacy_settings_if_needed() -> Result<()> {
    let legacy = legacy_settings_path();
    if !legacy.is_file() {
        return Ok(());
    }
    // Already have new files with content? Still merge key if auth empty.
    let raw = fs::read_to_string(&legacy)
        .map_err(|e| PipelineError::settings(format!("read legacy settings: {e}")))?;
    if raw.trim().is_empty() {
        return Ok(());
    }
    let old: AppSettings = serde_json::from_str(&raw)
        .map_err(|e| PipelineError::settings(format!("parse legacy settings.json: {e}")))?;

    // If new config already exists and is non-legacy, only fill missing auth key.
    let cfg_exists = config_path().is_file()
        && fs::read_to_string(config_path())
            .map(|s| !s.trim().is_empty() && !looks_like_legacy_config_toml(&s))
            .unwrap_or(false);
    let auth_has_key = auth_path().is_file()
        && fs::read_to_string(auth_path())
            .ok()
            .and_then(|s| serde_json::from_str::<AuthFile>(&s).ok())
            .map(|a| !a.openrouter_api_key.trim().is_empty())
            .unwrap_or(false);

    if cfg_exists && auth_has_key {
        rename_legacy_settings(&legacy);
        return Ok(());
    }

    let mut merged = if cfg_exists {
        load_settings_raw_no_migrate().unwrap_or_default()
    } else {
        AppSettings::default()
    };

    if merged.openrouter_model.is_empty() {
        merged.openrouter_model = old.openrouter_model;
    }
    if merged.openrouter_fast_model.is_empty() {
        merged.openrouter_fast_model = old.openrouter_fast_model;
    }
    if merged.openrouter_model_provider.is_empty() {
        merged.openrouter_model_provider = old.openrouter_model_provider;
    }
    if merged.openrouter_fast_provider.is_empty() {
        merged.openrouter_fast_provider = old.openrouter_fast_provider;
    }
    if !merged.openrouter_pin_provider {
        merged.openrouter_pin_provider = old.openrouter_pin_provider;
    }
    if merged.capability_preset.is_empty() {
        merged.capability_preset = old.capability_preset;
    }
    if merged.openrouter_api_key.is_empty() {
        merged.openrouter_api_key = old.openrouter_api_key;
    }

    save_settings(&merged)?;
    rename_legacy_settings(&legacy);
    tracing::info!("migrated settings.json → config.toml + auth.json");
    Ok(())
}

fn load_settings_raw_no_migrate() -> Result<AppSettings> {
    let mut out = AppSettings::default();
    if config_path().is_file() {
        let raw = fs::read_to_string(config_path()).map_err(PipelineError::from)?;
        if !looks_like_legacy_config_toml(&raw) {
            if let Ok(cfg) = toml::from_str::<ConfigFile>(&raw) {
                apply_config_file(&mut out, cfg);
            }
        }
    }
    if auth_path().is_file() {
        let raw = fs::read_to_string(auth_path()).map_err(PipelineError::from)?;
        if let Ok(auth) = serde_json::from_str::<AuthFile>(&raw) {
            out.openrouter_api_key = auth.openrouter_api_key;
            out.exa_api_key = auth.exa_api_key;
        }
    }
    Ok(out)
}

fn rename_legacy_settings(legacy: &std::path::Path) {
    let bak = legacy.with_extension("json.bak");
    if let Err(e) = fs::rename(legacy, &bak) {
        tracing::warn!(error = %e, "could not rename settings.json → .bak");
    }
}

/// Old product config.toml had `[api]` / workspace model paths — not our schema.
fn looks_like_legacy_config_toml(raw: &str) -> bool {
    raw.contains("wakeword_path")
        || raw.contains("stt_dir")
        || raw.contains("tts_dir")
        || (raw.contains("[api]") && raw.contains("chat_model"))
}

fn archive_legacy_config_toml_if_needed() -> Result<()> {
    let path = config_path();
    if !path.is_file() {
        return Ok(());
    }
    let raw = fs::read_to_string(&path)
        .map_err(|e| PipelineError::settings(format!("read config: {e}")))?;
    if !looks_like_legacy_config_toml(&raw) {
        return Ok(());
    }
    let bak = boris_home().join("config.legacy.toml.bak");
    if let Err(e) = fs::rename(&path, &bak) {
        tracing::warn!(error = %e, "could not archive legacy config.toml");
    } else {
        tracing::info!(path = %bak.display(), "archived legacy config.toml");
    }
    Ok(())
}

/// Write `bytes` via a sibling `*.tmp`, then replace `path` without an
/// unlink-first crash window when the platform allows it.
///
/// - **Unix:** `rename(tmp, path)` atomically replaces an existing file.
///   When `private`, temp and final files get mode `0o600` (auth.json).
/// - **Windows:** move existing aside to `*.replace.bak`, rename temp in,
///   then remove the backup (never delete the only copy first).
/// - **Fallback:** direct write to `path` if rename fails.
fn write_atomic(path: &std::path::Path, bytes: &[u8], private: bool) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    write_temp_file(&tmp, bytes, private)?;

    #[cfg(unix)]
    {
        match fs::rename(&tmp, path) {
            Ok(()) => {
                if private {
                    set_owner_secret_mode(path)?;
                }
                Ok(())
            }
            Err(_e) => {
                let _ = fs::remove_file(&tmp);
                write_direct(path, bytes, private)
            }
        }
    }

    #[cfg(windows)]
    {
        // Never unlink the destination first: move it aside, then promote temp.
        let bak = path.with_extension("replace.bak");
        if path.exists() {
            let _ = fs::remove_file(&bak); // leftover from a prior crashed replace
            if let Err(_e) = fs::rename(path, &bak) {
                // Could not park old file; fall back without leaving a hole.
                let _ = fs::remove_file(&tmp);
                return write_direct(path, bytes, private);
            }
        }
        match fs::rename(&tmp, path) {
            Ok(()) => {
                let _ = fs::remove_file(&bak);
                if private {
                    restrict_to_current_user_windows(path);
                }
                Ok(())
            }
            Err(_e) => {
                // Restore previous content if we moved it aside.
                if bak.exists() && !path.exists() {
                    let _ = fs::rename(&bak, path);
                } else {
                    let _ = fs::remove_file(&bak);
                }
                let _ = fs::remove_file(&tmp);
                write_direct(path, bytes, private)
            }
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        // Best-effort: try rename over existing; fall back to direct write.
        match fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(_e) => {
                let _ = fs::remove_file(&tmp);
                write_direct(path, bytes, private)
            }
        }
    }
}

fn write_temp_file(tmp: &std::path::Path, bytes: &[u8], private: bool) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        if private {
            opts.mode(0o600);
        }
        let mut f = opts.open(tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        if private {
            set_owner_secret_mode(tmp)?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = private;
        let mut f = fs::File::create(tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    }
}

fn write_direct(path: &std::path::Path, bytes: &[u8], private: bool) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        if private {
            opts.mode(0o600);
        }
        let mut f = opts.open(path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        if private {
            set_owner_secret_mode(path)?;
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        let mut f = fs::File::create(path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        if private {
            restrict_to_current_user_windows(path);
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = private;
        let mut f = fs::File::create(path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    }
}

#[cfg(unix)]
fn set_owner_secret_mode(path: &std::path::Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

/// Windows analog of the Unix `0o600` intent: restrict `auth.json` (plaintext
/// API keys) to the current user only.
///
/// Best-effort, defense-in-depth — mirrors `set_owner_secret_mode` but Windows
/// has no POSIX mode bits, so this shells out to `icacls` to strip inherited
/// ACEs and grant Full Control to the current user exclusively:
/// `icacls <path> /inheritance:r /grant:r "<user>":F`.
/// Spawned with `CREATE_NO_WINDOW` so the packaged GUI does not flash a
/// console (tauri-apps/discussions#11446).
///
/// Never fails the settings save: on any error (missing `icacls`, no)
/// permission, etc.) this just logs a warning and leaves the file's default
/// ACL in place. Only call this for files that actually hold secrets — not
/// every settings file.
#[cfg(windows)]
fn restrict_to_current_user_windows(path: &std::path::Path) {
    let user = std::env::var("USERNAME").unwrap_or_default();
    if user.trim().is_empty() {
        tracing::warn!(
            path = %path.display(),
            "USERNAME env var unavailable; skipping Windows ACL restriction on secrets file"
        );
        return;
    }
    let grant = format!("{user}:F");
    let mut cmd = std::process::Command::new("icacls");
    cmd.arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(&grant);
    // GUI / packaged exe: `icacls` is a console app. Without CREATE_NO_WINDOW
    // Windows allocates a console for the child and you get the empty
    // title-bar flash on every settings save and on first-run auth write.
    // Same fix as tauri-apps/discussions#11446 / #9719.
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let result = cmd.output();
    match result {
        Ok(out) if out.status.success() => {
            tracing::debug!(path = %path.display(), "restricted secrets file ACL to current user");
        }
        Ok(out) => {
            tracing::warn!(
                path = %path.display(),
                status = %out.status,
                stderr = %String::from_utf8_lossy(&out.stderr),
                "icacls did not fully succeed restricting secrets file ACL"
            );
        }
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to run icacls to restrict secrets file ACL (best-effort, continuing)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize tests that touch BORIS_HOME.
    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn settings_path_is_config_toml() {
        let p = settings_path();
        assert!(
            p.ends_with("config.toml"),
            "unexpected path {}",
            p.display()
        );
    }

    #[test]
    fn default_deserializes_empty_object() {
        let s: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, AppSettings::default());
        assert_eq!(s.update_channel, "stable");
    }

    #[test]
    fn update_channel_normalizes_to_stable_or_beta() {
        assert_eq!(normalize_update_channel("beta".into()), "beta");
        assert_eq!(normalize_update_channel("BETA".into()), "beta");
        assert_eq!(normalize_update_channel(" stable ".into()), "stable");
        assert_eq!(normalize_update_channel("nightly".into()), "stable");
        assert_eq!(normalize_update_channel(String::new()), "stable");
    }

    #[test]
    fn roundtrip_config_and_auth() {
        let _g = LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "boris-settings-rt-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        std::env::set_var(paths::BORIS_HOME_ENV, &dir);

        let s = AppSettings {
            openrouter_api_key: "sk-test".into(),
            exa_api_key: "exa-test".into(),
            openrouter_model: "google/gemini-2.5-flash-lite".into(),
            openrouter_fast_model: "fast-model".into(),
            openrouter_model_provider: "coreweave".into(),
            openrouter_fast_provider: "baseten".into(),
            openrouter_pin_provider: true,
            capability_preset: "voice_safe".into(),
            input_device: "mic-1".into(),
            output_device: "spk-1".into(),
            tts_voice_id: "M4".into(),
            long_term_memory: false,
            trusted_auto_moderate: false,
            max_confirms_per_turn: 8,
            show_overlay_on_wake: true,
            overlay_caption_mode: "assistant".into(),
            overlay_position: "top_right".into(),
            overlay_scale_percent: 115,
            start_engine_on_launch: true,
            start_with_windows: true,
            update_channel: "beta".into(),
            logging_filter: "info".into(),
        };
        save_settings(&s).expect("save");
        assert!(config_path().is_file());
        assert!(auth_path().is_file());

        let raw_cfg = fs::read_to_string(config_path()).unwrap();
        assert!(raw_cfg.contains("[models]"));
        assert!(raw_cfg.contains("strong"));
        assert!(raw_cfg.contains("[audio]"));
        assert!(raw_cfg.contains("[speech]"));
        assert!(raw_cfg.contains("[agent]"));
        assert!(raw_cfg.contains("max_confirms_per_turn"));
        assert!(raw_cfg.contains("[ui]"));
        assert!(raw_cfg.contains("[logging]"));
        assert!(
            !raw_cfg.contains("sk-test"),
            "key must not be in config.toml"
        );

        let raw_auth = fs::read_to_string(auth_path()).unwrap();
        assert!(raw_auth.contains("sk-test"));
        assert!(raw_auth.contains("openrouter_api_key"));
        assert!(raw_auth.contains("exa-test"));
        assert!(raw_auth.contains("exa_api_key"));

        let loaded = load_settings().expect("load");
        assert_eq!(loaded.openrouter_api_key, "sk-test");
        assert_eq!(loaded.exa_api_key, "exa-test");
        assert_eq!(loaded.openrouter_model, "google/gemini-2.5-flash-lite");
        assert_eq!(loaded.openrouter_fast_model, "fast-model");
        assert_eq!(loaded.openrouter_model_provider, "coreweave");
        assert!(loaded.openrouter_pin_provider);
        assert_eq!(loaded.capability_preset, "voice_safe");
        assert_eq!(loaded.input_device, "mic-1");
        assert_eq!(loaded.output_device, "spk-1");
        assert_eq!(loaded.tts_voice_id, "M4");
        assert!(!loaded.long_term_memory);
        assert!(!loaded.trusted_auto_moderate);
        assert_eq!(loaded.max_confirms_per_turn, 8);
        assert!(loaded.show_overlay_on_wake);
        assert_eq!(loaded.overlay_caption_mode, "assistant");
        assert_eq!(loaded.overlay_position, "top_right");
        assert_eq!(loaded.overlay_scale_percent, 115);
        assert!(loaded.start_engine_on_launch);
        assert!(loaded.start_with_windows);
        assert_eq!(loaded.update_channel, "beta");
        assert_eq!(loaded.logging_filter, "info");
        assert!(raw_cfg.contains("update_channel"));

        // Unknown root tables must survive a subsequent save.
        let mut raw_cfg = fs::read_to_string(config_path()).unwrap();
        raw_cfg.push_str("\n[custom]\nfoo = 1\n");
        fs::write(config_path(), &raw_cfg).unwrap();
        save_settings(&loaded).expect("re-save");
        let after = fs::read_to_string(config_path()).unwrap();
        assert!(after.contains("[custom]"), "unknown section wiped: {after}");
        assert!(after.contains("foo"), "unknown key wiped: {after}");
        assert!(after.contains("mic-1"), "managed audio lost: {after}");

        std::env::remove_var(paths::BORIS_HOME_ENV);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_writes_managed_sections_skips_empty_logging() {
        let _g = LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "boris-settings-managed-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        std::env::set_var(paths::BORIS_HOME_ENV, &dir);

        let s = AppSettings {
            openrouter_model: "m".into(),
            ..Default::default()
        };
        save_settings(&s).expect("save");
        let raw = fs::read_to_string(config_path()).unwrap();
        assert!(raw.contains("[models]"));
        assert!(raw.lines().any(|l| l.trim() == "[audio]"));
        assert!(raw.lines().any(|l| l.trim() == "[speech]"));
        assert!(raw.lines().any(|l| l.trim() == "[agent]"));
        assert!(raw.lines().any(|l| l.trim() == "[ui]"));
        // Empty logging filter must not invent [logging].
        assert!(
            !raw.lines().any(|l| l.trim() == "[logging]"),
            "should not invent [logging] when filter empty: {raw}"
        );

        std::env::remove_var(paths::BORIS_HOME_ENV);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrates_settings_json() {
        let _g = LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "boris-settings-mig-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        std::env::set_var(paths::BORIS_HOME_ENV, &dir);

        let legacy = AppSettings {
            openrouter_api_key: "sk-legacy".into(),
            openrouter_model: "m1".into(),
            ..Default::default()
        };
        fs::write(
            legacy_settings_path(),
            serde_json::to_string_pretty(&legacy).unwrap(),
        )
        .unwrap();

        let loaded = load_settings().expect("load migrates");
        assert_eq!(loaded.openrouter_api_key, "sk-legacy");
        assert_eq!(loaded.openrouter_model, "m1");
        assert!(auth_path().is_file());
        assert!(config_path().is_file());
        assert!(!legacy_settings_path().is_file());
        assert!(dir.join("settings.json.bak").is_file());

        std::env::remove_var(paths::BORIS_HOME_ENV);
        let _ = fs::remove_dir_all(&dir);
    }
}
