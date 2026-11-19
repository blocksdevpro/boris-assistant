//! Keep executed work when a turn fails or is interrupted.
//!
//! Tool side effects (files written, notes saved, shell commands) stay done
//! when a turn errors or the user barges in. Rolling the context back to the
//! pre-turn snapshot made the model forget them, so "continue" redid or
//! contradicted finished work. Instead, a turn in which any tool executed
//! keeps its human message, assistant tool calls, and tool results; unrun
//! calls are closed with a synthetic failed result so the provider sequence
//! stays valid; and a short persisted host note records how the turn ended.
//! A turn in which nothing executed is still rolled back by the caller.

use serde_json::{json, Value};

use super::{Context, Message, Role, TaskStatus};

/// Observation for a tool call that never ran because its turn ended first.
pub(crate) const NOT_RUN_INTERRUPTED: &str = "Error: not run: turn interrupted";
/// Observation [`Context::resolve_pending_tool_calls_as_cancelled`] writes.
const CANCELLED_ON_ABORT: &str = "Error: tool call cancelled because the turn was aborted";
/// Prefix of the persisted host note appended after salvaged work.
pub(crate) const TURN_NOTE_PREFIX: &str = "[Host note:";

/// How a salvaged turn ended (wording of the host note).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEnd {
    /// Cancelled: barge-in, stop, or replacement.
    Interrupted,
    /// Provider, transport, or loop error.
    Failed,
}

impl Context {
    /// Index of the latest human message in canonical history (0 if none).
    pub(crate) fn current_turn_history_start(&self) -> usize {
        self.history
            .iter()
            .rposition(|message| message.origin.is_human())
            .unwrap_or(0)
    }

    /// Keep the executed part of a turn that failed or was interrupted.
    ///
    /// `turn_history_start` is the canonical history length before the turn
    /// began (or the index of its human message). Returns `false` when no
    /// tool executed in that range; the caller should then roll back. Calling
    /// it again on an already salvaged turn is a no-op that returns `true`.
    ///
    /// On success: host controls are cleared, an undelivered final reply
    /// (plain assistant text after the last tool result) is dropped, unrun
    /// calls of the latest batch get [`NOT_RUN_INTERRUPTED`] results, and an
    /// assistant-role host note naming the executed tools is appended.
    pub(crate) fn salvage_interrupted_turn(
        &mut self,
        turn_history_start: usize,
        end: TurnEnd,
    ) -> bool {
        if turn_history_start > self.history.len() {
            return false;
        }
        let turn = &self.history[turn_history_start..];
        if turn.last().is_some_and(is_turn_note) {
            return true;
        }
        let executed = executed_tool_names(turn);
        if executed.is_empty() {
            return false;
        }

        self.clear_host_controls();
        drop_trailing_replies(&mut self.history, turn_history_start);
        let messages_floor = self
            .messages
            .iter()
            .rposition(|message| matches!(message.role, Role::Tool))
            .map_or(self.messages.len(), |index| index + 1);
        drop_trailing_replies(&mut self.messages, messages_floor);
        self.close_unresolved_calls(turn_history_start);

        let note = match end {
            TurnEnd::Interrupted => format!(
                "{TURN_NOTE_PREFIX} the user interrupted this turn after these tools ran: {}. \
                 Their effects already happened; no final reply was given.]",
                executed.join(", ")
            ),
            TurnEnd::Failed => format!(
                "{TURN_NOTE_PREFIX} this turn failed after these tools ran: {}. \
                 Their effects already happened; no final reply was given.]",
                executed.join(", ")
            ),
        };
        self.push(Role::Assistant, note);
        self.finish_task(TaskStatus::Failed, "");
        true
    }

    /// Give every unresolved call of the turn's latest batch a failed result.
    fn close_unresolved_calls(&mut self, turn_history_start: usize) {
        let turn = &self.history[turn_history_start..];
        let Some(batch) = turn.iter().rposition(has_tool_calls) else {
            return;
        };
        let resolved = turn[batch + 1..]
            .iter()
            .filter(|message| matches!(message.role, Role::Tool))
            .filter_map(|message| message.content.get("tool_call_id").and_then(Value::as_str))
            .map(ToOwned::to_owned)
            .collect::<std::collections::HashSet<_>>();
        let unresolved = tool_calls(&turn[batch])
            .filter_map(|call| call.get("id").and_then(Value::as_str))
            .filter(|id| !id.is_empty() && !resolved.contains(*id))
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        for call_id in unresolved {
            self.push(
                Role::Tool,
                json!({"tool_call_id": call_id, "content": NOT_RUN_INTERRUPTED, "_tool_ok": false}),
            );
        }
    }
}

fn is_turn_note(message: &Message) -> bool {
    matches!(message.role, Role::Assistant)
        && message
            .content
            .as_str()
            .is_some_and(|text| text.starts_with(TURN_NOTE_PREFIX))
}

fn has_tool_calls(message: &Message) -> bool {
    matches!(message.role, Role::Assistant) && tool_calls(message).next().is_some()
}

