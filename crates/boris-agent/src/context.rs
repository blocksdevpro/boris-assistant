use std::fmt;

use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    /// For User/System/Assistant: a plain string or content array.
    /// For Tool: a JSON object `{ tool_call_id, content }`.
    /// For Assistant with tool_calls: the raw message object from the LLM.
    pub content: Value,
}

#[derive(Debug, Default)]
pub struct Context {
    pub messages: Vec<Message>,
    pub max_turns: u32,
}

impl Message {
    pub fn dump(&self) -> Value {
        match self.role {
            // Tool result — must surface tool_call_id at the top level.
            Role::Tool => json!({
                "role": "tool",
                "tool_call_id": self.content["tool_call_id"],
                "content":      self.content["content"],
            }),
            // Assistant with tool_calls — forward the raw object (already has "role").
            Role::Assistant if self.content.get("tool_calls").is_some() => self.content.clone(),
            // Everything else.
            _ => json!({
                "role":    self.role.to_string(),
                "content": self.content,
            }),
        }
    }
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

    /// Prune conversation history by **user turns**, not raw message count.
    ///
    /// Algorithm:
    /// 1. Optionally keep index-0 `Role::System` forever.
    /// 2. Partition remaining messages into turn groups; each group starts at
    ///    a `Role::User` and includes all following assistant/tool traffic
    ///    until the next user message.
    /// 3. Keep only the last `max_turns` groups (oldest turns dropped first).
    /// 4. Rebuild: `[system?] + kept groups`.
    ///
    /// This never splits an assistant `tool_calls` message from its following
    /// `Role::Tool` results, because those always live inside the same turn group.
    fn prune(&mut self) {
        if self.messages.is_empty() {
            return;
        }

        let has_system = matches!(self.messages[0].role, Role::System);
        let body_start = if has_system { 1 } else { 0 };

        // Indices in `messages` where each user-turn group begins.
        let mut turn_starts: Vec<usize> = Vec::new();
        for (i, msg) in self.messages.iter().enumerate().skip(body_start) {
            if matches!(msg.role, Role::User) {
                turn_starts.push(i);
            }
        }

        let max = self.max_turns as usize;

        // max_turns == 0: drop every user turn; keep only the system prompt (if any).
        if max == 0 {
            self.messages.truncate(body_start);
            return;
        }

        // No user turns, or already within budget — nothing to drop.
        if turn_starts.is_empty() || turn_starts.len() <= max {
            return;
        }

        // First message index of the oldest turn we still keep.
        let keep_from = turn_starts[turn_starts.len() - max];

        if has_system {
            // Retain system at index 0, then only the kept turn groups.
            let system = self.messages[0].clone();
            let kept: Vec<Message> = self.messages.drain(keep_from..).collect();
            self.messages.clear();
            self.messages.push(system);
            self.messages.extend(kept);
        } else {
            self.messages.drain(0..keep_from);
        }
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
    fn prune_plain_user_assistant_pairs() {
        let mut ctx = Context::new(2);
        ctx.push(Role::System, "sys");
        ctx.push(Role::User, "u1");
        ctx.push(Role::Assistant, "a1");
        ctx.push(Role::User, "u2");
        ctx.push(Role::Assistant, "a2");
        // Still within budget.
        assert_eq!(
            roles_of(&ctx),
            vec!["system", "user", "assistant", "user", "assistant"]
        );
        assert_eq!(text(&ctx.messages[1]), "u1");

        // Third turn drops the oldest (u1/a1).
        ctx.push(Role::User, "u3");
        ctx.push(Role::Assistant, "a3");
        assert_eq!(
            roles_of(&ctx),
            vec!["system", "user", "assistant", "user", "assistant"]
        );
        assert_eq!(text(&ctx.messages[1]), "u2");
        assert_eq!(text(&ctx.messages[2]), "a2");
        assert_eq!(text(&ctx.messages[3]), "u3");
        assert_eq!(text(&ctx.messages[4]), "a3");
    }

    #[test]
    fn prune_keeps_tool_call_chains_together() {
        let mut ctx = Context::new(1);
        ctx.push(Role::System, "sys");

        // Turn 1: user → assistant(tool_calls) → tool → assistant(final)
        ctx.push(Role::User, "u1");
        ctx.push(
            Role::Assistant,
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{ "id": "call_1", "type": "function" }]
            }),
        );
        ctx.push(
            Role::Tool,
            json!({ "tool_call_id": "call_1", "content": "tool-result-1" }),
        );
        ctx.push(Role::Assistant, "final-1");

        // Turn 2 with its own tool chain — turn 1 must drop as a whole.
        ctx.push(Role::User, "u2");
        ctx.push(
            Role::Assistant,
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{ "id": "call_2", "type": "function" }]
            }),
        );
        ctx.push(
            Role::Tool,
            json!({ "tool_call_id": "call_2", "content": "tool-result-2" }),
        );

        assert_eq!(
            roles_of(&ctx),
            vec!["system", "user", "assistant", "tool"]
        );
        assert_eq!(text(&ctx.messages[1]), "u2");
        // Assistant tool_calls still paired with its tool result.
        assert!(ctx.messages[2].content.get("tool_calls").is_some());
        assert_eq!(
            ctx.messages[3].content["tool_call_id"],
            json!("call_2")
        );
        // No leftover turn-1 tool results (would be orphaned without assistant).
        assert!(
            !ctx.messages
                .iter()
                .any(|m| m.content.get("tool_call_id") == Some(&json!("call_1")))
        );
    }

    #[test]
    fn prune_never_drops_system_prompt() {
        let mut ctx = Context::new(1);
        ctx.push(Role::System, "you are boris");
        for i in 0..5 {
            ctx.push(Role::User, format!("user-{i}"));
            ctx.push(Role::Assistant, format!("asst-{i}"));
        }

        assert!(!ctx.messages.is_empty());
        assert!(matches!(ctx.messages[0].role, Role::System));
        assert_eq!(text(&ctx.messages[0]), "you are boris");
        // Only the latest user turn remains after system.
        assert_eq!(roles_of(&ctx), vec!["system", "user", "assistant"]);
        assert_eq!(text(&ctx.messages[1]), "user-4");
    }

    #[test]
    fn prune_max_turns_zero_keeps_only_system() {
        let mut ctx = Context::new(0);
        ctx.push(Role::System, "sys");
        ctx.push(Role::User, "u1");
        ctx.push(Role::Assistant, "a1");
        assert_eq!(roles_of(&ctx), vec!["system"]);
    }
}
