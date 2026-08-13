//! Filesystem-backed session store (Grok-like layout).
//!
//! On-disk layout under `sessions_root` (pipeline passes `~/.boris/sessions/desktop`):
//!
//! ```text
//! {sessions_root}/
//!   current.json              # { "session_id": "<uuid>" }
//!   {session_id}/
//!     summary.json            # Grok-like session summary
//!     chat_history.jsonl      # full agent transcript (user/assistant/tool/system)
//!     events.jsonl            # turn lifecycle events
//!     todos.json              # session todo list (`[]` when empty)
//!     tool_calls.jsonl        # may be lazy-created by audit sink
//!     memory.md               # session turn log (LTM; lazy on first append)
//!     artifacts/              # visual cards: index.json + `{slug}-{id}.{ext}`
//!     subagents/              # per-session subagent artifacts
//!     scratch/                # optional empty dir
//! ```
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`summary`] | pure `summary.json` / `current.json` shapes |
//! | [`atomic`] | tmp+rename write helper |

mod atomic;
mod summary;

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

use super::transcript::{
    self, append_event, append_records, read_all, write_all, TranscriptRecord,
};
use super::types::{generate_session_id, SessionId, SessionMeta, SessionStatus};

use atomic::write_atomic;
use summary::{CurrentFile, SummaryFile};

