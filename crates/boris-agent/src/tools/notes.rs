//! File-backed mini memory: append-only notes JSONL.
//!
//! Path root is **injected** via [`NotesStore::new`] / tool constructors — callers
//! (pipeline / desktop) supply e.g. `~/.boris/memory/notes.jsonl`. Do not hardcode
//! home directories inside this module.
//!
//! # On-disk format
//!
//! One JSON object per line (JSONL):
//! ```json
//! {"ts_ms":1712345678901,"note":"buy milk"}
//! ```

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::tool::{
    optional_string, require_object, require_string, truncate_tool_result, Permission, Tool,
    ToolError, ToolMeta, ToolRisk,
};

const DEFAULT_RECALL_LIMIT: usize = 5;
const MAX_RECALL_LIMIT: usize = 20;

// ── Store ─────────────────────────────────────────────────────────────────────

/// Append-only notes store backed by a single JSONL file.
#[derive(Debug, Clone)]
pub struct NotesStore {
    path: PathBuf,
}

/// One line in the notes file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct NoteRecord {
    ts_ms: u64,
    note: String,
}

impl NotesStore {
    /// Create a store for the given file path (does not create the file yet).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Path to the JSONL file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one note as a JSONL line. Creates parent directories if needed.
    pub fn append(&self, note: &str) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("create notes parent dir {}: {e}", parent.display()))?;
            }
        }

        let record = NoteRecord {
            ts_ms: now_ms(),
            note: note.to_string(),
        };
        let line = serde_json::to_string(&record).map_err(|e| format!("serialize note: {e}"))?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("open notes {}: {e}", self.path.display()))?;

        writeln!(file, "{line}")
            .map_err(|e| format!("write notes {}: {e}", self.path.display()))?;
        file.flush()
            .map_err(|e| format!("flush notes {}: {e}", self.path.display()))?;
        Ok(())
    }

    /// Most recent notes (newest first), up to `limit`.
    pub fn list_recent(&self, limit: usize) -> Result<Vec<String>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let all = self.read_all()?;
        let take = limit.min(all.len());
        Ok(all.into_iter().rev().take(take).map(|r| r.note).collect())
    }

    /// Case-insensitive substring search; newest matches first, up to `limit`.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<String>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let q = query.to_lowercase();
        let all = self.read_all()?;
        let mut out = Vec::new();
        for rec in all.into_iter().rev() {
            if rec.note.to_lowercase().contains(&q) {
                out.push(rec.note);
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Read every well-formed record in file order. Missing file → empty.
    /// Blank lines skipped; malformed lines skipped (warn via tracing).
    fn read_all(&self) -> Result<Vec<NoteRecord>, String> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let file = OpenOptions::new()
            .read(true)
            .open(&self.path)
            .map_err(|e| format!("open notes {}: {e}", self.path.display()))?;

        let reader = BufReader::new(file);
        let mut out = Vec::new();

        for (idx, line_res) in reader.lines().enumerate() {
            let line = line_res.map_err(|e| format!("read notes {}: {e}", self.path.display()))?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<NoteRecord>(trimmed) {
                Ok(rec) => out.push(rec),
                Err(err) => {
                    tracing::warn!(
                        path = %self.path.display(),
                        line = idx + 1,
                        error = %err,
                        "skipping malformed notes line"
                    );
                }
            }
        }

        Ok(out)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Tools ─────────────────────────────────────────────────────────────────────

/// LLM tool: persist a short note to the local notes file.
#[derive(Debug, Clone)]
pub struct RememberNoteTool {
    store: NotesStore,
}

impl RememberNoteTool {
    /// Build a tool writing to the given JSONL path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            store: NotesStore::new(path),
        }
    }

    /// Shared store (path) this tool writes to.
    pub fn store(&self) -> &NotesStore {
        &self.store
    }
}

#[async_trait]
impl Tool for RememberNoteTool {
    fn name(&self) -> &str {
        "remember_note"
    }

    fn description(&self) -> &str {
        "Save a short note to local memory for later recall. Use for facts, reminders, or preferences the user wants remembered."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note": {
                    "type": "string",
                    "description": "The note text to remember"
                }
            },
            "required": ["note"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Moderate).permissions(&[Permission::FsWrite])
    }

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let note = require_string(obj, "note")?;
        self.store
            .append(&note)
            .map_err(|e| ToolError::failed(format!("failed to save note: {e}")))?;
        Ok(truncate_tool_result("Saved note.".to_string()))
    }
}

/// LLM tool: list recent notes or search by substring.
#[derive(Debug, Clone)]
pub struct RecallNotesTool {
    store: NotesStore,
}

impl RecallNotesTool {
    /// Build a tool reading from the given JSONL path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            store: NotesStore::new(path),
        }
    }

    /// Shared store (path) this tool reads from.
    pub fn store(&self) -> &NotesStore {
        &self.store
    }
}

#[async_trait]
impl Tool for RecallNotesTool {
    fn name(&self) -> &str {
        "recall_notes"
    }

    fn description(&self) -> &str {
        "Recall notes from local memory. Optionally filter with a case-insensitive query; otherwise returns the most recent notes."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Optional case-insensitive substring to search for"
                },
                "limit": {
                    "type": "number",
                    "description": "Max notes to return (default 5, max 20)"
                }
            },
            "required": []
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe).permissions(&[Permission::FsRead])
    }

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let limit = parse_limit(obj)?;
        let query = optional_string(obj, "query");

        let notes = match query {
            Some(q) if !q.trim().is_empty() => self
                .store
                .search(q.trim(), limit)
                .map_err(|e| ToolError::failed(format!("failed to search notes: {e}")))?,
            _ => self
                .store
                .list_recent(limit)
                .map_err(|e| ToolError::failed(format!("failed to list notes: {e}")))?,
        };

        Ok(truncate_tool_result(format_notes_list(&notes)))
    }
}

