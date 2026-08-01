//! Filesystem-backed session store.
//!
//! On-disk layout under `sessions_root` (caller supplies the path; pipeline will
//! pass `~/.boris/sessions`):
//!
//! ```text
//! {sessions_root}/
//!   current.json          # { "session_id": "s-..." }
//!   {session_id}/
//!     meta.json
//!     transcript.jsonl
//! ```
//!
//! Meta and `current.json` use atomic-ish writes (temp file + rename, with a
//! Windows-friendly fallback), matching `boris-pipeline` settings.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::transcript::{self, append_exchange, read_all};
use super::types::{generate_session_id, SessionId, SessionMeta, SessionStatus};

/// Pointer file at `{sessions_root}/current.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CurrentFile {
    session_id: SessionId,
}

/// Filesystem session store rooted at `sessions_root`.
#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    /// Create a store for the given root directory (does not create it yet).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Ensure `sessions_root` exists.
    pub fn ensure_root(&self) -> Result<(), String> {
        fs::create_dir_all(&self.root).map_err(|e| format!("create sessions root: {e}"))
    }

    /// Directory for one session: `{root}/{session_id}/`.
    pub fn session_dir(&self, id: &SessionId) -> PathBuf {
        self.root.join(id.as_str())
    }

    /// Path to the JSONL transcript for a session.
    pub fn transcript_path(&self, id: &SessionId) -> PathBuf {
        self.session_dir(id).join("transcript.jsonl")
    }

    fn meta_path(&self, id: &SessionId) -> PathBuf {
        self.session_dir(id).join("meta.json")
    }

    fn current_path(&self) -> PathBuf {
        self.root.join("current.json")
    }

    /// Create a new Active session: write `meta.json`, set `current.json`.
    pub fn create(&self) -> Result<SessionMeta, String> {
        self.ensure_root()?;
        let id = generate_session_id();
        let meta = SessionMeta::new_active(id.clone());
        fs::create_dir_all(self.session_dir(&id))
            .map_err(|e| format!("create session dir: {e}"))?;
        self.write_meta(&meta)?;
        self.set_current(&id)?;
        Ok(meta)
    }

    /// Load `meta.json` for an existing session.
    pub fn open(&self, id: &SessionId) -> Result<SessionMeta, String> {
        let path = self.meta_path(id);
        let raw = fs::read_to_string(&path)
            .map_err(|e| format!("read meta {}: {e}", path.display()))?;
        serde_json::from_str(&raw).map_err(|e| format!("parse meta {}: {e}", path.display()))
    }

    /// Read the current session id pointer, if any.
    pub fn current_id(&self) -> Result<Option<SessionId>, String> {
        let path = self.current_path();
        if !path.is_file() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path)
            .map_err(|e| format!("read current {}: {e}", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(None);
        }
        let cur: CurrentFile = serde_json::from_str(&raw)
            .map_err(|e| format!("parse current {}: {e}", path.display()))?;
        Ok(Some(cur.session_id))
    }

    /// Point `current.json` at `id`.
    pub fn set_current(&self, id: &SessionId) -> Result<(), String> {
        self.ensure_root()?;
        let cur = CurrentFile {
            session_id: id.clone(),
        };
        let json = serde_json::to_string_pretty(&cur)
            .map_err(|e| format!("serialize current: {e}"))?;
        write_atomic(&self.current_path(), json.as_bytes())
            .map_err(|e| format!("write current: {e}"))
    }

    /// Mark session Ended; clear `current.json` if it points at this id.
    pub fn end(&self, id: &SessionId) -> Result<SessionMeta, String> {
        let mut meta = self.open(id)?;
        meta.end();
        self.write_meta(&meta)?;
        if let Some(cur) = self.current_id()? {
            if &cur == id {
                self.clear_current()?;
            }
        }
        Ok(meta)
    }

    /// End the current session if one is set.
    pub fn end_current(&self) -> Result<Option<SessionMeta>, String> {
        match self.current_id()? {
            Some(id) => Ok(Some(self.end(&id)?)),
            None => Ok(None),
        }
    }

    /// Resume current if Active; otherwise create a new session.
    pub fn resume_or_create(&self) -> Result<SessionMeta, String> {
        if let Some(id) = self.current_id()? {
            match self.open(&id) {
                Ok(meta) if meta.status == SessionStatus::Active => return Ok(meta),
                Ok(_) | Err(_) => {
                    // Ended, missing, or corrupt — fall through to create.
                }
            }
        }
        self.create()
    }

    /// Append a user/assistant exchange and touch session metadata.
    pub fn append_user_assistant(
        &self,
        id: &SessionId,
        user: &str,
        assistant: &str,
    ) -> Result<(), String> {
        let path = self.transcript_path(id);
        append_exchange(&path, user, assistant)?;
        self.touch(id)?;
        Ok(())
    }

    /// Load the full transcript (missing file → empty).
    pub fn load_transcript(
        &self,
        id: &SessionId,
    ) -> Result<Vec<transcript::TranscriptRecord>, String> {
        read_all(&self.transcript_path(id))
    }

    /// Refresh `updated_at_unix_ms` on disk.
    pub fn touch(&self, id: &SessionId) -> Result<(), String> {
        let mut meta = self.open(id)?;
        meta.touch();
        self.write_meta(&meta)
    }

    fn write_meta(&self, meta: &SessionMeta) -> Result<(), String> {
        let path = self.meta_path(&meta.id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create session dir: {e}"))?;
        }
        let json =
            serde_json::to_string_pretty(meta).map_err(|e| format!("serialize meta: {e}"))?;
        write_atomic(&path, json.as_bytes()).map_err(|e| format!("write meta: {e}"))
    }

    fn clear_current(&self) -> Result<(), String> {
        let path = self.current_path();
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("clear current: {e}"))?;
        }
        Ok(())
    }
}

