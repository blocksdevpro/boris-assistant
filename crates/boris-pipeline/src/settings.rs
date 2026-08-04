//! Persistent user settings under `~/.boris/settings.json`.
//!
//! Stores OpenRouter credentials so the desktop shell can restore fields
//! across launches. Never log the API key.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::paths::boris_home;

/// Path to the settings file (`~/.boris/settings.json`, or `$BORIS_HOME/settings.json`).
pub fn settings_path() -> PathBuf {
    boris_home().join("settings.json")
}

/// Desktop connection settings restored into the main window on launch.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSettings {
    /// OpenRouter API key (password field). Empty means "use env at start".
    #[serde(default)]
    pub openrouter_api_key: String,
    /// Preferred OpenRouter model id. Empty means backend default.
    #[serde(default)]
    pub openrouter_model: String,
    /// Tool capability preset: `full` | `local_power` | `voice_safe`.
    /// Empty → engine default (Full). Override also via `BORIS_CAPABILITY`.
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
            .field("capability_preset", &self.capability_preset)
            .finish()
    }
}

/// Load settings from disk. Missing file → defaults. Corrupt JSON → error.
pub fn load_settings() -> Result<AppSettings, String> {
    let path = settings_path();
    if !path.is_file() {
        return Ok(AppSettings::default());
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("read settings: {e}"))?;
    if raw.trim().is_empty() {
        return Ok(AppSettings::default());
    }
    serde_json::from_str(&raw).map_err(|e| format!("parse settings: {e}"))
}

/// Persist settings atomically-ish (write temp then rename when possible).
/// Creates `~/.boris` if needed. Does not log secrets.
pub fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create ~/.boris: {e}"))?;
    }

    let json =
        serde_json::to_string_pretty(settings).map_err(|e| format!("serialize settings: {e}"))?;

    write_atomic(&path, json.as_bytes()).map_err(|e| format!("write settings: {e}"))?;

    tracing::debug!(path = %path.display(), "settings saved");
    Ok(())
}

fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    // On Windows, rename over existing may fail — remove first.
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Fallback: direct write if rename failed.
            let _ = fs::remove_file(&tmp);
            let mut f = fs::File::create(path)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            // Preserve original error only if fallback also fails — already wrote.
            let _ = e;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_path_under_boris() {
        let p = settings_path();
        assert!(
            p.ends_with("settings.json"),
            "unexpected path {}",
            p.display()
        );
        assert!(
            p.parent()
                .map(|d| d.ends_with(".boris")
                    || std::env::var(crate::paths::BORIS_HOME_ENV).is_ok())
                .unwrap_or(false),
            "settings should live under boris home, got {}",
            p.display()
        );
    }

    #[test]
    fn default_deserializes_empty_object() {
        let s: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, AppSettings::default());
    }
}
