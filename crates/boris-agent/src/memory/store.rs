//! Load / save [`UserProfile`] under a host-supplied path (typically
//! `~/.boris/memory/profile.json`).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::profile::UserProfile;

/// File-backed personal profile store.
#[derive(Debug, Clone)]
pub struct ProfileStore {
    path: PathBuf,
}

impl ProfileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Missing / empty file → default profile. Corrupt JSON → error.
    pub fn load(&self) -> Result<UserProfile, String> {
        if !self.path.is_file() {
            return Ok(UserProfile::default());
        }
        let raw = fs::read_to_string(&self.path)
            .map_err(|e| format!("read profile {}: {e}", self.path.display()))?;
        if raw.trim().is_empty() {
            return Ok(UserProfile::default());
        }
        serde_json::from_str(&raw)
            .map_err(|e| format!("parse profile {}: {e}", self.path.display()))
    }

    /// Atomic-ish write (temp + rename). Creates parent dirs.
    pub fn save(&self, profile: &UserProfile) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create profile dir {}: {e}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(profile)
            .map_err(|e| format!("serialize profile: {e}"))?;
        write_atomic(&self.path, json.as_bytes())
            .map_err(|e| format!("write profile {}: {e}", self.path.display()))?;
        Ok(())
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
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
        Err(_) => {
            let _ = fs::remove_file(&tmp);
            let mut f = fs::File::create(path)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::profile::{FactCategory, UserFact};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(label: &str) -> PathBuf {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        std::env::temp_dir().join(format!("boris-profile-{label}-{ms}.json"))
    }

    #[test]
    fn load_missing_defaults() {
        let path = temp_path("missing");
        let _ = fs::remove_file(&path);
        let store = ProfileStore::new(&path);
        let p = store.load().unwrap();
        assert!(p.is_empty());
    }

    #[test]
    fn round_trip() {
        let path = temp_path("rt");
        let _ = fs::remove_file(&path);
        let store = ProfileStore::new(&path);
        let mut p = UserProfile::default();
        p.set_preferred_name("Ada");
        p.add_or_refresh_fact(UserFact::new(
            "Builds voice agents",
            FactCategory::Project,
            "test",
        ));
        store.save(&p).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.preferred_name.as_deref(), Some("Ada"));
        assert_eq!(loaded.facts.len(), 1);
        let _ = fs::remove_file(&path);
    }
}