/// Atomic-ish write: temp file + rename; on Windows remove destination first,
/// with direct-write fallback if rename still fails.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
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
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_store_root(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("boris-session-store-{nanos}-{n}-{label}"));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn cleanup(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn create_writes_meta_and_current() {
        let root = temp_store_root("create");
        let store = SessionStore::new(&root);

        let meta = store.create().expect("create");
        assert_eq!(meta.status, SessionStatus::Active);
        assert!(meta.id.as_str().starts_with("s-"));

        let meta_path = store.meta_path(&meta.id);
        assert!(meta_path.is_file(), "meta.json should exist");

        let cur = store.current_id().expect("current_id");
        assert_eq!(cur.as_ref(), Some(&meta.id));

        let opened = store.open(&meta.id).expect("open");
        assert_eq!(opened, meta);

        cleanup(&root);
    }

    #[test]
    fn set_current_and_paths() {
        let root = temp_store_root("paths");
        let store = SessionStore::new(&root);
        let a = store.create().expect("create a");
        let b = store.create().expect("create b");

        // create() sets current to the latest
        assert_eq!(store.current_id().unwrap().as_ref(), Some(&b.id));

        store.set_current(&a.id).expect("set_current");
        assert_eq!(store.current_id().unwrap().as_ref(), Some(&a.id));

        let dir = store.session_dir(&a.id);
        assert_eq!(dir, root.join(a.id.as_str()));
        assert_eq!(
            store.transcript_path(&a.id),
            dir.join("transcript.jsonl")
        );

        cleanup(&root);
    }

    #[test]
    fn end_marks_ended_and_clears_current() {
        let root = temp_store_root("end");
        let store = SessionStore::new(&root);
        let meta = store.create().expect("create");

        let ended = store.end(&meta.id).expect("end");
        assert_eq!(ended.status, SessionStatus::Ended);
        assert!(ended.updated_at_unix_ms >= meta.updated_at_unix_ms);
        assert_eq!(store.current_id().unwrap(), None);

        let reopened = store.open(&meta.id).expect("open after end");
        assert_eq!(reopened.status, SessionStatus::Ended);

        cleanup(&root);
    }

    #[test]
    fn end_does_not_clear_unrelated_current() {
        let root = temp_store_root("end_other");
        let store = SessionStore::new(&root);
        let a = store.create().expect("a");
        let b = store.create().expect("b");
        assert_eq!(store.current_id().unwrap().as_ref(), Some(&b.id));

        store.end(&a.id).expect("end a");
        assert_eq!(store.current_id().unwrap().as_ref(), Some(&b.id));

        cleanup(&root);
    }

    #[test]
    fn end_current_none_and_some() {
        let root = temp_store_root("end_current");
        let store = SessionStore::new(&root);
        store.ensure_root().unwrap();

        assert!(store.end_current().unwrap().is_none());

        let meta = store.create().expect("create");
        let ended = store.end_current().expect("end_current").expect("some");
        assert_eq!(ended.id, meta.id);
        assert_eq!(ended.status, SessionStatus::Ended);
        assert_eq!(store.current_id().unwrap(), None);

        cleanup(&root);
    }

    #[test]
    fn resume_or_create_resumes_active() {
        let root = temp_store_root("resume_active");
        let store = SessionStore::new(&root);
        let meta = store.create().expect("create");

        let resumed = store.resume_or_create().expect("resume");
        assert_eq!(resumed.id, meta.id);
        assert_eq!(resumed.status, SessionStatus::Active);

        cleanup(&root);
    }

    #[test]
    fn resume_or_create_creates_when_ended() {
        let root = temp_store_root("resume_ended");
        let store = SessionStore::new(&root);
        let old = store.create().expect("create");
        store.end(&old.id).expect("end");

        let next = store.resume_or_create().expect("create new");
        assert_ne!(next.id, old.id);
        assert_eq!(next.status, SessionStatus::Active);
        assert_eq!(store.current_id().unwrap().as_ref(), Some(&next.id));

        cleanup(&root);
    }

    #[test]
    fn resume_or_create_when_empty_root() {
        let root = temp_store_root("resume_empty");
        let store = SessionStore::new(&root);
        let meta = store.resume_or_create().expect("create");
        assert_eq!(meta.status, SessionStatus::Active);
        assert!(store.meta_path(&meta.id).is_file());
        cleanup(&root);
    }

    #[test]
    fn append_and_load_transcript_touches_meta() {
        let root = temp_store_root("transcript");
        let store = SessionStore::new(&root);
        let meta = store.create().expect("create");
        let before = store.open(&meta.id).unwrap().updated_at_unix_ms;

        store
            .append_user_assistant(&meta.id, "hello", "hi there")
            .expect("append");

        let records = store.load_transcript(&meta.id).expect("load");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].role, "user");
        assert_eq!(records[1].role, "assistant");

        let after = store.open(&meta.id).unwrap().updated_at_unix_ms;
        assert!(after >= before);
        assert!(store.transcript_path(&meta.id).is_file());

        cleanup(&root);
    }

    #[test]
    fn load_transcript_missing_is_empty() {
        let root = temp_store_root("no_transcript");
        let store = SessionStore::new(&root);
        let meta = store.create().expect("create");
        let records = store.load_transcript(&meta.id).expect("load");
        assert!(records.is_empty());
        cleanup(&root);
    }

    #[test]
    fn touch_updates_timestamp() {
        let root = temp_store_root("touch");
        let store = SessionStore::new(&root);
        let meta = store.create().expect("create");
        let before = meta.updated_at_unix_ms;

        store.touch(&meta.id).expect("touch");
        let after = store.open(&meta.id).unwrap().updated_at_unix_ms;
        assert!(after >= before);

        cleanup(&root);
    }

    #[test]
    fn current_json_shape() {
        let root = temp_store_root("current_shape");
        let store = SessionStore::new(&root);
        let meta = store.create().expect("create");
        let raw = fs::read_to_string(store.current_path()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            v["session_id"].as_str().unwrap(),
            meta.id.as_str()
        );
        cleanup(&root);
    }
}
