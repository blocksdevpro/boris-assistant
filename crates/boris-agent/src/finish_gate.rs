//! Finish gate: nudge the model to keep working when todos it created this
//! turn remain open.
//!
//! Used by the agent loop after a content-only reply, and only when the turn
//! already used tools: if the session (or sandbox) todos file still has
//! pending items → [`todo_gate_reminder`]. Keyword gates that guessed
//! "research" or "local work" from the request text were removed; they
//! fought the model with extra rounds. The tool-markup re-prompt and the
//! unknown-tool breaker live in the loop.
//!
//! All helpers are pure / best-effort FS reads — never panic on missing files.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Budget for finish-gate re-entries within one user turn.
///
/// The two counters are independent:
/// - `markup` guards tool-markup-only speech re-prompts (model dumped
///   `<invoke>` XML into `content` instead of using API `tool_calls`).
/// - `gate` guards open-todo reminders after a content-only reply.
///
/// They were previously a single shared counter (init 3), so a markup
/// rejection consumed the todo budget and vice versa.
///
/// Batch-HITL note: one yes/no prompt may cover several sibling tools, but
/// the confirm budget is still per-tool aware (see `loop_::tool_batch`).
/// Likewise this budget is per re-entry, not per tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinishGateBudget {
    /// Remaining markup-only re-prompts.
    pub markup: u32,
    /// Remaining open-todo re-entries.
    pub gate: u32,
}

impl FinishGateBudget {
    /// Fresh-turn markup budget: at most two markup re-prompts.
    pub const MARKUP_INIT: u32 = 2;
    /// Fresh-turn finish-gate budget: at most three open-todo re-entries.
    pub const GATE_INIT: u32 = 3;

    /// Fresh turn: full markup + gate budgets.
    pub fn fresh() -> Self {
        Self {
            markup: Self::MARKUP_INIT,
            gate: Self::GATE_INIT,
        }
    }

    /// No re-entries (gates fully disabled).
    pub fn none() -> Self {
        Self { markup: 0, gate: 0 }
    }

    /// Post-HITL resume: keep markup budget, disable the todo gate so an
    /// approval cannot loop back into finish-gate nudges.
    pub fn post_hitl(markup_left: u32) -> Self {
        Self {
            markup: markup_left,
            gate: 0,
        }
    }

    /// Legacy single-counter mapping (`finish_gate_left: u32`).
    ///
    /// `0` disables both; any non-zero value keeps the full markup budget and
    /// uses the legacy value as the gate budget. Fresh turns should prefer
    /// [`Self::fresh`].
    pub fn from_legacy(left: u32) -> Self {
        if left == 0 {
            Self::none()
        } else {
            Self {
                markup: Self::MARKUP_INIT,
                gate: left,
            }
        }
    }
}

impl Default for FinishGateBudget {
    fn default() -> Self {
        Self::fresh()
    }
}

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

/// Count pending todos in a todos JSON file (best-effort).
///
/// `todos_file` is the full path to `todos.json` (session-local or sandbox).
pub fn pending_todo_count(todos_file: &Path) -> usize {
    let Ok(raw) = std::fs::read_to_string(todos_file) else {
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
         You still have {pending} open todo(s). Continue requested work that remains. \
         Use todo_write to record completed, superseded, or blocked items; optional or stale todos \
         do not expand the user's request. Finish when done or state the real blocker.\n\
         </system-reminder>"
    )
}

/// Resolve agent write root from common host layouts (`~/.boris/state/workspace`).
pub fn default_sandbox_guess() -> PathBuf {
    if let Ok(h) = std::env::var("BORIS_HOME") {
        return PathBuf::from(h).join("state").join("workspace");
    }
    if let Ok(h) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        return PathBuf::from(h)
            .join(".boris")
            .join("state")
            .join("workspace");
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
        let path = dir.join("todos.json");
        std::fs::write(
            &path,
            r#"[{"id":"1","content":"a","status":"pending"},{"id":"2","content":"b","status":"done"}]"#,
        )
        .unwrap();
        assert_eq!(pending_todo_count(&path), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn counts_in_progress_open_and_empty_status() {
        let dir = temp_dir("open-statuses");
        let path = dir.join("todos.json");
        std::fs::write(
            &path,
            r#"[
                {"id":"1","content":"a","status":"in_progress"},
                {"id":"2","content":"b","status":"open"},
                {"id":"3","content":"c","status":""},
                {"id":"4","content":"d","status":"COMPLETED"},
                {"id":"5","content":"e","status":"done"}
            ]"#,
        )
        .unwrap();
        assert_eq!(pending_todo_count(&path), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_returns_zero() {
        let dir = temp_dir("missing");
        assert_eq!(pending_todo_count(&dir.join("todos.json")), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_json_returns_zero() {
        let dir = temp_dir("bad-json");
        let path = dir.join("todos.json");
        std::fs::write(&path, "not-json").unwrap();
        assert_eq!(pending_todo_count(&path), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_array_returns_zero() {
        let dir = temp_dir("empty");
        let path = dir.join("todos.json");
        std::fs::write(&path, "[]").unwrap();
        assert_eq!(pending_todo_count(&path), 0);
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
                    || p == Path::new(".boris-workspace")
            );
        }
    }

    #[test]
    fn finish_gate_budget_splits_markup_and_gate() {
        // B4: fresh turn gets independent budgets; legacy 0 disables both.
        let fresh = FinishGateBudget::fresh();
        assert_eq!(fresh.markup, FinishGateBudget::MARKUP_INIT);
        assert_eq!(fresh.gate, FinishGateBudget::GATE_INIT);
        assert_eq!(FinishGateBudget::MARKUP_INIT, 2);
        assert_eq!(FinishGateBudget::GATE_INIT, 3);

        assert_eq!(
            FinishGateBudget::none(),
            FinishGateBudget { markup: 0, gate: 0 }
        );
        assert_eq!(
            FinishGateBudget::from_legacy(0),
            FinishGateBudget::none(),
            "legacy 0 disables both"
        );
        let mapped = FinishGateBudget::from_legacy(3);
        assert_eq!(mapped.gate, 3);
        assert_eq!(mapped.markup, FinishGateBudget::MARKUP_INIT);

        // Post-HITL keeps markup, forces gate to 0.
        let post = FinishGateBudget::post_hitl(1);
        assert_eq!(post.markup, 1);
        assert_eq!(post.gate, 0);
    }
}
