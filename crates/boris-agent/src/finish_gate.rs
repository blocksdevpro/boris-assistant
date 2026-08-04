//! Finish gate: nudge the model to keep working when todos remain open.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
struct TodoItem {
    #[serde(default)]
    status: String,
}

/// Count pending todos under the sandbox `todos.json` (best-effort).
pub fn pending_todo_count(sandbox_root: &Path) -> usize {
    let path = sandbox_root.join("todos.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return 0;
    };
    let Ok(items) = serde_json::from_str::<Vec<TodoItem>>(&raw) else {
        return 0;
    };
    items
        .iter()
        .filter(|t| {
            let s = t.status.to_ascii_lowercase();
            s == "pending" || s == "in_progress" || s == "open" || s.is_empty()
        })
        .count()
}

/// System-reminder text when open todos remain after a content-only reply.
pub fn todo_gate_reminder(pending: usize) -> String {
    format!(
        "<system-reminder>\n\
         You still have {pending} open todo(s). Do not stop yet. \
         Continue with tools (todo_write to update, then the next work steps) \
         until the list is done or you need one short question from the human. \
         Stay silent on tool dumps — only speak when truly finished or blocked.\n\
         </system-reminder>"
    )
}

/// Resolve sandbox root from common host layouts.
pub fn default_sandbox_guess() -> PathBuf {
    if let Ok(h) = std::env::var("BORIS_HOME") {
        let p = PathBuf::from(h).join("sandbox");
        if p.is_dir() || p.parent().is_some() {
            return p;
        }
    }
    if let Ok(h) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        return PathBuf::from(h).join(".boris").join("sandbox");
    }
    PathBuf::from(".boris-sandbox")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn counts_pending() {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("boris-todos-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("todos.json"),
            r#"[{"id":"1","content":"a","status":"pending"},{"id":"2","content":"b","status":"done"}]"#,
        )
        .unwrap();
        assert_eq!(pending_todo_count(&dir), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
