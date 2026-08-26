//! Load / save [`UserProfile`] under a host-supplied path (typically
//! `~/.boris/memory/profile.json`).
//!
//! # On-disk format
//!
//! Pretty-printed JSON matching the serde shape of [`UserProfile`]. Missing or
//! empty files load as [`UserProfile::default`]. Writes are temp+rename
//! (atomic-ish) so a crash mid-write does not leave a half JSON file.

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
        let mut profile: UserProfile = serde_json::from_str(&raw)
            .map_err(|e| format!("parse profile {}: {e}", self.path.display()))?;
        profile.expire_due_facts();
        Ok(profile)
    }

    /// Atomic-ish write (temp + rename). Creates parent dirs.
    pub fn save(&self, profile: &UserProfile) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create profile dir {}: {e}", parent.display()))?;
        }
        let json =
            serde_json::to_string_pretty(profile).map_err(|e| format!("serialize profile: {e}"))?;
        write_atomic(&self.path, json.as_bytes())
            .map_err(|e| format!("write profile {}: {e}", self.path.display()))?;
        Ok(())
    }
}

/// Write `bytes` to `path` via `{path}.json.tmp` then rename (fallback: direct write).
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
        assert_eq!(loaded.facts.len(), 2);
        assert!(loaded.facts.iter().all(|fact| fact.is_active()));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn loads_v1_facts_with_active_lifecycle_defaults() {
        let path = temp_path("v1");
        let raw = serde_json::json!({
            "version": 1,
            "preferred_name": null,
            "preferences": [],
            "facts": [{
                "id": "old-fact",
                "text": "Uses Rust",
                "category": "project",
                "confidence": 0.8,
                "source": "legacy",
                "created_at_ms": 1,
                "last_seen_at_ms": 1,
                "salience": 5
            }],
            "ongoing": [],
            "updated_at_ms": 1,
            "turns_seen": 2
        });
        fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();

        let loaded = ProfileStore::new(&path).load().unwrap();
        assert_eq!(loaded.facts[0].status, crate::memory::FactStatus::Active);
        assert!(loaded.facts[0].memory_key.is_none());
        let _ = fs::remove_file(&path);
    }
}
