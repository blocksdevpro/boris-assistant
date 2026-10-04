//! Opt-in, process-local developer capture. Nothing is written to disk.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};

use crate::context::{estimate_serialized_tokens, Context, MessageOrigin};
use crate::types::AgentEvent;

const MAX_EVENTS: usize = 240;
const MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct DebugEvent {
    pub seq: u64,
    pub at_ms: u64,
    pub turn_id: Option<String>,
    pub kind: String,
    pub data: Value,
}

#[derive(Serialize)]
pub struct DebugSnapshot {
    pub enabled: bool,
    pub oldest_seq: u64,
    pub latest_seq: u64,
    pub dropped_events: u64,
    pub events: Vec<DebugEvent>,
}

#[derive(Debug, Default)]
struct CaptureState {
    events: VecDeque<(DebugEvent, usize)>,
    bytes: usize,
    seq: u64,
    dropped_events: u64,
}

#[derive(Debug, Default)]
pub struct DebugCapture {
    enabled: AtomicBool,
    state: Mutex<CaptureState>,
}

impl DebugCapture {
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    pub fn clear(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.events.clear();
            state.bytes = 0;
            state.dropped_events = 0;
        }
    }

    pub fn snapshot(&self, after: u64) -> DebugSnapshot {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        DebugSnapshot {
            enabled: self.enabled(),
            oldest_seq: state
                .events
                .front()
                .map(|(event, _)| event.seq)
                .unwrap_or(state.seq + 1),
            latest_seq: state.seq,
            dropped_events: state.dropped_events,
            events: state
                .events
                .iter()
                .filter(|(event, _)| event.seq > after)
                .map(|(event, _)| event.clone())
                .collect(),
        }
    }

    pub fn record(&self, turn_id: Option<&str>, kind: &str, data: Value) -> Option<u64> {
        if !self.enabled() {
            return None;
        }
        let mut state = self.state.lock().ok()?;
        if !self.enabled() {
            return None;
        }
        state.seq += 1;
        let seq = state.seq;
        let event = DebugEvent {
            seq,
            at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            turn_id: turn_id.map(str::to_owned),
            kind: kind.to_owned(),
            data,
        };
        let bytes = serde_json::to_vec(&event)
            .map(|value| value.len())
            .unwrap_or(0);
        state.bytes = state.bytes.saturating_add(bytes);
        state.events.push_back((event, bytes));
        while state.events.len() > MAX_EVENTS || (state.bytes > MAX_BYTES && state.events.len() > 1)
        {
            if let Some((_, removed)) = state.events.pop_front() {
                state.bytes = state.bytes.saturating_sub(removed);
                state.dropped_events += 1;
            }
        }
        Some(seq)
    }

    pub fn record_request(
        &self,
        turn_id: Option<&str>,
        stage: &str,
        context: &Context,
        tools: &Value,
        estimate: usize,
        context_limit: u32,
        output_reserve: u32,
    ) -> Option<u64> {
        if !self.enabled() {
            return None;
        }
        let mut breakdown = std::collections::BTreeMap::<&str, usize>::new();
        let messages: Vec<Value> = context
            .wire_messages()
            .iter()
            .map(|message| {
                let source = match message.origin {
                    MessageOrigin::System => "system",
                    MessageOrigin::Human => "human",
                    MessageOrigin::Assistant => "assistant",
                    MessageOrigin::Tool => "tool results",
                    MessageOrigin::HostControl => "host controls",
                    MessageOrigin::Summary | MessageOrigin::CompactedTool => "compacted history",
                    MessageOrigin::Skill => "skills",
                    MessageOrigin::PersonalContext => "personal context",
                    MessageOrigin::TaskState => "task state",
                    MessageOrigin::RetrievedMemory => "retrieved memory",
                    MessageOrigin::DerivedContext => "derived context",
                };
                let wire = message.dump();
                let message_tokens = estimate_serialized_tokens(&wire.to_string());
                if message.origin == MessageOrigin::DerivedContext {
                    let mut section_tokens = 0;
                    for (name, body) in context.derived_context_sections() {
                        let tokens = estimate_serialized_tokens(&format!("## {name}\n{body}\n"));
                        *breakdown.entry(name).or_default() += tokens;
                        section_tokens += tokens;
                    }
                    *breakdown.entry("derived framing").or_default() +=
                        message_tokens.saturating_sub(section_tokens);
                } else {
                    *breakdown.entry(source).or_default() += message_tokens;
                }
                json!({ "source": source, "message": wire })
            })
            .collect();
        if !tools.is_null() {
            breakdown.insert(
                "tool schemas",
                estimate_serialized_tokens(&tools.to_string()),
            );
        }
        self.record(
            turn_id,
            "request",
            json!({
                "stage": stage,
                "messages": messages,
                "tools": tools,
                "estimate_tokens": estimate,
                "context_limit": context_limit,
                "output_reserve": output_reserve,
                "breakdown": breakdown,
            }),
        )
    }

    /// Canonical append-only transcript, including messages pruned from the model view.
    pub fn record_history(&self, turn_id: Option<&str>, context: &Context) {
        if !self.enabled() {
            return;
        }
        let messages: Vec<Value> = context
            .history()
            .iter()
            .map(|message| {
                json!({
                    "source": format!("{:?}", message.origin),
                    "message": message.dump(),
                })
            })
            .collect();
        self.record(turn_id, "history", json!({ "messages": messages }));
    }

    pub fn record_agent_event(&self, turn_id: Option<&str>, event: &AgentEvent) {
        if !self.enabled() {
            return;
        }
        let (kind, data) = match event {
            AgentEvent::AgentStart => ("agent_start", json!({})),
            AgentEvent::AgentEnd { outcome } => {
                ("agent_end", json!({ "outcome": format!("{outcome:?}") }))
            }
            AgentEvent::TurnStart { round } => ("round_start", json!({ "round": round })),
            AgentEvent::TurnEnd { round } => ("round_end", json!({ "round": round })),
            AgentEvent::MessageEnd { role, preview } => (
                "message",
                json!({ "role": role.to_string(), "preview": preview }),
            ),
            AgentEvent::ToolNote { text } => ("tool_note", json!({ "text": text })),
            AgentEvent::ToolBatchStart {
                call_ids,
                max_parallel,
            } => (
                "parallel_batch_start",
                json!({ "call_ids": call_ids, "tool_count": call_ids.len(), "max_parallel": max_parallel }),
            ),
            AgentEvent::ToolBatchEnd {
                tool_count,
                duration_ms,
                failed,
                paused,
                cancelled,
            } => (
                "parallel_batch_end",
                json!({ "tool_count": tool_count, "duration_ms": duration_ms, "failed": failed, "paused": paused, "cancelled": cancelled }),
            ),
            AgentEvent::ReportFallback { meta, title, .. } => (
                "report_fallback",
                json!({
                    "title": title, "saved": meta.is_some(), "meta": meta,
                }),
            ),
            AgentEvent::ToolExecutionStart {
                call_id,
                tool_name,
                args_summary,
            } => (
                "tool_start",
                json!({ "call_id": call_id, "tool_name": tool_name, "args_summary": args_summary }),
            ),
            AgentEvent::ToolExecutionEnd {
                call_id,
                tool_name,
                ok,
                duration_ms,
            } => (
                "tool_end",
                json!({ "call_id": call_id, "tool_name": tool_name, "ok": ok, "duration_ms": duration_ms }),
            ),
            AgentEvent::ToolProgress { .. } | AgentEvent::Reasoning { .. } => return,
            AgentEvent::NeedsConfirmation { pending } => {
                ("confirmation", json!({ "tool": pending.name }))
            }
            AgentEvent::NeedsInput { pending } => ("input", json!({ "tool": pending.name })),
            AgentEvent::Error { message } => ("error", json!({ "message": message })),
        };
        self.record(turn_id, kind, data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Role;

    #[test]
    fn opt_in_cursor_and_clear() {
        let capture = DebugCapture::default();
        assert!(capture
            .record(Some("turn"), "test", json!({"value": 1}))
            .is_none());
        capture.set_enabled(true);
        let first = capture
            .record(Some("turn"), "test", json!({"value": 1}))
            .unwrap();
        capture.record(Some("turn"), "test", json!({"value": 2}));
        assert_eq!(capture.snapshot(first).events.len(), 1);
        capture.clear();
        assert!(capture.snapshot(0).events.is_empty());
        assert!(capture.snapshot(0).latest_seq >= first);
    }

    #[test]
    fn request_capture_keeps_full_payload_and_source_breakdown() {
        let capture = DebugCapture::default();
        capture.set_enabled(true);
        let mut context = Context::new(20);
        context.push(Role::System, "system policy");
        context.push(Role::User, "question");
        context.push(Role::Assistant, json!({"content": "", "tool_calls": [{"id": "1", "function": {"name": "lookup", "arguments": "{}"}}]}));
        context.push(
            Role::Tool,
            json!({"tool_call_id": "1", "content": "full tool output"}),
        );
        let tools = json!([{"type": "function", "function": {"name": "lookup"}}]);
        capture.record_request(
            Some("turn-1"),
            "ToolPlanning",
            &context,
            &tools,
            context.estimate_request_tokens(&tools),
            128_000,
            4_096,
        );
        let event = capture.snapshot(0).events.pop().unwrap();
        assert_eq!(
            event.data["messages"][3]["message"]["content"],
            "full tool output"
        );
        assert!(event.data["breakdown"]["tool results"].as_u64().unwrap() > 0);
        assert!(event.data["breakdown"]["tool schemas"].as_u64().unwrap() > 0);
        assert_eq!(event.data["tools"], tools);
    }

    #[test]
    fn old_events_are_bounded_and_reported() {
        let capture = DebugCapture::default();
        capture.set_enabled(true);
        for index in 0..=MAX_EVENTS {
            capture.record(None, "test", json!({"index": index}));
        }
        let snapshot = capture.snapshot(0);
        assert_eq!(snapshot.events.len(), MAX_EVENTS);
        assert_eq!(snapshot.dropped_events, 1);
        assert_eq!(snapshot.oldest_seq, 2);
    }
}
