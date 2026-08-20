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

/// Needles that mark person / social-profile lookup (stronger research path).
const PERSON_FIND_NEEDLES: &[&str] = &[
    "linkedin",
    "linked in",
    "github",
    "profile",
    "who is",
    "find my",
    "find me",
    "find their",
    "look up",
    "look for",
    "my linkedin",
    "my github",
    "social",
];

/// True when the latest user text looks like a research / find-person / lookup ask.
pub fn looks_like_research_request(user_text: &str) -> bool {
    let lower = user_text.to_ascii_lowercase();
    RESEARCH_NEEDLES.iter().any(|n| lower.contains(n))
}

/// True when the user is asking to find a person or social profile.
pub fn looks_like_person_find(user_text: &str) -> bool {
    let lower = user_text.to_ascii_lowercase();
    PERSON_FIND_NEEDLES.iter().any(|n| lower.contains(n))
}

/// Count tool names equal to `name` in `tools_used`.
fn count_tool(tools_used: &[String], name: &str) -> usize {
    tools_used.iter().filter(|t| t.as_str() == name).count()
}

/// Weak web research for general lookup: fewer than 2 among {web_search, web_fetch}
/// or zero web_search.
///
/// Person/profile finds need more: fewer than 3 web_search, or zero fetch when
/// any search ran (must verify candidates).
pub fn research_effort_weak(tools_used: &[String]) -> bool {
    research_effort_weak_for(tools_used, false)
}

/// Same as [`research_effort_weak`] with optional person-find bar.
pub fn research_effort_weak_for(tools_used: &[String], person_find: bool) -> bool {
    let web_search = count_tool(tools_used, "web_search");
    let web_fetch = count_tool(tools_used, "web_fetch");
    if person_find {
        // Minimum: 3 searches, or 2 searches + 1 fetch.
        return web_search < 3 && !(web_search >= 2 && web_fetch >= 1);
    }
    let total = web_search + web_fetch;
    total < 2 || web_search == 0
}

/// True when the spoken reply claims research came up empty / failed.
pub fn looks_like_gave_up_research(reply: &str) -> bool {
    let lower = reply.to_ascii_lowercase();
    GAVE_UP_NEEDLES.iter().any(|n| lower.contains(n))
}

/// Conservative research finish-gate when observation quality is unavailable.
///
/// Production loop callers should use [`should_research_gate_with`] and pass
/// the number of useful observations. Raw calls alone never count as evidence.
pub fn should_research_gate(user_text: &str, reply: &str, tools_used: &[String]) -> bool {
    should_research_gate_with(user_text, reply, tools_used, 0)
}

/// Same as [`should_research_gate`] with a count of useful (non-error) observations.
pub fn should_research_gate_with(
    user_text: &str,
    reply: &str,
    tools_used: &[String],
    useful_results: u32,
) -> bool {
    let _ = reply;
    let traits = crate::task::classify_task(user_text);
    if traits.research_depth == crate::task::ResearchDepth::None {
        return false;
    }
    let coverage = crate::task::EvidenceCoverage::from_tools(tools_used, useful_results);
    !coverage.meets(traits.research_depth)
}

/// True when the user asked for local workspace work and no file/search/shell
/// tools ran this turn.
pub fn should_local_work_gate(user_text: &str, tools_used: &[String]) -> bool {
    let traits = crate::task::classify_task(user_text);
    if !traits.is_workspace_job(user_text) {
        return false;
    }
    !tools_used.iter().any(|n| {
        matches!(
            n.as_str(),
            "grep" | "glob" | "file_read" | "list_dir" | "bash" | "file_edit" | "file_write"
        )
    })
}

/// System-reminder when local workspace work was under-tooled.
pub fn local_work_gate_reminder() -> String {
    "<system-reminder>\n\
     This is local workspace work. Use grep, glob, list_dir, and file_read \
     (not bash cat/grep/find, not web_search) before answering. \
     Batch independent searches and reads in one multi-tool message. \
     Do not claim you looked unless tool output says so.\n\
     </system-reminder>"
        .to_string()
}

/// System-reminder when research was under-tooled after a content-only reply.
pub fn research_gate_reminder() -> String {
    "<system-reminder>\n\
     You under-tooled this research. Use real API tool_calls only (never tool XML in speech). \
     Fan out 3+ web_search queries with different angles in one multi-tool message \
     (name+city, job, site:linkedin.com, company), then web_fetch 2+ candidates. \
     For a high-confidence profile match you may speak ONE URL or call open_url. \
     Do not conclude not found yet. Stay silent until you have real evidence or true exhaustion.\n\
     </system-reminder>"
        .to_string()
}

