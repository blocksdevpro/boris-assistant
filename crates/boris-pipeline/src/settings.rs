//! User prefs + secrets under `~/.boris`, Grok-style.
//!
//! | File | Role |
//! |------|------|
//! | `config.toml` | Prefs only (models, capability, audio, ui, logging) |
//! | `auth.json`   | Secrets only (`openrouter_api_key`) |
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

/// Desktop connection settings restored into the main window on launch.
///
/// **Model vs model-provider**
/// - `openrouter_model` / `openrouter_fast_model` — OpenRouter model ids
/// - `openrouter_model_provider` / `openrouter_fast_provider` — inference hosts
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSettings {
    #[serde(default)]
    pub openrouter_api_key: String,
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
            .field("openrouter_model", &self.openrouter_model)
            .field("openrouter_fast_model", &self.openrouter_fast_model)
            .field("openrouter_model_provider", &self.openrouter_model_provider)
            .field("openrouter_fast_provider", &self.openrouter_fast_provider)
            .field("openrouter_pin_provider", &self.openrouter_pin_provider)
            .field("capability_preset", &self.capability_preset)
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
    #[serde(default = "default_device")]
    input_device: String,
    #[serde(default = "default_device")]
    output_device: String,
}

fn default_device() -> String {
    "default".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct UiSection {
    #[serde(default)]
    show_overlay_on_wake: bool,
    #[serde(default)]
    start_engine_on_launch: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct LoggingSection {
    #[serde(default)]
    filter: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct AuthFile {
    #[serde(default)]
    openrouter_api_key: String,
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
                out.openrouter_model = cfg.models.strong;
                out.openrouter_fast_model = cfg.models.fast;
                out.openrouter_model_provider = cfg.models.provider;
                out.openrouter_fast_provider = cfg.models.fast_provider;
                out.openrouter_pin_provider = cfg.models.pin_provider;
                out.capability_preset = cfg.capability.preset;
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
        }
    }

    Ok(out)
}

/// Persist prefs to `config.toml` and secrets to `auth.json`.
///
/// **Merge policy:** only `[models]` and `[capability]` are written from
/// [`AppSettings`]. Existing `[audio]`, `[ui]`, `[logging]`, and any unknown
/// tables/keys in `config.toml` are preserved so hand-edits are not wiped.
/// Fresh files get only the sections we own (no hard-coded audio/ui/logging).
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

    let models_val = toml::Value::try_from(&models)
        .map_err(|e| PipelineError::settings(format!("serialize models: {e}")))?;
    let capability_val = toml::Value::try_from(&capability)
        .map_err(|e| PipelineError::settings(format!("serialize capability: {e}")))?;

    let table = root.as_table_mut().ok_or_else(|| {
        PipelineError::settings("config.toml root is not a table")
    })?;
    table.insert("models".into(), models_val);
    table.insert("capability".into(), capability_val);
    // Intentionally do not insert/overwrite audio, ui, logging, or unknown keys.

    let toml_body = toml::to_string_pretty(&root)
        .map_err(|e| PipelineError::settings(format!("serialize config.toml: {e}")))?;
    // Header comment like a hand-edited Grok config.
    let mut body = String::from(
        "# Boris user config (prefs only — secrets live in auth.json)\n\
         # Managed sections: [models], [capability]\n\
         # Hand-edit free: [audio], [ui], [logging], and any extra tables\n\n",
    );
    body.push_str(&toml_body);

    write_atomic(&config_path(), body.as_bytes())
        .map_err(|e| PipelineError::settings(format!("write config.toml: {e}")))?;

    let auth = AuthFile {
        openrouter_api_key: settings.openrouter_api_key.clone(),
    };
    let json = serde_json::to_string_pretty(&auth)
        .map_err(|e| PipelineError::settings(format!("serialize auth.json: {e}")))?;
    write_atomic(&auth_path(), json.as_bytes())
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
                out.openrouter_model = cfg.models.strong;
                out.openrouter_fast_model = cfg.models.fast;
                out.openrouter_model_provider = cfg.models.provider;
                out.openrouter_fast_provider = cfg.models.fast_provider;
                out.openrouter_pin_provider = cfg.models.pin_provider;
                out.capability_preset = cfg.capability.preset;
            }
        }
    }
    if auth_path().is_file() {
        let raw = fs::read_to_string(auth_path()).map_err(PipelineError::from)?;
        if let Ok(auth) = serde_json::from_str::<AuthFile>(&raw) {
            out.openrouter_api_key = auth.openrouter_api_key;
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

fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            let mut f = fs::File::create(path)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            let _ = e;
            Ok(())
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
            openrouter_model: "google/gemini-2.5-flash-lite".into(),
            openrouter_fast_model: "fast-model".into(),
            openrouter_model_provider: "coreweave".into(),
            openrouter_fast_provider: "baseten".into(),
            openrouter_pin_provider: true,
            capability_preset: "voice_safe".into(),
        };
        save_settings(&s).expect("save");
        assert!(config_path().is_file());
        assert!(auth_path().is_file());

        let raw_cfg = fs::read_to_string(config_path()).unwrap();
        assert!(raw_cfg.contains("[models]"));
        assert!(raw_cfg.contains("strong"));
        assert!(!raw_cfg.contains("sk-test"), "key must not be in config.toml");

        let raw_auth = fs::read_to_string(auth_path()).unwrap();
        assert!(raw_auth.contains("sk-test"));
        assert!(raw_auth.contains("openrouter_api_key"));

        let loaded = load_settings().expect("load");
        assert_eq!(loaded.openrouter_api_key, "sk-test");
        assert_eq!(loaded.openrouter_model, "google/gemini-2.5-flash-lite");
        assert_eq!(loaded.openrouter_fast_model, "fast-model");
        assert_eq!(loaded.openrouter_model_provider, "coreweave");
        assert!(loaded.openrouter_pin_provider);
        assert_eq!(loaded.capability_preset, "voice_safe");

        // Hand-edited sections must survive a subsequent save.
        let mut raw_cfg = fs::read_to_string(config_path()).unwrap();
        raw_cfg.push_str(
            "\n[audio]\ninput_device = \"USB Mic\"\noutput_device = \"Speakers\"\n\n[custom]\nfoo = 1\n",
        );
        fs::write(config_path(), &raw_cfg).unwrap();
        save_settings(&loaded).expect("re-save");
        let after = fs::read_to_string(config_path()).unwrap();
        assert!(after.contains("USB Mic"), "audio section wiped: {after}");
        assert!(after.contains("[custom]"), "unknown section wiped: {after}");
        assert!(after.contains("foo"), "unknown key wiped: {after}");
        // Fresh defaults must not re-inject hard-coded audio/ui/logging when absent.
        assert!(!after.contains("show_overlay_on_wake") || after.contains("[ui]"));

        std::env::remove_var(paths::BORIS_HOME_ENV);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_does_not_inject_default_audio_ui_logging() {
        let _g = LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "boris-settings-noinject-{}",
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
        // Header comments may mention section names; require real tables are absent.
        assert!(
            !raw.lines().any(|l| l.trim() == "[audio]"),
            "should not invent [audio] table: {raw}"
        );
        assert!(
            !raw.lines().any(|l| l.trim() == "[ui]"),
            "should not invent [ui] table: {raw}"
        );
        assert!(
            !raw.lines().any(|l| l.trim() == "[logging]"),
            "should not invent [logging] table: {raw}"
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
