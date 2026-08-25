//! Working conversation memory for the agent loop.
//!
//! Keeps canonical append-only [`Message`] history separate from the derived
//! model view, which may be pruned, compacted, and enriched with task/memory
//! capsules to stay within token budgets.
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`role`] | [`Role`] wire strings / parsing |
//! | [`message`] | [`Message`] + OpenAI/OpenRouter dump |
//! | [`turns`] | user-turn partitioning + prune |
//! | [`compact`] | mechanical / summary compaction helpers |
//!
//! # Contributor notes
//!
//! - **Public surface**: [`Context`], [`Message`], [`MessageOrigin`], [`Role`],
//!   and [`ContextBudget`] (re-exported from crate root).
//! - Prune never splits assistant `tool_calls` from following tool results.
//! - Compact must emit plain-string assistant content (never nested message objects).
//! - Prefer pure helpers in submodules with unit tests over growing `Context` methods.

mod compact;
mod message;
mod retrieval;
mod role;
mod task_state;
mod turns;

use serde_json::{json, Value};

pub use compact::ContextBudget;
pub use message::{Message, MessageOrigin};
pub use retrieval::RetrievedMemory;
pub use role::Role;
pub use task_state::{TaskStateCapsule, TaskStateEntry, TaskStatus};

#[derive(Debug, Clone, Default)]
pub struct Context {
    /// Mutable, budgeted model view. Compaction is allowed to rewrite this.
    messages: Vec<Message>,
    /// Canonical transcript. Only original system/human/assistant/tool events
    /// enter this vector, and compaction never mutates it.
    history: Vec<Message>,
    task_state: TaskStateCapsule,
    retrieved_memory: Vec<RetrievedMemory>,
    pub max_turns: u32,
}

impl Context {
    pub fn new(max_turns: u32) -> Self {
        Self {
            messages: Vec::new(),
            history: Vec::new(),
            task_state: TaskStateCapsule::default(),
            retrieved_memory: Vec::new(),
            max_turns,
        }
    }

    pub fn push(&mut self, role: Role, content: impl Into<Value>) {
        let message = Message::new(role, content);
        if message.origin.is_history_event() {
            self.history.push(message.clone());
        }
        self.messages.push(message);
        self.prune();
    }

    /// Add an ephemeral harness instruction without creating a human turn.
    pub fn push_control(&mut self, content: impl Into<Value>) {
        self.messages.push(Message::with_origin(
            Role::User,
            MessageOrigin::HostControl,
            content,
        ));
        self.prune();
    }

    /// Replace or insert the leading system message (used when personal context refreshes).
    pub fn set_system(&mut self, content: impl Into<Value>) {
        let content = content.into();
        if self
            .messages
            .first()
            .is_some_and(|m| matches!(m.role, Role::System))
        {
            self.messages[0].content = content;
        } else {
            let message = Message {
                role: Role::System,
                origin: MessageOrigin::System,
                content,
            };
            self.messages.insert(0, message.clone());
            if !self
                .history
                .iter()
                .any(|item| matches!(item.role, Role::System))
            {
                self.history.insert(0, message);
            }
        }
    }

    /// Start a new derived task capsule from the current human request.
    pub fn begin_task(&mut self, user_text: &str) {
        self.task_state.begin_turn(user_text);
        self.retrieved_memory.clear();
    }

    pub fn record_tool_result(&mut self, name: &str, call_id: &str, ok: bool, output: &str) {
        self.task_state
            .record_tool_result(name, call_id, ok, output);
    }

    pub fn finish_task(&mut self, status: TaskStatus, assistant_text: &str) {
        self.task_state.finish(status, assistant_text);
    }

    pub fn task_state(&self) -> &TaskStateCapsule {
        &self.task_state
    }

    pub fn set_task_state(&mut self, capsule: TaskStateCapsule) {
        self.task_state = capsule;
    }

    pub fn set_retrieved_memory(&mut self, memories: Vec<RetrievedMemory>) {
        self.retrieved_memory = memories;
    }

    pub fn retrieved_memory(&self) -> &[RetrievedMemory] {
        &self.retrieved_memory
    }