/// Compact playbook injected for person/profile finds when the research skill loads.
pub fn person_find_skill_nudge(skill_body: &str) -> String {
    format!(
        "<system-reminder>\n\
         Person/profile research request. Follow this research playbook using real API tool_calls only \
         (never write tool XML, invoke tags, or tool JSON in speech). \
         Word limit applies only to the final spoken line after tools. \
         When you have a verified profile, speak one short line and may include exactly one URL, \
         or call open_url for the user.\n\n\
         {skill_body}\n\
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

    // ── Research helpers ────────────────────────────────────────────────────

    #[test]
    fn research_request_needles() {
        assert!(looks_like_research_request("Find Jane Doe on LinkedIn"));
        assert!(looks_like_research_request("look up the CEO of Acme"));
        assert!(looks_like_research_request("Who is Satya Nadella?"));
        assert!(looks_like_research_request(
            "search for rust async runtimes"
        ));
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
    fn should_research_gate_zero_tools_freestyle() {
        // LinkedIn freestyle with no tools must re-enter.
        assert!(should_research_gate(
            "find my LinkedIn based on my hints",
            "Hit me with the first hint, bro.",
            &[]
        ));
    }

    #[test]
    fn should_research_gate_adequate_effort_skips() {
        let tools = vec![s("web_search"), s("web_search")];
        assert!(!should_research_gate_with(
            "search for rust async runtimes",
            "I couldn't find her.",
            &tools,
            2,
        ));
        let tools2 = vec![s("web_search"), s("web_fetch")];
        assert!(!should_research_gate_with(
            "research the latest tokio release",
            "Not found.",
            &tools2,
            2,
        ));
    }

    #[test]
    fn person_find_needs_more_effort() {
        // 2 searches alone is not enough for LinkedIn person-find.
        let tools = vec![s("web_search"), s("web_search")];
        assert!(should_research_gate(
            "find Jane Doe on LinkedIn",
            "I think I found her.",
            &tools
        ));
        // 2 search + 1 fetch is adequate for person-find.
        let tools_ok = vec![s("web_search"), s("web_search"), s("web_fetch")];
        assert!(!should_research_gate_with(
            "find Jane Doe on LinkedIn",
            "Here is the profile.",
            &tools_ok,
            2,
        ));
    }

    #[test]
    fn failed_search_calls_do_not_count_as_evidence() {
        let tools = vec![s("web_search"), s("web_search"), s("web_fetch")];
        assert!(should_research_gate_with(
            "find Jane Doe on LinkedIn",
            "I couldn't find her.",
            &tools,
            0,
        ));
    }

    #[test]
    fn looks_like_person_find_needles() {
        assert!(looks_like_person_find("find my LinkedIn"));
        assert!(looks_like_person_find("look up Jane on GitHub"));
        assert!(!looks_like_person_find("what time is it"));
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
        // Research ask, only local tools → still nudge (weak effort).
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
        assert!(s.contains("tool_calls") || s.contains("API"));
    }

    fn s(name: &str) -> String {
        name.to_string()
    }

    #[test]
    fn finish_gate_false_positive_local_find() {
        // "find" in a local-file ask must not trip the research gate.
        assert!(!should_research_gate(
            "find the function in src/main.rs",
            "It's in main.",
            &[]
        ));
    }

    #[test]
    fn finish_gate_false_negative_zero_tools_person() {
        assert!(should_research_gate(
            "find Jane Doe on LinkedIn",
            "Need more hints.",
            &[]
        ));
    }

    #[test]
    fn local_work_gate_on_undertooled_file_ask() {
        assert!(should_local_work_gate(
            "find the function in src/main.rs",
            &[]
        ));
        assert!(!should_local_work_gate(
            "find the function in src/main.rs",
            &[s("grep")]
        ));
        assert!(!should_local_work_gate("what time is it", &[]));
        assert!(!should_local_work_gate("find Jane Doe on LinkedIn", &[]));
        let reminder = local_work_gate_reminder();
        assert!(reminder.contains("grep"));
        assert!(reminder.contains("<system-reminder>"));
    }
}
