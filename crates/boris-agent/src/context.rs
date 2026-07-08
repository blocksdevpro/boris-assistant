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

    fn prune(&mut self) {
        // Each turn = 3 messages (User + Assistant tool_call + Tool result)
        // +1 for the System prompt at index 0
        let max_messages = 1 + (self.max_turns * 3);

        if self.messages.len() > max_messages as usize {
            self.messages
                .drain(1..self.messages.len() - max_messages as usize);
        }
    }

    pub fn as_json(&self) -> Value {
        let messages: Vec<Value> = self.messages.iter().map(|m| m.dump()).collect();
        json!(messages)
    }
}
