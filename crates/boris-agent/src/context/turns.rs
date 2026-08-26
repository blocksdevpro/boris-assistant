//! Human-turn partitioning and history pruning.
//!
//! Prune is by real **human turns**, not user-role rows or raw message count, so assistant `tool_calls`
//! stay paired with their following `Role::Tool` results.

use super::{Context, Message, MessageOrigin, Role};

/// Index of the first non-system message (0 or 1).
pub(super) fn body_start(messages: &[Message]) -> usize {
    if messages
        .first()
        .is_some_and(|m| matches!(m.role, Role::System))
    {
        1
    } else {
        0
    }
}

/// Message indices where each real human-turn group begins.
pub(super) fn user_turn_starts(messages: &[Message], body_start: usize) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .skip(body_start)
        .filter_map(|(i, msg)| msg.origin.is_human().then_some(i))
        .collect()
}

impl Context {
    /// Prune conversation history by real **human turns**, not raw message count.
    ///
    /// Algorithm:
    /// 1. Optionally keep index-0 `Role::System` forever.
    /// 2. Partition remaining messages into turn groups; each group starts at
    ///    a `MessageOrigin::Human` and includes all following control,
    ///    assistant, and tool traffic until the next human message.
    /// 3. Keep only the last `max_turns` groups (oldest turns dropped first).
    /// 4. Rebuild: `[system?] + kept groups`.
    ///
    /// This never splits an assistant `tool_calls` message from its following
    /// `Role::Tool` results, because those always live inside the same turn group.
    pub(super) fn prune(&mut self) {
        if self.messages.is_empty() {
            return;
        }

        let body = body_start(&self.messages);
        let turn_starts = user_turn_starts(&self.messages, body);
        let max = self.max_turns as usize;
        let prefix_end = turn_starts.first().copied().unwrap_or(self.messages.len());

        // max_turns == 0: drop every human turn; preserve system/summary prefix.
        if max == 0 {
            let metadata_prefix_end = self
                .messages
                .iter()
                .position(|m| {
                    matches!(
                        m.origin,
                        MessageOrigin::Human | MessageOrigin::Assistant | MessageOrigin::Tool
                    )
                })
                .unwrap_or(self.messages.len());
            self.messages.truncate(metadata_prefix_end);
            return;
        }

        // No user turns, or already within budget — nothing to drop.
        if turn_starts.is_empty() || turn_starts.len() <= max {
            return;
        }

        // First message index of the oldest turn we still keep.
        let keep_from = turn_starts[turn_starts.len() - max];

        let prefix = self.messages[..prefix_end].to_vec();
        let kept = self.messages[keep_from..].to_vec();
        self.messages.clear();
        self.messages.extend(prefix);
        self.messages.extend(kept);
    }

    /// Count real human turns (excludes summary and host-control user-role rows).
    pub fn user_turn_count(&self) -> usize {
        self.messages.iter().filter(|m| m.origin.is_human()).count()
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
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }

    #[test]
    fn body_start_detects_system() {
        assert_eq!(body_start(&[]), 0);
        assert_eq!(
            body_start(&[Message {
                role: Role::User,
                origin: MessageOrigin::Human,
                content: json!("u"),
            }]),
            0
        );
        assert_eq!(
            body_start(&[Message {
                role: Role::System,
                origin: MessageOrigin::System,
                content: json!("s"),
            }]),
            1
        );
    }

    #[test]
    fn user_turn_starts_lists_user_indices() {
        let msgs = vec![
            Message {
                role: Role::System,
                origin: MessageOrigin::System,
                content: json!("s"),
            },
            Message {
                role: Role::User,
                origin: MessageOrigin::Human,
                content: json!("u1"),
            },
            Message {
                role: Role::Assistant,
                origin: MessageOrigin::Assistant,
                content: json!("a1"),
            },
            Message {
                role: Role::User,
                origin: MessageOrigin::Human,
                content: json!("u2"),
            },
        ];
        assert_eq!(user_turn_starts(&msgs, 1), vec![1, 3]);
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

        assert_eq!(roles_of(&ctx), vec!["system", "user", "assistant", "tool"]);
        assert_eq!(text(&ctx.messages[1]), "u2");
        // Assistant tool_calls still paired with its tool result.
        assert!(ctx.messages[2].content.get("tool_calls").is_some());
        assert_eq!(ctx.messages[3].content["tool_call_id"], json!("call_2"));
        // No leftover turn-1 tool results (would be orphaned without assistant).
        assert!(!ctx
            .messages
            .iter()
            .any(|m| m.content.get("tool_call_id") == Some(&json!("call_1"))));
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

    #[test]
    fn prune_without_system_drops_oldest_turns() {
        let mut ctx = Context::new(1);
        ctx.push(Role::User, "u1");
        ctx.push(Role::Assistant, "a1");
        ctx.push(Role::User, "u2");
        ctx.push(Role::Assistant, "a2");
        assert_eq!(roles_of(&ctx), vec!["user", "assistant"]);
        assert_eq!(text(&ctx.messages[0]), "u2");
    }

    #[test]
    fn user_turn_count_ignores_system() {
        let mut ctx = Context::new(20);
        ctx.push(Role::System, "sys");
        ctx.push(Role::User, "u1");
        ctx.push(Role::Assistant, "a1");
        ctx.push(Role::User, "u2");
        assert_eq!(ctx.user_turn_count(), 2);
    }

    #[test]
    fn host_controls_do_not_create_or_displace_human_turns() {
        let mut ctx = Context::new(1);
        ctx.push(Role::System, "sys");
        ctx.push(Role::User, "u1");
        ctx.push(Role::Assistant, "a1");
        ctx.push_control("<system-reminder>continue</system-reminder>");
        assert_eq!(ctx.user_turn_count(), 1);

        ctx.push(Role::User, "u2");
        ctx.push_control("<system-reminder>finish</system-reminder>");
        assert_eq!(ctx.user_turn_count(), 1);
        assert!(ctx.messages.iter().any(|m| m.content == json!("u2")));
        assert!(!ctx.messages.iter().any(|m| m.content == json!("u1")));
        assert!(ctx
            .messages
            .iter()
            .any(|m| m.origin == MessageOrigin::HostControl));
    }

    #[test]
    fn summary_prefix_is_preserved_but_not_counted_as_human() {
        let mut ctx = Context::new(1);
        ctx.push(Role::System, "sys");
        ctx.messages.push(Message::with_origin(
            Role::User,
            MessageOrigin::Summary,
            "<conversation_summary>prior facts</conversation_summary>",
        ));
        ctx.push(Role::User, "u1");
        ctx.push(Role::Assistant, "a1");
        ctx.push(Role::User, "u2");

        assert_eq!(ctx.user_turn_count(), 1);
        assert_eq!(ctx.messages[1].origin, MessageOrigin::Summary);
        assert!(ctx.messages.iter().any(|m| m.content == json!("u2")));
    }
}