/// Filesystem session store rooted at `sessions_root`.
#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn ensure_root(&self) -> Result<(), String> {
        fs::create_dir_all(&self.root).map_err(|e| format!("create sessions root: {e}"))
    }

    pub fn session_dir(&self, id: &SessionId) -> PathBuf {
        self.root.join(id.as_str())
    }

    /// Path to Grok-style chat history.
    pub fn transcript_path(&self, id: &SessionId) -> PathBuf {
        self.session_dir(id).join("chat_history.jsonl")
    }

    /// Path to session todos (`todos.json`).
    pub fn todos_path(&self, id: &SessionId) -> PathBuf {
        self.session_dir(id).join("todos.json")
    }

    /// Path to tool-call audit log (`tool_calls.jsonl`; may be lazy-created).
    pub fn tool_calls_path(&self, id: &SessionId) -> PathBuf {
        self.session_dir(id).join("tool_calls.jsonl")
    }

    /// Directory for per-session subagent artifacts.
    pub fn subagents_dir(&self, id: &SessionId) -> PathBuf {
        self.session_dir(id).join("subagents")
    }

    /// Optional per-session scratch directory.
    pub fn scratch_dir(&self, id: &SessionId) -> PathBuf {
        self.session_dir(id).join("scratch")
    }

    /// Directory for session-local visual cards (`artifacts/`).
    pub fn artifacts_dir(&self, id: &SessionId) -> PathBuf {
        self.session_dir(id).join("artifacts")
    }

    /// Catalog file (`artifacts/index.json`).
    pub fn artifacts_index_path(&self, id: &SessionId) -> PathBuf {
        self.artifacts_dir(id).join("index.json")
    }

    /// Load the session artifact catalog. Missing dir/file → empty index.
    pub fn load_artifact_index(
        &self,
        id: &SessionId,
    ) -> Result<super::artifacts::ArtifactIndex, String> {
        super::artifacts::ArtifactStore::new(self.artifacts_dir(id)).load_index()
    }

    /// Session-local turn log (`memory.md`) for long-term memory append/search.
    pub fn memory_path(&self, id: &SessionId) -> PathBuf {
        self.session_dir(id).join("memory.md")
    }

    fn summary_path(&self, id: &SessionId) -> PathBuf {
        self.session_dir(id).join("summary.json")
    }

    fn events_path(&self, id: &SessionId) -> PathBuf {
        self.session_dir(id).join("events.jsonl")
    }

    fn current_path(&self) -> PathBuf {
        self.root.join("current.json")
    }

    /// Ensure session dir exists; write empty `todos.json` if missing; create
    /// `subagents/` and `scratch/`.
    ///
    /// Does **not** create empty `tool_calls.jsonl` (lazy on first audit write).
    pub fn ensure_session_artifacts(&self, id: &SessionId) -> Result<(), String> {
        let dir = self.session_dir(id);
        fs::create_dir_all(&dir).map_err(|e| format!("create session dir: {e}"))?;

        let todos = self.todos_path(id);
        if !todos.is_file() {
            write_atomic(&todos, b"[]").map_err(|e| format!("write todos: {e}"))?;
        }

        fs::create_dir_all(self.subagents_dir(id))
            .map_err(|e| format!("create subagents dir: {e}"))?;
        fs::create_dir_all(self.scratch_dir(id)).map_err(|e| format!("create scratch dir: {e}"))?;
        super::artifacts::ArtifactStore::new(self.artifacts_dir(id)).ensure()?;

        Ok(())
    }

    /// Session ids under the root that have a `summary.json`.
    pub fn list_ids(&self) -> Result<Vec<SessionId>, String> {
        if !self.root.is_dir() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        let entries = fs::read_dir(&self.root).map_err(|e| format!("read sessions root: {e}"))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("read sessions root entry: {e}"))?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if !path.join("summary.json").is_file() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(|s| s.to_string()) else {
                continue;
            };
            ids.push(SessionId::from(name));
        }
        ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        Ok(ids)
    }

    /// Create a new Active session: write `summary.json`, set `current.json`.
    pub fn create(&self) -> Result<SessionMeta, String> {
        self.ensure_root()?;
        let id = generate_session_id();
        let meta = SessionMeta::new_active(id.clone());
        fs::create_dir_all(self.session_dir(&id))
            .map_err(|e| format!("create session dir: {e}"))?;
        self.ensure_session_artifacts(&id)?;
        self.write_summary(&meta, 0)?;
        self.set_current(&id)?;
        let _ = append_event(
            &self.events_path(&id),
            "session_started",
            json!({ "session_id": id.as_str() }),
        );
        Ok(meta)
    }

    /// Load `summary.json` for an existing session.
    pub fn open(&self, id: &SessionId) -> Result<SessionMeta, String> {
        let summary = self.summary_path(id);
        if !summary.is_file() {
            return Err(format!("session summary not found: {}", summary.display()));
        }
        let raw = fs::read_to_string(&summary)
            .map_err(|e| format!("read summary {}: {e}", summary.display()))?;
        let file: SummaryFile = serde_json::from_str(&raw)
            .map_err(|e| format!("parse summary {}: {e}", summary.display()))?;
        Ok(file.to_meta())
    }

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

    pub fn set_current(&self, id: &SessionId) -> Result<(), String> {
        self.ensure_root()?;
        let cur = CurrentFile {
            session_id: id.clone(),
        };
        let json =
            serde_json::to_string_pretty(&cur).map_err(|e| format!("serialize current: {e}"))?;
        write_atomic(&self.current_path(), json.as_bytes())
            .map_err(|e| format!("write current: {e}"))
    }

    pub fn end(&self, id: &SessionId) -> Result<SessionMeta, String> {
        let mut meta = self.open(id)?;
        meta.end();
        let n = self.count_messages(id);
        self.write_summary(&meta, n)?;
        let _ = append_event(
            &self.events_path(id),
            "session_ended",
            json!({ "session_id": id.as_str() }),
        );
        if let Some(cur) = self.current_id()? {
            if &cur == id {
                self.clear_current()?;
            }
        }
        Ok(meta)
    }

    pub fn end_current(&self) -> Result<Option<SessionMeta>, String> {
        match self.current_id()? {
            Some(id) => Ok(Some(self.end(&id)?)),
            None => Ok(None),
        }
    }

    pub fn resume_or_create(&self) -> Result<SessionMeta, String> {
        if let Some(id) = self.current_id()? {
            match self.open(&id) {
                Ok(meta) if meta.status == SessionStatus::Active => return Ok(meta),
                Ok(_) | Err(_) => {}
            }
        }
        self.create()
    }

    /// Append full agent messages (system / user / assistant+tool_calls / tool).
    ///
    /// Each entry is `(role, content)` matching [`crate::context::Message`].
    pub fn append_messages(
        &self,
        id: &SessionId,
        messages: &[(String, Value)],
    ) -> Result<(), String> {
        if messages.is_empty() {
            return Ok(());
        }
        let records: Vec<TranscriptRecord> = messages
            .iter()
            .map(|(role, content)| TranscriptRecord::from_role_content(role, content.clone()))
            .collect();
        append_records(&self.transcript_path(id), &records)?;
        let _ = append_event(
            &self.events_path(id),
            "messages_appended",
            json!({
                "session_id": id.as_str(),
                "count": records.len(),
            }),
        );
        self.touch(id)?;
        Ok(())
    }

    /// Replace chat_history with the full current agent context (after prune/compact).
    pub fn write_messages(
        &self,
        id: &SessionId,
        messages: &[(String, Value)],
    ) -> Result<(), String> {
        let records: Vec<TranscriptRecord> = messages
            .iter()
            .map(|(role, content)| TranscriptRecord::from_role_content(role, content.clone()))
            .collect();
        write_all(&self.transcript_path(id), &records)?;
        let _ = append_event(
            &self.events_path(id),
            "messages_rewritten",
            json!({
                "session_id": id.as_str(),
                "count": records.len(),
            }),
        );
        self.touch(id)?;
        Ok(())
    }

    /// Sync agent context to disk: append when growing, rewrite when pruned.
    ///
    /// `already_persisted` is the number of messages previously written from this
    /// live context. Returns the new persisted count (`messages.len()`).
    pub fn sync_messages(
        &self,
        id: &SessionId,
        messages: &[(String, Value)],
        already_persisted: usize,
    ) -> Result<usize, String> {
        let n = messages.len();
        if n < already_persisted {
            // Context pruned/compacted â€” rewrite full snapshot.
            self.write_messages(id, messages)?;
        } else if n > already_persisted {
            self.append_messages(id, &messages[already_persisted..])?;
        }
        Ok(n)
    }

    /// Load `chat_history.jsonl`. Missing file yields an empty transcript.
    pub fn load_transcript(
        &self,
        id: &SessionId,
    ) -> Result<Vec<transcript::TranscriptRecord>, String> {
        let path = self.transcript_path(id);
        if path.is_file() {
            return read_all(&path);
        }
        Ok(Vec::new())
    }

    pub fn touch(&self, id: &SessionId) -> Result<(), String> {
        let mut meta = self.open(id)?;
        meta.touch();
        let n = self.count_messages(id);
        self.write_summary(&meta, n)
    }

    fn count_messages(&self, id: &SessionId) -> u64 {
        self.load_transcript(id)
            .map(|r| r.len() as u64)
            .unwrap_or(0)
    }

    fn write_summary(&self, meta: &SessionMeta, num_messages: u64) -> Result<(), String> {
        let path = self.summary_path(&meta.id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create session dir: {e}"))?;
        }
        let file = SummaryFile::from_meta(meta, num_messages);
        let json =
            serde_json::to_string_pretty(&file).map_err(|e| format!("serialize summary: {e}"))?;
        write_atomic(&path, json.as_bytes()).map_err(|e| format!("write summary: {e}"))
    }

    fn clear_current(&self) -> Result<(), String> {
        let path = self.current_path();
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("clear current: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
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
    fn create_writes_summary_and_current() {
        let root = temp_store_root("create");
        let store = SessionStore::new(&root);

        let meta = store.create().expect("create");
        assert_eq!(meta.status, SessionStatus::Active);

        let summary_path = store.summary_path(&meta.id);
        assert!(summary_path.is_file(), "summary.json should exist");
        let raw = fs::read_to_string(&summary_path).unwrap();
        assert!(raw.contains("\"info\""));
        assert!(raw.contains("created_at"));
        assert!(raw.contains("active"));

        let cur = store.current_id().expect("current_id");
        assert_eq!(cur.as_ref(), Some(&meta.id));

        cleanup(&root);
    }

    #[test]
    fn create_ensures_session_artifacts() {
        let root = temp_store_root("artifacts");
        let store = SessionStore::new(&root);
        let meta = store.create().expect("create");

        let todos = store.todos_path(&meta.id);
        assert!(todos.is_file(), "todos.json should exist");
        let todos_raw = fs::read_to_string(&todos).unwrap();
        assert_eq!(todos_raw.trim(), "[]");

        assert!(
            store.subagents_dir(&meta.id).is_dir(),
            "subagents/ should exist"
        );
        assert!(
            store.scratch_dir(&meta.id).is_dir(),
            "scratch/ should exist"
        );
        assert!(
            store.artifacts_dir(&meta.id).is_dir(),
            "artifacts/ should exist"
        );
        let art_index = store.artifacts_index_path(&meta.id);
        assert!(art_index.is_file(), "artifacts/index.json should exist");
        let art_raw = fs::read_to_string(&art_index).unwrap();
        assert!(art_raw.contains("\"items\""), "{art_raw}");
        let loaded = store.load_artifact_index(&meta.id).unwrap();
        assert!(loaded.items.is_empty());
        assert!(loaded.current.is_none());

        // tool_calls.jsonl is lazy — must not be created empty at session start.
        assert!(
            !store.tool_calls_path(&meta.id).exists(),
            "tool_calls.jsonl should not be pre-created"
        );

        cleanup(&root);
    }

    #[test]
    fn ensure_session_artifacts_idempotent_preserves_todos() {
        let root = temp_store_root("artifacts_idempotent");
        let store = SessionStore::new(&root);
        let meta = store.create().expect("create");

        let todos = store.todos_path(&meta.id);
        fs::write(&todos, b"[{\"id\":\"t1\"}]").unwrap();

        store
            .ensure_session_artifacts(&meta.id)
            .expect("ensure again");

        let todos_raw = fs::read_to_string(&todos).unwrap();
        assert_eq!(todos_raw, "[{\"id\":\"t1\"}]");
        assert!(store.subagents_dir(&meta.id).is_dir());
        assert!(store.scratch_dir(&meta.id).is_dir());

        cleanup(&root);
    }

    #[test]
    fn list_ids_finds_sessions_with_summary() {
        let root = temp_store_root("list_ids");
        let store = SessionStore::new(&root);

        assert!(store.list_ids().unwrap().is_empty());

        let a = store.create().expect("create a");
        let b = store.create().expect("create b");

        // Noise: dir without summary.json is ignored.
        fs::create_dir_all(root.join("not-a-session")).unwrap();
        fs::write(root.join("junk.txt"), b"x").unwrap();

        let mut ids = store.list_ids().expect("list");
        ids.sort_by(|x, y| x.as_str().cmp(y.as_str()));
        let mut expected = vec![a.id.clone(), b.id.clone()];
        expected.sort_by(|x, y| x.as_str().cmp(y.as_str()));
        assert_eq!(ids, expected);

        cleanup(&root);
    }

    #[test]
    fn append_writes_chat_history_grok_shape() {
        let root = temp_store_root("transcript");
        let store = SessionStore::new(&root);
        let meta = store.create().expect("create");

        store
            .append_messages(
                &meta.id,
                &[
                    ("user".into(), json!("hello")),
                    ("assistant".into(), json!("hi there")),
                ],
            )
            .expect("append");

        let path = store.transcript_path(&meta.id);
        assert!(path.is_file());
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"type\":\"user\""));
        assert!(raw.contains("chat_history") || raw.contains("\"text\":\"hello\""));

        let events = fs::read_to_string(store.events_path(&meta.id)).unwrap();
        assert!(events.contains("messages_appended") || events.contains("session_started"));

        let records = store.load_transcript(&meta.id).expect("load");
        assert_eq!(records.len(), 2);

        cleanup(&root);
    }

    #[test]
    fn sync_messages_appends_then_rewrites_on_shrink() {
        let root = temp_store_root("sync");
        let store = SessionStore::new(&root);
        let meta = store.create().expect("create");

        let full = vec![
            ("system".into(), json!("sys")),
            ("user".into(), json!("u1")),
            (
                "assistant".into(),
                json!({
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "c1",
                        "type": "function",
                        "function": { "name": "bash", "arguments": "{}" }
                    }]
                }),
            ),
            (
                "tool".into(),
                json!({ "tool_call_id": "c1", "content": "ok" }),
            ),
            ("assistant".into(), json!("done")),
        ];
        let n = store.sync_messages(&meta.id, &full, 0).unwrap();
        assert_eq!(n, 5);

        let raw = fs::read_to_string(store.transcript_path(&meta.id)).unwrap();
        assert!(raw.contains("tool_result"));
        assert!(raw.contains("bash"));

        let records = store.load_transcript(&meta.id).unwrap();
        assert_eq!(records.len(), 5);
        assert_eq!(records[3].role, "tool");
        assert!(records[2].content.get("tool_calls").is_some());

        // Append one more
        let mut grown = full.clone();
        grown.push(("user".into(), json!("u2")));
        grown.push(("assistant".into(), json!("a2")));
        let n2 = store.sync_messages(&meta.id, &grown, n).unwrap();
        assert_eq!(n2, 7);
        assert_eq!(store.load_transcript(&meta.id).unwrap().len(), 7);

        // Shrink (prune) â†’ rewrite
        let shrunk = vec![
            ("system".into(), json!("sys")),
            ("user".into(), json!("u2")),
            ("assistant".into(), json!("a2")),
        ];
        let n3 = store.sync_messages(&meta.id, &shrunk, n2).unwrap();
        assert_eq!(n3, 3);
        assert_eq!(store.load_transcript(&meta.id).unwrap().len(), 3);

        cleanup(&root);
    }

    #[test]
    fn create_uses_uuid_session_id() {
        let root = temp_store_root("uuid");
        let store = SessionStore::new(&root);
        let meta = store.create().expect("create");
        let parts: Vec<_> = meta.id.as_str().split('-').collect();
        assert_eq!(parts.len(), 5, "id={}", meta.id.as_str());
        cleanup(&root);
    }

    #[test]
    fn resume_or_create_resumes_active() {
        let root = temp_store_root("resume_active");
        let store = SessionStore::new(&root);
        let meta = store.create().expect("create");
        let resumed = store.resume_or_create().expect("resume");
        assert_eq!(resumed.id, meta.id);
        cleanup(&root);
    }

    #[test]
    fn end_marks_ended_and_clears_current() {
        let root = temp_store_root("end");
        let store = SessionStore::new(&root);
        let meta = store.create().expect("create");
        let ended = store.end(&meta.id).expect("end");
        assert_eq!(ended.status, SessionStatus::Ended);
        assert_eq!(store.current_id().unwrap(), None);
        cleanup(&root);
    }
}