fn parse_limit(obj: &Map<String, Value>) -> Result<usize, ToolError> {
    match obj.get("limit") {
        None | Some(Value::Null) => Ok(DEFAULT_RECALL_LIMIT),
        Some(v) => {
            let n = v
                .as_u64()
                .or_else(|| {
                    v.as_i64()
                        .and_then(|i| if i >= 0 { Some(i as u64) } else { None })
                })
                .or_else(|| {
                    v.as_f64().and_then(|f| {
                        if f.is_finite() && f >= 0.0 {
                            Some(f as u64)
                        } else {
                            None
                        }
                    })
                })
                .ok_or_else(|| {
                    ToolError::invalid_args("argument `limit` must be a non-negative number")
                })?;
            Ok((n as usize).min(MAX_RECALL_LIMIT))
        }
    }
}

fn format_notes_list(notes: &[String]) -> String {
    if notes.is_empty() {
        return "No notes found.".to_string();
    }
    notes
        .iter()
        .map(|n| format!("- {n}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_notes_path(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("boris-notes-{nanos}-{n}-{label}"));
        let _ = fs::remove_dir_all(&dir);
        dir.join("notes.jsonl")
    }

    fn cleanup(path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn append_creates_parent_and_jsonl_shape() {
        let path = temp_notes_path("append");
        let store = NotesStore::new(&path);
        store.append("buy milk").expect("append");

        assert!(path.is_file());
        let raw = fs::read_to_string(&path).unwrap();
        let line = raw.lines().next().expect("one line");
        let v: Value = serde_json::from_str(line).unwrap();
        assert!(v["ts_ms"].as_u64().unwrap() > 0);
        assert_eq!(v["note"], "buy milk");

        cleanup(&path);
    }

    #[test]
    fn list_recent_newest_first_respects_limit() {
        let path = temp_notes_path("recent");
        let store = NotesStore::new(&path);
        store.append("one").unwrap();
        store.append("two").unwrap();
        store.append("three").unwrap();

        let recent = store.list_recent(2).unwrap();
        assert_eq!(recent, vec!["three".to_string(), "two".to_string()]);

        cleanup(&path);
    }

    #[test]
    fn search_case_insensitive_contains() {
        let path = temp_notes_path("search");
        let store = NotesStore::new(&path);
        store.append("Buy Milk").unwrap();
        store.append("walk dog").unwrap();
        store.append("MILK tea").unwrap();

        let hits = store.search("milk", 10).unwrap();
        assert_eq!(hits.len(), 2);
        // newest first
        assert_eq!(hits[0], "MILK tea");
        assert_eq!(hits[1], "Buy Milk");

        cleanup(&path);
    }

    #[tokio::test]
    async fn remember_and_recall_tools_roundtrip() {
        let path = temp_notes_path("tools");
        let remember = RememberNoteTool::new(&path);
        let recall = RecallNotesTool::new(&path);

        assert_eq!(remember.name(), "remember_note");
        assert_eq!(recall.name(), "recall_notes");

        let saved = remember
            .execute(json!({ "note": "user likes dark mode" }))
            .await
            .expect("remember");
        assert_eq!(saved, "Saved note.");

        let listed = recall.execute(json!({})).await.expect("recall");
        assert!(listed.contains("user likes dark mode"), "got: {listed}");
        assert!(listed.starts_with("- "), "got: {listed}");

        let searched = recall
            .execute(json!({ "query": "DARK", "limit": 5 }))
            .await
            .expect("search");
        assert!(searched.contains("dark mode"), "got: {searched}");

        cleanup(&path);
    }

    #[tokio::test]
    async fn recall_missing_file_is_empty_message() {
        let path = temp_notes_path("missing");
        let recall = RecallNotesTool::new(&path);
        let out = recall.execute(json!({})).await.unwrap();
        assert_eq!(out, "No notes found.");
        cleanup(&path);
    }

    #[tokio::test]
    async fn remember_requires_note() {
        let path = temp_notes_path("req");
        let tool = RememberNoteTool::new(&path);
        let err = tool.execute(json!({})).await.unwrap_err();
        assert!(err.message.contains("note"), "got: {}", err.message);
        cleanup(&path);
    }

    #[tokio::test]
    async fn limit_defaults_and_caps() {
        let path = temp_notes_path("limit");
        let store = NotesStore::new(&path);
        for i in 0..25 {
            store.append(&format!("n{i}")).unwrap();
        }
        let recall = RecallNotesTool::new(&path);

        // default 5
        let out = recall.execute(json!({})).await.unwrap();
        let count = out.lines().count();
        assert_eq!(count, 5, "got:\n{out}");

        // cap at 20
        let out = recall.execute(json!({ "limit": 100 })).await.unwrap();
        assert_eq!(out.lines().count(), 20);

        // explicit 3
        let out = recall.execute(json!({ "limit": 3 })).await.unwrap();
        assert_eq!(out.lines().count(), 3);

        cleanup(&path);
    }

    #[test]
    fn skips_malformed_lines() {
        let path = temp_notes_path("malformed");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(
            &path,
            r#"
{"ts_ms":1,"note":"ok"}

not-json
{"ts_ms":2,"note":"fine"}
{"bad":true}
"#,
        )
        .unwrap();

        let store = NotesStore::new(&path);
        let recent = store.list_recent(10).unwrap();
        assert_eq!(recent, vec!["fine".to_string(), "ok".to_string()]);

        cleanup(&path);
    }
}
