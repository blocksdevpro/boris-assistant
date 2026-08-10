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
//!
//! # Tools
//!
//! | Tool name | Type | Purpose |
//! |-----------|------|---------|
//! | `remember_note` | [`RememberNoteTool`] | Append a note |
//! | `recall_notes`  | [`RecallNotesTool`]  | List recent / search |
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`store`]    | JSONL I/O + pure list/search |
//! | [`format`]   | pure limit parse + bullet formatting |
//! | [`remember`] | `remember_note` tool |
//! | [`recall`]   | `recall_notes` tool |

mod format;
mod recall;
mod remember;
mod store;

pub use recall::RecallNotesTool;
pub use remember::RememberNoteTool;
pub use store::NotesStore;

/// Default `recall_notes` limit when the model omits `limit`.
pub(crate) const DEFAULT_RECALL_LIMIT: usize = 5;
/// Hard cap on `recall_notes` result count.
pub(crate) const MAX_RECALL_LIMIT: usize = 20;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool;
    use serde_json::{json, Value};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

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
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "note": "user likes dark mode" }),
            )
            .await
            .expect("remember");
        assert_eq!(saved, "Saved note.");

        let listed = recall
            .execute(&crate::tool_context::ToolCallContext::new("t"), json!({}))
            .await
            .expect("recall");
        assert!(listed.contains("user likes dark mode"), "got: {listed}");
        assert!(listed.starts_with("- "), "got: {listed}");

        let searched = recall
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "query": "DARK", "limit": 5 }),
            )
            .await
            .expect("search");
        assert!(searched.contains("dark mode"), "got: {searched}");

        cleanup(&path);
    }

    #[tokio::test]
    async fn recall_missing_file_is_empty_message() {
        let path = temp_notes_path("missing");
        let recall = RecallNotesTool::new(&path);
        let out = recall
            .execute(&crate::tool_context::ToolCallContext::new("t"), json!({}))
            .await
            .unwrap();
        assert_eq!(out, "No notes found.");
        cleanup(&path);
    }

    #[tokio::test]
    async fn remember_requires_note() {
        let path = temp_notes_path("req");
        let tool = RememberNoteTool::new(&path);
        let err = tool
            .execute(&crate::tool_context::ToolCallContext::new("t"), json!({}))
            .await
            .unwrap_err();
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
        let out = recall
            .execute(&crate::tool_context::ToolCallContext::new("t"), json!({}))
            .await
            .unwrap();
        let count = out.lines().count();
        assert_eq!(count, 5, "got:\n{out}");

        // cap at 20
        let out = recall
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "limit": 100 }),
            )
            .await
            .unwrap();
        assert_eq!(out.lines().count(), 20);

        // explicit 3
        let out = recall
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "limit": 3 }),
            )
            .await
            .unwrap();
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
