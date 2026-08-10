//! Finish gate: nudge the model to keep working when todos remain open
//! or when research was under-tooled.
//!
//! Used by the agent loop after a content-only reply:
//! - if the session (or sandbox) todos file still has pending items → [`todo_gate_reminder`]
//! - if the user asked for research/lookup and effort was weak → [`research_gate_reminder`]
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
         You still have {pending} open todo(s). Do not stop yet. \
         Continue with tools (todo_write to update, then the next work steps) \
         until the list is done or you need one short question from the human. \
         Stay silent on tool dumps — only speak when truly finished or blocked.\n\
         </system-reminder>"
    )
}

// ── Research effort gate ────────────────────────────────────────────────────

/// Case-insensitive needles that mark a user message as a research / lookup ask.
const RESEARCH_NEEDLES: &[&str] = &[
    "look up",
    "look for",
    "find out",
    "who is",
    "where is",
    "linkedin",
    "github",
    "profile",
    "research",
    "search",
    "investigate",
    "find",
];

/// Phrases that signal the model gave up on research too early.
const GAVE_UP_NEEDLES: &[&str] = &[
    "not found",
    "couldn't find",
    "could not find",
    "failed to find",
    "no results",
    "nothing",
    "empty",
    "unable to find",
    "no information",
    "i couldn't",
    "i could not",
];

/// True when the latest user text looks like a research / find-person / lookup ask.
pub fn looks_like_research_request(user_text: &str) -> bool {
    let lower = user_text.to_ascii_lowercase();
    RESEARCH_NEEDLES.iter().any(|n| lower.contains(n))
}

/// Count tool names equal to `name` in `tools_used`.
fn count_tool(tools_used: &[String], name: &str) -> usize {
    tools_used.iter().filter(|t| t.as_str() == name).count()
}

/// Weak web research: fewer than 2 total among {web_search, web_fetch}, or zero web_search.
///
/// Adequate effort is either 2+ web_search calls, or at least one search plus one fetch
/// (total ≥ 2 with ≥ 1 search).
pub fn research_effort_weak(tools_used: &[String]) -> bool {
    let web_search = count_tool(tools_used, "web_search");
    let web_fetch = count_tool(tools_used, "web_fetch");
    let total = web_search + web_fetch;
    total < 2 || web_search == 0
}

/// True when the spoken reply claims research came up empty / failed.
pub fn looks_like_gave_up_research(reply: &str) -> bool {
    let lower = reply.to_ascii_lowercase();
    GAVE_UP_NEEDLES.iter().any(|n| lower.contains(n))
}

/// Whether the research finish-gate should fire for this speak.
///
/// Re-enter only when research intent + weak effort + (gave-up speech OR at least
/// one `web_search` was used). One lazy search + "not found" (or any weak web
/// speak) gets nudged to fan out more queries.
pub fn should_research_gate(user_text: &str, reply: &str, tools_used: &[String]) -> bool {
    if !looks_like_research_request(user_text) || !research_effort_weak(tools_used) {
        return false;
    }
    let has_web_search = count_tool(tools_used, "web_search") >= 1;
    looks_like_gave_up_research(reply) || has_web_search
}

/// System-reminder when research was under-tooled after a content-only reply.
pub fn research_gate_reminder() -> String {
    "<system-reminder>\n\
     You under-tooled this research. Fan out 3+ web_search queries with different \
     angles in one multi-tool message (name+city, job, site:linkedin.com, company), \
     then web_fetch 2+ candidates. Do not conclude not found yet. Stay silent until \
     you have real evidence or true exhaustion.\n\
     </system-reminder>"
        .to_string()
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
                    || p == PathBuf::from(".boris-workspace")
            );
        }
    }

    // ── Research helpers ────────────────────────────────────────────────────

    #[test]
    fn research_request_needles() {
        assert!(looks_like_research_request("Find Jane Doe on LinkedIn"));
        assert!(looks_like_research_request("look up the CEO of Acme"));
        assert!(looks_like_research_request("Who is Satya Nadella?"));
        assert!(looks_like_research_request("search for rust async runtimes"));
        assert!(looks_like_research_request("research this company"));
        assert!(looks_like_research_request("investigate the outage"));
        assert!(looks_like_research_request("where is their github profile"));
        assert!(!looks_like_research_request("what time is it"));
        assert!(!looks_like_research_request("summarize this file"));
    }

    #[test]
    fn research_effort_weak_counts() {
        assert!(research_effort_weak(&[]));
        assert!(research_effort_weak(&[s("bash")]));
        assert!(research_effort_weak(&[s("web_search")]));
        assert!(research_effort_weak(&[s("web_fetch"), s("web_fetch")])); // zero search
        assert!(!research_effort_weak(&[s("web_search"), s("web_search")]));
        assert!(!research_effort_weak(&[s("web_search"), s("web_fetch")]));
        assert!(!research_effort_weak(&[
            s("web_search"),
            s("web_search"),
            s("web_fetch")
        ]));
    }

    #[test]
    fn gave_up_research_needles() {
        assert!(looks_like_gave_up_research("I couldn't find anything."));
        assert!(looks_like_gave_up_research("Not found on LinkedIn."));
        assert!(looks_like_gave_up_research("No results for that name."));
        assert!(looks_like_gave_up_research("Search returned empty."));
        assert!(!looks_like_gave_up_research(
            "Jane Doe is a software engineer at Acme."
        ));
    }

    #[test]
    fn should_research_gate_lazy_search_not_found() {
        let tools = vec![s("web_search")];
        assert!(should_research_gate(
            "find John Smith linkedin",
            "I couldn't find John Smith.",
            &tools
        ));
    }

    #[test]
    fn should_research_gate_weak_search_even_without_gave_up() {
        // Prefer: one web_search + research intent + weak effort re-enters.
        let tools = vec![s("web_search")];
        assert!(should_research_gate(
            "look up Jane Doe",
            "Jane works at Acme Corp.",
            &tools
        ));
    }

    #[test]
    fn should_research_gate_adequate_effort_skips() {
        let tools = vec![s("web_search"), s("web_search")];
        assert!(!should_research_gate(
            "find Jane Doe",
            "I couldn't find her.",
            &tools
        ));
        let tools2 = vec![s("web_search"), s("web_fetch")];
        assert!(!should_research_gate(
            "research Jane Doe",
            "Not found.",
            &tools2
        ));
    }

    #[test]
    fn should_research_gate_no_research_intent() {
        let tools = vec![s("web_search")];
        assert!(!should_research_gate(
            "what time is it in Tokyo",
            "I couldn't find that.",
            &tools
        ));
    }

    #[test]
    fn should_research_gate_gave_up_without_web_tools() {
        // Research ask, only local tools, gave-up speech → still nudge.
        let tools = vec![s("bash")];
        assert!(should_research_gate(
            "search for the maintainer of serde",
            "I couldn't find anything.",
            &tools
        ));
    }

    #[test]
    fn research_gate_reminder_shape() {
        let s = research_gate_reminder();
        assert!(s.contains("<system-reminder>"));
        assert!(s.contains("</system-reminder>"));
        assert!(s.contains("under-tooled"));
        assert!(s.contains("web_search"));
        assert!(s.contains("web_fetch"));
    }

    fn s(name: &str) -> String {
        name.to_string()
    }
}
