//! Append-only session chat history (Grok-style `chat_history.jsonl`).
//!
//! ## Wire format
//!
//! ```json
//! {"type":"system","content":"...","ts":"2026-08-07T12:00:00.000Z"}
//! {"type":"user","content":[{"type":"text","text":"hello"}],"ts":"..."}
//! {"type":"assistant","content":"","tool_calls":[{"id":"...","name":"...","arguments":"{...}"}],"ts":"..."}
//! {"type":"tool_result","tool_call_id":"...","content":"...","ts":"..."}
//! {"type":"assistant","content":[{"type":"text","text":"done"}],"ts":"..."}
//! ```
//!
//! Assistant `tool_calls` on disk are flat Grok shape (`id`/`name`/`arguments`).
//! On load they are rebuilt to OpenAI nested `function.{name,arguments}` for the agent.
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`content`] | pure content-block / plain-string conversion |
//! | [`tool_calls`] | pure OpenAI ↔ Grok tool_calls mapping |
//! | [`time`] | RFC3339 ↔ ms |
//! | [`io`] | chat_history / events file I/O |

mod content;
mod io;
mod time;
mod tool_calls;

pub use io::{
    append_event, append_exchange, append_record, append_records, read_all,
    records_to_openai_messages, write_all,
};

use serde_json::{json, Map, Value};

use content::{blocks_to_content, content_to_blocks, content_to_plain, flatten_content_value};
use time::{ms_to_rfc3339, now_ms, rfc3339_to_ms};
use tool_calls::{extract_tool_calls, openai_tool_calls_from_disk};

/// One logical message in the session history.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptRecord {
    pub ts_ms: u64,
    /// Wire role: `user` | `assistant` | `system` | `tool` (disk type is `tool_result`).
    pub role: String,
    /// Message body. Shape depends on role:
    /// - user/system/plain assistant: string or content-block array
    /// - assistant + tools: object with optional `content` + `tool_calls`
    /// - tool: object `{ tool_call_id, content }`
    pub content: Value,
}

impl TranscriptRecord {
    /// Build a record with the current wall-clock time in milliseconds.
    pub fn now(role: impl Into<String>, content: Value) -> Self {
        Self {
            ts_ms: now_ms(),
            role: role.into(),
            content,
        }
    }

    /// Build from an in-memory agent [`crate::context::Message`]-like pair.
    pub fn from_role_content(role: &str, content: Value) -> Self {
        Self {
            ts_ms: now_ms(),
            role: role.to_string(),
            content,
        }
    }

    /// Grok-style JSON line for `chat_history.jsonl`.
    pub(super) fn to_json_line(&self) -> Result<String, String> {
        let ts = ms_to_rfc3339(self.ts_ms);
        let v = match self.role.as_str() {
            "tool" | "tool_result" => {
                let tool_call_id = self
                    .content
                    .get("tool_call_id")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                let body = self
                    .content
                    .get("content")
                    .map(content_to_plain)
                    .unwrap_or_else(|| content_to_plain(&self.content));
                json!({
                    "type": "tool_result",
                    "tool_call_id": tool_call_id,
                    "content": body,
                    "ts": ts,
                })
            }
            "assistant" => {
                // Assistant with tool_calls: content is the raw LLM message object.
                if let Some(calls) = extract_tool_calls(&self.content) {
                    let text = self
                        .content
                        .get("content")
                        .map(content_to_plain)
                        .unwrap_or_default();
                    // Grok uses a string (often "") for tool-call assistant rows.
                    json!({
                        "type": "assistant",
                        "content": text,
                        "tool_calls": calls,
                        "ts": ts,
                    })
                } else {
                    json!({
                        "type": "assistant",
                        "content": content_to_blocks(&self.content),
                        "ts": ts,
                    })
                }
            }
            "system" => {
                // Grok stores system as a plain string body.
                json!({
                    "type": "system",
                    "content": content_to_plain(&self.content),
                    "ts": ts,
                })
            }
            role => {
                // user (and any other text roles)
                json!({
                    "type": role,
                    "content": content_to_blocks(&self.content),
                    "ts": ts,
                })
            }
        };
        serde_json::to_string(&v).map_err(|e| format!("serialize chat_history record: {e}"))
    }