    /// Replace non-system messages with history loaded from a session.
    ///
    /// Always installs `system_prompt` as the first message (current prompt wins
    /// over any system rows that may appear in `history`). Remaining history
    /// messages that are not `Role::System` are appended, then pruned once.
    pub fn load_history(&mut self, system_prompt: &str, history: Vec<Message>) {
        self.messages.clear();
        let current_system = Message {
            role: Role::System,
            origin: MessageOrigin::System,
            content: Value::String(system_prompt.to_string()),
        };
        self.messages.push(current_system.clone());
        self.history = history
            .into_iter()
            .filter(|message| message.origin.is_history_event())
            .collect();
        if !self
            .history
            .iter()
            .any(|message| matches!(message.role, Role::System))
        {
            self.history.insert(0, current_system);
        }
        for msg in &self.history {
            if !matches!(msg.role, Role::System) {
                self.messages.push(msg.clone());
            }
        }
        self.task_state.rebuild_from_history(&self.history);
        self.retrieved_memory.clear();
        self.prune();
    }

    /// Current derived model messages (borrowed) for request inspection.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Canonical append-only transcript for persistence and audit.
    pub fn history(&self) -> &[Message] {
        &self.history
    }

    /// Replace both canonical history and its freshly-derived model view.
    pub fn replace_history(&mut self, messages: Vec<Message>) {
        let system_prompt = messages
            .iter()
            .find(|message| matches!(message.role, Role::System))
            .and_then(|message| message.content.as_str())
            .or_else(|| {
                self.messages
                    .iter()
                    .find(|message| matches!(message.role, Role::System))
                    .and_then(|message| message.content.as_str())
            })
            .unwrap_or_default()
            .to_string();
        self.load_history(&system_prompt, messages);
    }

    /// Clear all canonical and derived state and install a fresh system prompt.
    pub fn reset(&mut self, system_prompt: impl Into<Value>) {
        self.messages.clear();
        self.history.clear();
        self.task_state = TaskStateCapsule::default();
        self.retrieved_memory.clear();
        self.push(Role::System, system_prompt);
    }

    /// Build [`Message`] list from transcript role strings + content values.
    ///
    /// Skips unknown roles. Prefer this over importing session transcript types
    /// so `context` stays free of a dependency cycle with `session`.
    pub fn messages_from_transcript(records: &[(String, Value)]) -> Vec<Message> {
        records
            .iter()
            .filter_map(|(role, content)| {
                let role = Role::from_role_str(role)?;
                let origin = infer_transcript_origin(role.clone(), content);
                Some(Message {
                    role,
                    origin,
                    content: content.clone(),
                })
            })
            .collect()
    }

    pub fn as_json(&self) -> Value {
        let messages: Vec<Value> = self.wire_messages().iter().map(|m| m.dump()).collect();
        json!(messages)
    }

    pub(super) fn wire_messages(&self) -> Vec<Message> {
        let mut messages = self.messages.clone();
        let insert_at = usize::from(
            messages
                .first()
                .is_some_and(|message| matches!(message.role, Role::System)),
        );
        let mut derived = Vec::new();
        if let Some(memory) = retrieval::as_message(&self.retrieved_memory) {
            derived.push(memory);
        }
        if let Some(task_state) = self.task_state.as_message() {
            derived.push(task_state);
        }
        messages.splice(insert_at..insert_at, derived);
        messages
    }
}