fn tool_calls(message: &Message) -> impl Iterator<Item = &Value> {
    message
        .content
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// Names of tools whose results in `turn` are real executions (not synthetic
/// not-run rows), deduplicated in first-seen order.
fn executed_tool_names(turn: &[Message]) -> Vec<String> {
    let names = turn
        .iter()
        .filter(|message| matches!(message.role, Role::Assistant))
        .flat_map(tool_calls)
        .filter_map(|call| {
            Some((
                call.get("id")?.as_str()?.to_string(),
                call.get("function")?.get("name")?.as_str()?.to_string(),
            ))
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut executed = Vec::new();
    for message in turn.iter().filter(|m| matches!(m.role, Role::Tool)) {
        let content = message
            .content
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if content.starts_with(NOT_RUN_INTERRUPTED) || content.starts_with(CANCELLED_ON_ABORT) {
            continue;
        }
        let name = message
            .content
            .get("tool_call_id")
            .and_then(Value::as_str)
            .and_then(|id| names.get(id))
            .cloned()
            .unwrap_or_else(|| "tool".to_string());
        if !executed.contains(&name) {
            executed.push(name);
        }
    }
    executed
}

/// Pop plain assistant replies (no tool calls) from the end, never below `floor`.
fn drop_trailing_replies(messages: &mut Vec<Message>, floor: usize) {
    while messages.len() > floor
        && messages.last().is_some_and(|message| {
            matches!(message.role, Role::Assistant) && !has_tool_calls(message)
        })
    {
        messages.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, name: &str) -> Value {
        json!({"id": id, "type": "function", "function": {"name": name, "arguments": "{}"}})
    }

    fn turn_with_batch(ctx: &mut Context, calls: Vec<Value>) -> usize {
        ctx.push(Role::System, "sys");
        ctx.push(Role::User, "earlier");
        ctx.push(Role::Assistant, "earlier answer");
        let start = ctx.history().len();
        ctx.push(Role::User, "write and run");
        ctx.push(
            Role::Assistant,
            json!({"role": "assistant", "content": "", "tool_calls": calls}),
        );
        start
    }

    /// Provider view of stored messages (without the derived-context block).
    fn wire(ctx: &Context) -> Vec<Value> {
        ctx.messages().iter().map(Message::dump).collect()
    }

    #[test]
    fn executed_work_is_kept_and_unrun_calls_are_closed() {
        let mut ctx = Context::new(u32::MAX);
        let start = turn_with_batch(
            &mut ctx,
            vec![call("c1", "file_write"), call("c2", "bash")],
        );
        ctx.push_tool_result("file_write", "c1", "wrote notes.txt".into(), true);
        ctx.push_control("<system-reminder>scoped</system-reminder>");

        assert!(ctx.salvage_interrupted_turn(start, TurnEnd::Interrupted));

        let wire = wire(&ctx);
        let roles: Vec<&str> = wire.iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(
            roles,
            ["system", "user", "assistant", "user", "assistant", "tool", "tool", "assistant"]
        );
        assert_eq!(wire[5]["content"], "wrote notes.txt");
        assert_eq!(wire[6]["tool_call_id"], "c2");
        assert_eq!(wire[6]["content"], NOT_RUN_INTERRUPTED);
        let note = wire[7]["content"].as_str().unwrap();
        assert!(note.starts_with(TURN_NOTE_PREFIX) && note.contains("file_write"));
        assert!(!note.contains("bash"), "unrun calls are not reported as executed");
        // Persisted: the note and the closing row are canonical history.
        assert!(ctx.history().last().unwrap().content.as_str() == Some(note));
        assert!(ctx
            .history()
            .iter()
            .any(|m| m.content.get("content").and_then(Value::as_str) == Some(NOT_RUN_INTERRUPTED)));
        assert_eq!(ctx.task_state().status, TaskStatus::Failed);
        // Idempotent: a second salvage (e.g. host abandon after the agent's
        // own error handling) adds nothing.
        let len = ctx.history().len();
        assert!(ctx.salvage_interrupted_turn(start, TurnEnd::Interrupted));
        assert_eq!(ctx.history().len(), len);
    }

    #[test]
    fn undelivered_final_reply_is_dropped() {
        let mut ctx = Context::new(u32::MAX);
        let start = turn_with_batch(&mut ctx, vec![call("c1", "remember_note")]);
        ctx.push_tool_result("remember_note", "c1", "saved".into(), true);
        ctx.push(Role::Assistant, "Saved it for you.");

        assert!(ctx.salvage_interrupted_turn(start, TurnEnd::Failed));
        let texts: Vec<String> = ctx
            .history()
            .iter()
            .filter_map(|m| m.content.as_str().map(str::to_owned))
            .collect();
        assert!(!texts.iter().any(|t| t == "Saved it for you."));
        assert!(texts.last().unwrap().contains("failed after these tools ran: remember_note"));
        assert!(!wire(&ctx)
            .iter()
            .any(|m| m["content"] == "Saved it for you."));
    }

    #[test]
    fn nothing_executed_asks_for_rollback() {
        let mut ctx = Context::new(u32::MAX);
        let start = turn_with_batch(&mut ctx, vec![call("c1", "bash")]);
        assert!(!ctx.salvage_interrupted_turn(start, TurnEnd::Interrupted));
        ctx.resolve_pending_tool_calls_as_cancelled();
        assert!(
            !ctx.salvage_interrupted_turn(start, TurnEnd::Interrupted),
            "synthetic cancellation rows are not executed work"
        );
        assert!(!ctx.salvage_interrupted_turn(ctx.history().len() + 5, TurnEnd::Failed));
    }
}
