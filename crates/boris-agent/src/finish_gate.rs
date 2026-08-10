//! Finish gate: nudge the model to keep working when todos remain open.
//!
//! Used by the agent loop after a content-only reply: if `todos.json` still has
//! pending items, inject a [`todo_gate_reminder`] as a user system-reminder.
//!
//! All helpers are pure / best-effort FS reads — never panic on missing files.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
struct TodoItem {
    #[serde(default)]
    status: String,
}

/// Status strings treated as still-open work.
fn is_open_status(status: &str) -> bool {
    let s = status.to_ascii_lowercase();
    s == "pending" || s == "in_progress" || s == "open" || s.is_empty()
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
    items.iter().filter(|t| is_open_status(&t.status)).count()
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

/// Resolve agent write root from common host layouts (`~/.boris/state/workspace`).
pub fn default_sandbox_guess() -> PathBuf {
    if let Ok(h) = std::env::var("BORIS_HOME") {
        return PathBuf::from(h).join("state").join("workspace");
    }
    if let Ok(h) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        return PathBuf::from(h).join(".boris").join("state").join("workspace");
    }
    PathBuf::from(".boris-workspace")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("boris-todos-{tag}-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn counts_pending() {
        let dir = temp_dir("count");
        std::fs::write(
            dir.join("todos.json"),
            r#"[{"id":"1","content":"a","status":"pending"},{"id":"2","content":"b","status":"done"}]"#,
        )
        .unwrap();
        assert_eq!(pending_todo_count(&dir), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn counts_in_progress_open_and_empty_status() {
        let dir = temp_dir("open-statuses");
        std::fs::write(
            dir.join("todos.json"),
            r#"[
                {"id":"1","content":"a","status":"in_progress"},
                {"id":"2","content":"b","status":"open"},
                {"id":"3","content":"c","status":""},
                {"id":"4","content":"d","status":"COMPLETED"},
                {"id":"5","content":"e","status":"done"}
            ]"#,
        )
        .unwrap();
        assert_eq!(pending_todo_count(&dir), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_returns_zero() {
        let dir = temp_dir("missing");
        assert_eq!(pending_todo_count(&dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_json_returns_zero() {
        let dir = temp_dir("bad-json");
        std::fs::write(dir.join("todos.json"), "not-json").unwrap();
        assert_eq!(pending_todo_count(&dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_array_returns_zero() {
        let dir = temp_dir("empty");
        std::fs::write(dir.join("todos.json"), "[]").unwrap();
        assert_eq!(pending_todo_count(&dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_open_status_cases() {
        assert!(is_open_status("pending"));
        assert!(is_open_status("PENDING"));
        assert!(is_open_status("in_progress"));
        assert!(is_open_status("open"));
        assert!(is_open_status(""));
        assert!(!is_open_status("done"));
        assert!(!is_open_status("completed"));
        assert!(!is_open_status("cancelled"));
    }

    #[test]
    fn todo_gate_reminder_includes_count_and_tags() {
        let s = todo_gate_reminder(3);
        assert!(s.contains("<system-reminder>"));
        assert!(s.contains("</system-reminder>"));
        assert!(s.contains("3 open todo"));
        assert!(s.contains("todo_write"));
    }

    #[test]
    fn default_sandbox_guess_prefers_boris_home() {
        // Only assert shape when BORIS_HOME is set; otherwise path is user-dependent.
        if let Ok(h) = std::env::var("BORIS_HOME") {
            let p = default_sandbox_guess();
            assert_eq!(p, PathBuf::from(h).join("state").join("workspace"));
        } else {
            let p = default_sandbox_guess();
            // USERPROFILE/HOME path or local fallback
            assert!(
                p.ends_with(Path::new(".boris").join("state").join("workspace"))
                    || p == PathBuf::from(".boris-workspace")
            );
        }
    }
}