fn infer_transcript_origin(role: Role, content: &Value) -> MessageOrigin {
    let text = content.as_str().unwrap_or_default().trim_start();
    if text.starts_with("<conversation_summary>") {
        return MessageOrigin::Summary;
    }
    if text.starts_with("<system-reminder>") {
        return MessageOrigin::HostControl;
    }
    MessageOrigin::for_role(role)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn roles_of(ctx: &Context) -> Vec<&'static str> {
        ctx.messages
            .iter()
            .map(|m| match m.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            })
            .collect()
    }

    fn text(m: &Message) -> String {
        match &m.content {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }

    #[test]
    fn load_history_forces_system_and_skips_history_system() {
        let mut ctx = Context::new(20);
        ctx.push(Role::System, "old-sys");
        ctx.push(Role::User, "stale");

        let history = vec![
            Message {
                role: Role::System,
                origin: MessageOrigin::System,
                content: json!("history-sys"),
            },
            Message {
                role: Role::User,
                origin: MessageOrigin::Human,
                content: json!("hello"),
            },
            Message {
                role: Role::Assistant,
                origin: MessageOrigin::Assistant,
                content: json!("hi"),
            },
        ];
        ctx.load_history("fresh-sys", history);

        assert_eq!(roles_of(&ctx), vec!["system", "user", "assistant"]);
        assert_eq!(text(&ctx.messages[0]), "fresh-sys");
        assert_eq!(text(&ctx.messages[1]), "hello");
        assert_eq!(text(&ctx.messages[2]), "hi");
    }

    #[test]
    fn messages_from_transcript_skips_unknown_roles() {
        let records = vec![
            ("user".into(), json!("u")),
            ("bogus".into(), json!("x")),
            ("assistant".into(), json!("a")),
            (
                "tool".into(),
                json!({ "tool_call_id": "c1", "content": "ok" }),
            ),
        ];
        let msgs = Context::messages_from_transcript(&records);
        assert_eq!(msgs.len(), 3);
        assert!(matches!(msgs[0].role, Role::User));
        assert!(matches!(msgs[1].role, Role::Assistant));
        assert!(matches!(msgs[2].role, Role::Tool));
    }

    #[test]
    fn transcript_markers_restore_non_human_origins() {
        let records = vec![
            (
                "user".into(),
                json!("<conversation_summary>facts</conversation_summary>"),
            ),
            (
                "user".into(),
                json!("<system-reminder>finish</system-reminder>"),
            ),
            ("user".into(), json!("real question")),
        ];
        let messages = Context::messages_from_transcript(&records);
        assert_eq!(messages[0].origin, MessageOrigin::Summary);
        assert_eq!(messages[1].origin, MessageOrigin::HostControl);
        assert_eq!(messages[2].origin, MessageOrigin::Human);
    }

    #[test]
    fn set_system_replaces_existing() {
        let mut ctx = Context::new(20);
        ctx.push(Role::System, "old");
        ctx.push(Role::User, "u");
        ctx.set_system("new");
        assert_eq!(text(&ctx.messages[0]), "new");
        assert_eq!(ctx.messages.len(), 2);
    }

    #[test]
    fn set_system_inserts_when_missing() {
        let mut ctx = Context::new(20);
        ctx.push(Role::User, "u");
        ctx.set_system("sys");
        assert!(matches!(ctx.messages[0].role, Role::System));
        assert_eq!(text(&ctx.messages[0]), "sys");
        assert_eq!(text(&ctx.messages[1]), "u");
    }

    #[test]
    fn as_json_is_message_array() {
        let mut ctx = Context::new(20);
        ctx.push(Role::User, "hello");
        let v = ctx.as_json();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["role"], "user");
        assert_eq!(arr[0]["content"], "hello");
    }

    #[test]
    fn pruning_and_summary_never_rewrite_append_only_history() {
        let mut ctx = Context::new(1);
        ctx.push(Role::System, "sys");
        ctx.push(Role::User, "u1");
        ctx.push(Role::Assistant, "a1");
        ctx.push(Role::User, "u2");
        ctx.push(Role::Assistant, "a2");

        assert_eq!(ctx.messages().len(), 3, "derived view is pruned");
        assert_eq!(ctx.history().len(), 5, "canonical events remain complete");
        assert!(ctx
            .history()
            .iter()
            .any(|message| message.content == json!("u1")));

        let mut summarized = Context::new(20);
        summarized.push(Role::System, "sys");
        for turn in 1..=4 {
            summarized.push(Role::User, format!("u{turn}"));
            summarized.push(Role::Assistant, format!("a{turn}"));
        }
        summarized.apply_summary_compact("older work", 1);
        assert!(summarized
            .messages()
            .iter()
            .any(|message| message.origin == MessageOrigin::Summary));
        assert_eq!(summarized.history().len(), 9);
        assert!(!summarized
            .history()
            .iter()
            .any(|message| message.origin == MessageOrigin::Summary));
    }

    #[test]
    fn task_and_retrieval_are_derived_and_provenance_carrying() {
        let mut ctx = Context::new(20);
        ctx.push(Role::System, "sys");
        ctx.push(Role::User, "fix only the parser");
        ctx.begin_task("fix only the parser");
        ctx.set_retrieved_memory(vec![RetrievedMemory {
            snippet: "Parser lives in parser.rs".into(),
            path: "session/abc/memory.md".into(),
            source: "session".into(),
            score: 42,
        }]);

        let wire = ctx.as_json().to_string();
        assert!(wire.contains("<task_state>"));
        assert!(wire.contains("<retrieved_memory>"));
        assert!(wire.contains("session/abc/memory.md"));
        assert_eq!(ctx.history().len(), 2);
    }
}
