//! Structured, bounded state for the active task.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Message, MessageOrigin, Role};

const MAX_ENTRY_CHARS: usize = 320;
const MAX_ENTRIES: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[default]
    Idle,
    Active,
    WaitingForInput,
    WaitingForConfirmation,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStateEntry {
    pub text: String,
    /// Stable, human-readable provenance such as `human:turn-3` or
    /// `tool:web_search:call_7`.
    pub provenance: String,
}

impl TaskStateEntry {
    fn new(text: impl Into<String>, provenance: impl Into<String>) -> Self {
        Self {
            text: clip(&text.into(), MAX_ENTRY_CHARS),
            provenance: clip(&provenance.into(), 120),
        }
    }
}

/// Compact state injected into every model request but never transcript history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStateCapsule {
    pub version: u32,
    pub status: TaskStatus,
    pub turn: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<TaskStateEntry>,
    #[serde(default)]
    pub constraints: Vec<TaskStateEntry>,
    #[serde(default)]
    pub completed_steps: Vec<TaskStateEntry>,
    #[serde(default)]
    pub open_steps: Vec<TaskStateEntry>,
    #[serde(default)]
    pub evidence: Vec<TaskStateEntry>,
}

impl Default for TaskStateCapsule {
    fn default() -> Self {
        Self {
            version: 1,
            status: TaskStatus::Idle,
            turn: 0,
            objective: None,
            constraints: Vec::new(),
            completed_steps: Vec::new(),
            open_steps: Vec::new(),
            evidence: Vec::new(),
        }
    }
}

impl TaskStateCapsule {
    pub fn begin_turn(&mut self, user_text: &str) {
        if matches!(
            self.status,
            TaskStatus::Idle | TaskStatus::Completed | TaskStatus::Failed
        ) {
            self.constraints.clear();
            self.completed_steps.clear();
            self.open_steps.clear();
            self.evidence.clear();
        }
        self.turn = self.turn.saturating_add(1);
        self.status = TaskStatus::Active;
        let source = format!("human:turn-{}", self.turn);
        let objective = TaskStateEntry::new(user_text, &source);
        self.objective = Some(objective.clone());
        push_unique(&mut self.open_steps, objective);

        for sentence in user_text.split(['.', '!', '?', '\n']) {
            let lower = sentence.trim().to_ascii_lowercase();
            if sentence.trim().len() >= 4
                && ["must", "only", "don't", "do not", "without", "before"]
                    .iter()
                    .any(|needle| lower.contains(needle))
            {
                push_unique(
                    &mut self.constraints,
                    TaskStateEntry::new(sentence.trim(), &source),
                );
            }
        }
        cap(&mut self.constraints);
        cap(&mut self.completed_steps);
        cap(&mut self.evidence);
    }

    pub fn record_tool_result(&mut self, name: &str, call_id: &str, ok: bool, output: &str) {
        let provenance = format!("tool:{name}:{call_id}");
        let state = if ok { "succeeded" } else { "failed" };
        push_unique(
            &mut self.completed_steps,
            TaskStateEntry::new(format!("{name} {state}"), &provenance),
        );
        if matches!(name, "todo_read" | "todo_write") {
            self.open_steps
                .retain(|step| !step.provenance.starts_with("tool:todo_"));
            for line in output.lines().map(str::trim) {
                if let Some(step) = line.strip_prefix("[ ]") {
                    push_unique(
                        &mut self.open_steps,
                        TaskStateEntry::new(step.trim(), &provenance),
                    );
                } else if let Some(step) = line.strip_prefix("[x]") {
                    push_unique(
                        &mut self.completed_steps,
                        TaskStateEntry::new(step.trim(), &provenance),
                    );
                }
            }
        }
        let useful = if name == "collect_input" {
            ""
        } else {
            output.trim()
        };
        if !useful.is_empty() {
            push_unique(&mut self.evidence, TaskStateEntry::new(useful, provenance));
        }
        cap(&mut self.completed_steps);
        cap(&mut self.evidence);
    }

    pub fn finish(&mut self, status: TaskStatus, assistant_text: &str) {
        self.status = status;
        if !assistant_text.trim().is_empty() {
            push_unique(
                &mut self.completed_steps,
                TaskStateEntry::new(
                    assistant_text.trim(),
                    format!("assistant:turn-{}", self.turn),
                ),
            );
            cap(&mut self.completed_steps);
        }
        if matches!(status, TaskStatus::Completed) {
            let has_open_todos = self
                .open_steps
                .iter()
                .any(|step| step.provenance.starts_with("tool:todo_"));
            if has_open_todos {
                self.status = TaskStatus::Active;
                self.open_steps
                    .retain(|step| step.provenance.starts_with("tool:todo_"));
            } else {
                self.open_steps.clear();
            }
        }
    }

    pub(super) fn rebuild_from_history(&mut self, history: &[Message]) {
        *self = Self::default();
        let Some(last_human) = history
            .iter()
            .rposition(|message| message.origin == MessageOrigin::Human)
        else {
            return;
        };
        let user_text = value_text(&history[last_human].content);
        self.begin_turn(&user_text);
        if let Some(answer) = history[last_human + 1..]
            .iter()
            .rev()
            .find(|message| message.origin == MessageOrigin::Assistant)
        {
            self.finish(TaskStatus::Completed, &value_text(&answer.content));
        }
    }

    pub(super) fn as_message(&self) -> Option<Message> {
        self.objective.as_ref()?;
        let json = serde_json::to_string(self).ok()?;
        Some(Message::with_origin(
            Role::System,
            MessageOrigin::TaskState,
            format!(
                "<task_state>\nStructured host-maintained task state. Preserve constraints and use provenance when relying on evidence.\n{json}\n</task_state>"
            ),
        ))
    }
}

fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| value.to_string())
}

fn push_unique(entries: &mut Vec<TaskStateEntry>, entry: TaskStateEntry) {
    if !entries.iter().any(|current| {
        current.text.eq_ignore_ascii_case(&entry.text) && current.provenance == entry.provenance
    }) {
        entries.push(entry);
    }
}

fn cap(entries: &mut Vec<TaskStateEntry>) {
    if entries.len() > MAX_ENTRIES {
        entries.drain(0..entries.len() - MAX_ENTRIES);
    }
}

fn clip(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let clipped: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{clipped}…")
    } else {
        clipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capsule_tracks_objective_constraint_and_tool_provenance() {
        let mut capsule = TaskStateCapsule::default();
        capsule.begin_turn("Please change only the parser without touching the UI.");
        capsule.record_tool_result("read_file", "call-1", true, "parser is in src/parser.rs");
        assert_eq!(capsule.status, TaskStatus::Active);
        assert_eq!(capsule.constraints.len(), 1);
        assert_eq!(capsule.evidence[0].provenance, "tool:read_file:call-1");
        assert!(capsule.as_message().is_some());
    }

    #[test]
    fn capsule_maps_todos_into_open_and_completed_steps() {
        let mut capsule = TaskStateCapsule::default();
        capsule.begin_turn("ship the change");
        capsule.record_tool_result(
            "todo_write",
            "call-2",
            true,
            "Saved 2 todos.\n[ ] 1 — run tests\n[x] 2 — update code",
        );
        capsule.finish(TaskStatus::Completed, "Code updated; tests remain.");
        assert_eq!(capsule.status, TaskStatus::Active);
        assert_eq!(capsule.open_steps.len(), 1);
        assert!(capsule.open_steps[0].text.contains("run tests"));
        assert!(capsule
            .completed_steps
            .iter()
            .any(|step| step.text.contains("update code")));
    }
}
