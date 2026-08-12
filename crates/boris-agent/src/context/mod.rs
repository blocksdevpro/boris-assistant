//! Working conversation memory for the agent loop.
//!
//! Holds [`Message`]s, prunes by user-turn count, and applies mechanical /
//! summary compaction so the wire payload stays within token budgets.
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
//! - **Public surface**: [`Context`], [`Message`], [`Role`] only (re-exported from crate root).
//! - Prune never splits assistant `tool_calls` from following tool results.
//! - Compact must emit plain-string assistant content (never nested message objects).
//! - Prefer pure helpers in submodules with unit tests over growing `Context` methods.

mod compact;
mod message;
mod role;
mod turns;

use serde_json::{json, Value};

pub use message::Message;
pub use role::Role;

#[derive(Debug, Default)]
pub struct Context {
    pub messages: Vec<Message>,
    pub max_turns: u32,
}

impl Context {
    pub fn new(max_turns: u32) -> Self {
        Self {
            messages: Vec::new(),
            max_turns,
        }
    }

    pub fn push(&mut self, role: Role, content: impl Into<Value>) {
        self.messages.push(Message {
            role,
            content: content.into(),
        });
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
            self.messages.insert(
                0,
                Message {
                    role: Role::System,
                    content,
                },
            );
        }
    }

    /// Replace non-system messages with history loaded from a session.
    ///
    /// Always installs `system_prompt` as the first message (current prompt wins
    /// over any system rows that may appear in `history`). Remaining history
    /// messages that are not `Role::System` are appended, then pruned once.
    pub fn load_history(&mut self, system_prompt: &str, history: Vec<Message>) {
        self.messages.clear();
        self.messages.push(Message {
            role: Role::System,
            content: Value::String(system_prompt.to_string()),
        });
        for msg in history {
            if !matches!(msg.role, Role::System) {
                self.messages.push(msg);
            }
        }
        self.prune();
    }

    /// All messages (borrowed) for persistence / debug.
    pub fn messages(&self) -> &[Message] {
        &self.messages
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
                Some(Message {
                    role,
                    content: content.clone(),
                })
            })
            .collect()
    }

    pub fn as_json(&self) -> Value {
        let messages: Vec<Value> = self.messages.iter().map(|m| m.dump()).collect();
        json!(messages)
    }
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
                content: json!("history-sys"),
            },
            Message {
                role: Role::User,
                content: json!("hello"),
            },
            Message {
                role: Role::Assistant,
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
}
