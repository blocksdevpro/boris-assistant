//! Request-local tool bundles selected without a model discovery round.

use std::collections::HashSet;

use crate::task::{classify_task, ResearchDepth, TaskTraits};
use crate::tool::ToolKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Domain {
    Workspace,
    Research,
    Recall,
    Profile,
    Planning,
    Time,
    System,
    Clipboard,
    Artifacts,
    Open,
    Workflow,
    Skills,
}

/// A stable bundle for one objective. Discovery adds tools independently.
#[derive(Debug, Clone)]
pub struct ToolSelection {
    pub task: TaskTraits,
    domains: HashSet<Domain>,
}

impl ToolSelection {
    pub fn for_task(task: TaskTraits) -> Self {
        let mut domains = HashSet::new();
        if task.coding {
            domains.insert(Domain::Workspace);
        }
        if task.research_depth != ResearchDepth::None
            || (!task.local_only && !task.coding && !task.time_date)
        {
            domains.extend([Domain::Research, Domain::Recall]);
        }
        if task.multi_step || task.coding {
            domains.insert(Domain::Planning);
        }
        if task.time_date {
            domains.insert(Domain::Time);
        }
        if task.needs_strong() || domains.contains(&Domain::Research) {
            domains.insert(Domain::Workflow);
        }
        Self { task, domains }
    }

    pub fn for_request(user_text: &str) -> Self {
        let mut selection = Self::for_task(classify_task(user_text));
        let text = user_text.to_ascii_lowercase();
        let words = text
            .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
            .filter(|word| !word.is_empty())
            .collect::<HashSet<_>>();
        let mut include = |domain, needles: &[&str]| {
            if needles.iter().any(|needle| words.contains(needle)) {
                selection.domains.insert(domain);
            }
        };
        include(
            Domain::Workspace,
            &[
                "file",
                "files",
                "folder",
                "directory",
                "workspace",
                "repo",
                "repository",
                "grep",
                "glob",
                "git",
                "cargo",
                "shell",
                "terminal",
                "command",
                "bash",
                "powershell",
            ],
        );
        include(
            Domain::Recall,
            &[
                "remember",
                "recall",
                "memory",
                "memories",
                "notes",
                "preferences",
                "earlier",
                "previously",
            ],
        );
        include(
            Domain::Research,
            &["web", "online", "internet", "browse", "weather", "news"],
        );
        include(
            Domain::Profile,
            &["forget", "profile", "preference", "prefer", "name"],
        );
        include(Domain::Planning, &["todo", "todos", "tasks", "plan"]);
        include(Domain::System, &["system", "cpu", "ram", "operating"]);
        include(Domain::Clipboard, &["clipboard"]);
        include(
            Domain::Artifacts,
            &[
                "artifact",
                "artifacts",
                "card",
                "cards",
                "report",
                "reports",
            ],
        );
        include(Domain::Skills, &["skill", "skills", "playbook"]);
        include(Domain::Open, &["open", "launch"]);
        if text.contains("http://") || text.contains("https://") {
            selection.domains.insert(Domain::Research);
        }
        if selection
            .domains
            .iter()
            .any(|domain| !matches!(domain, Domain::Time | Domain::Profile))
        {
            selection.domains.insert(Domain::Workflow);
        }
        selection
    }

    /// Builtin bundles are eager; plugin category matches compete for top-k.
    pub(crate) fn score(&self, name: &str, kind: ToolKind) -> i32 {
        let domain = match name {
            "file_read" | "file_write" | "file_edit" | "list_dir" | "glob" | "grep" | "bash" => {
                Some(Domain::Workspace)
            }
            "web_search" | "web_fetch" => Some(Domain::Research),
            "memory_search"
            | "memory_get"
            | "memory_search_files"
            | "memory_get_file"
            | "recall_notes" => Some(Domain::Recall),
            "save_user_fact"
            | "update_user_profile"
            | "forget_user_memory"
            | "forget_memory"
            | "get_user_context" => Some(Domain::Profile),
            "todo_read" | "todo_write" => Some(Domain::Planning),
            "get_time" | "get_date" => Some(Domain::Time),
            "get_system_info" => Some(Domain::System),
            "clipboard_get" | "clipboard_set" => Some(Domain::Clipboard),
            "get_artifact" | "list_artifacts" => Some(Domain::Artifacts),
            "open_url" | "open_path" => Some(Domain::Open),
            "parallel" | "get_tool_output" => Some(Domain::Workflow),
            "load_skill" => Some(Domain::Skills),
            _ => None,
        };
        if let Some(domain) = domain {
            return if self.domains.contains(&domain)
                || (domain == Domain::Skills && self.domains.contains(&Domain::Workflow))
            {
                400
            } else {
                0
            };
        }
        let domain = match kind {
            ToolKind::Read | ToolKind::Write | ToolKind::Search | ToolKind::Execute => {
                Domain::Workspace
            }
            ToolKind::Web => Domain::Research,
            ToolKind::Memory => Domain::Recall,
            ToolKind::Skill => Domain::Skills,
            ToolKind::System => Domain::System,
            ToolKind::Plan => Domain::Planning,
            ToolKind::Other => return 0,
        };
        if self.domains.contains(&domain) {
            200
        } else {
            0
        }
    }
}

/// Short follow-ups inherit the preceding objective's bundle, not old greetings.
pub(crate) fn is_task_followup(text: &str) -> bool {
    let words = text
        .to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let phrase = words
        .iter()
        .filter(|word| {
            !matches!(
                word.as_str(),
                "uh" | "um" | "please" | "boris" | "just" | "okay" | "ok"
            )
        })
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    matches!(
        phrase.as_str(),
        "continue"
            | "go on"
            | "keep going"
            | "carry on"
            | "resume"
            | "try again"
            | "retry"
            | "do it"
            | "yes"
            | "yeah"
            | "yep"
            | "proceed"
            | "finish it"
            | "continue with it"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn website_lookup_loads_research_without_workspace_tools() {
        let selection =
            ToolSelection::for_request("Uh I actually asked you to look up my website.");
        assert_eq!(selection.score("web_search", ToolKind::Web), 400);
        assert_eq!(selection.score("web_fetch", ToolKind::Web), 400);
        assert_eq!(selection.score("memory_search", ToolKind::Memory), 400);
        assert_eq!(selection.score("bash", ToolKind::Execute), 0);
        assert_eq!(selection.score("file_edit", ToolKind::Write), 0);
        let mixed =
            ToolSelection::for_request("debug this repo and search the web for compiler news");
        assert_eq!(mixed.score("bash", ToolKind::Execute), 400);
        assert_eq!(mixed.score("web_search", ToolKind::Web), 400);
    }

    #[test]
    fn local_reads_shell_clipboard_and_time_need_no_discovery() {
        for (request, name, kind) in [
            ("summarize this file", "file_read", ToolKind::Read),
            ("run git status", "bash", ToolKind::Execute),
            ("read my clipboard", "clipboard_get", ToolKind::System),
            ("what time is it", "get_time", ToolKind::System),
            ("what is the weather today", "web_search", ToolKind::Web),
        ] {
            assert_eq!(
                ToolSelection::for_request(request).score(name, kind),
                400,
                "{request}"
            );
        }
    }

    #[test]
    fn followups_are_whole_requests() {
        assert!(is_task_followup("Uh, please continue!"));
        assert!(is_task_followup("try again"));
        assert!(!is_task_followup("continue by searching for Rust news"));
        assert!(!is_task_followup("yes, open my clipboard"));
    }
}