    pub(super) fn from_json_value(v: Value) -> Result<Self, String> {
        // Grok wire: {"type":"user"|"assistant"|"system"|"tool_result", ...}
        let ty = v
            .get("type")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "missing or invalid type".to_string())?;

        // Prefer RFC3339 `ts`; missing/invalid → 0 (resilient).
        let ts_ms = v
            .get("ts")
            .and_then(|x| x.as_str())
            .map(rfc3339_to_ms)
            .unwrap_or(0);

        match ty {
            "tool_result" => {
                let tool_call_id = v
                    .get("tool_call_id")
                    .cloned()
                    .unwrap_or(Value::String(String::new()));
                let content_body = v.get("content").cloned().unwrap_or(Value::Null);
                Ok(Self {
                    ts_ms,
                    role: "tool".into(),
                    content: json!({
                        "tool_call_id": tool_call_id,
                        "content": content_body,
                    }),
                })
            }
            "assistant" => {
                if v.get("tool_calls").is_some() {
                    // Rebuild agent-facing assistant object (OpenAI-style tool_calls).
                    let mut obj = Map::new();
                    obj.insert("role".into(), json!("assistant"));
                    let content = v
                        .get("content")
                        .cloned()
                        .unwrap_or(Value::String(String::new()));
                    obj.insert("content".into(), flatten_content_value(content));
                    let calls =
                        openai_tool_calls_from_disk(v.get("tool_calls").cloned().unwrap_or(Value::Null));
                    obj.insert("tool_calls".into(), calls);
                    return Ok(Self {
                        ts_ms,
                        role: "assistant".into(),
                        content: Value::Object(obj),
                    });
                }
                let content = v
                    .get("content")
                    .cloned()
                    .ok_or_else(|| "missing content".to_string())?;
                Ok(Self {
                    ts_ms,
                    role: "assistant".into(),
                    content: blocks_to_content(content),
                })
            }
            "system" => {
                let content = v
                    .get("content")
                    .cloned()
                    .ok_or_else(|| "missing content".to_string())?;
                Ok(Self {
                    ts_ms,
                    role: "system".into(),
                    content: flatten_content_value(content),
                })
            }
            "reasoning" => {
                // Grok reasoning rows — keep for audit; agent skips unknown roles.
                let content = v
                    .get("summary")
                    .cloned()
                    .or_else(|| v.get("content").cloned())
                    .unwrap_or(Value::Null);
                Ok(Self {
                    ts_ms,
                    role: "reasoning".into(),
                    content,
                })
            }
            other => {
                let content = v
                    .get("content")
                    .cloned()
                    .ok_or_else(|| "missing content".to_string())?;
                Ok(Self {
                    ts_ms,
                    role: other.to_string(),
                    content: blocks_to_content(content),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_json_line_user_uses_content_blocks() {
        let rec = TranscriptRecord {
            ts_ms: 1_700_000_000_000,
            role: "user".into(),
            content: Value::String("hello".into()),
        };
        let line = rec.to_json_line().unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["type"], "user");
        assert!(v["content"].is_array());
        assert_eq!(v["content"][0]["text"], "hello");
    }

    #[test]
    fn from_json_value_tool_result_maps_to_tool_role() {
        let v = json!({
            "type": "tool_result",
            "tool_call_id": "c1",
            "content": "ok",
            "ts": "2023-11-14T22:13:20.000Z"
        });
        let rec = TranscriptRecord::from_json_value(v).unwrap();
        assert_eq!(rec.role, "tool");
        assert_eq!(rec.content["tool_call_id"], "c1");
        assert_eq!(rec.content["content"], "ok");
    }
}
