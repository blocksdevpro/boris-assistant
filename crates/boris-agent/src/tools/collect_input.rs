//! Pause the loop so the host can collect typed or pasted input.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::runtime::InputKind;
use crate::tool::{require_object, Tool, ToolError, ToolKind, ToolMeta, ToolRisk};

/// Marker prefix on secret observations so persist/export can redact them.
pub const SECRET_INPUT_MARK: &str = "[boris-secret]";

/// Ask the user to type or paste something voice is a bad channel for.
#[derive(Debug, Default, Clone, Copy)]
pub struct CollectInputTool;

#[async_trait]
impl Tool for CollectInputTool {
    fn name(&self) -> &str {
        "collect_input"
    }

    fn description(&self) -> &str {
        "Ask the user to type or paste on screen. Use for secrets (passwords, API keys, tokens), \
         exact strings (email, path, branch, code), or large pastes (logs, JSON, drafts). \
         Never ask them to speak those. Speak one short line telling them to use the field. \
         kind: exact | secret | blob."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "spoken": {
                    "type": "string",
                    "description": "One short sentence to speak. Do not include the secret. Tell them to type or paste on screen."
                },
                "kind": {
                    "type": "string",
                    "description": "exact (visible short string), secret (masked, redacted later), blob (textarea for a large paste)."
                },
                "label": {
                    "type": "string",
                    "description": "Short field label, e.g. API key, email, stack trace."
                }
            },
            "required": ["spoken"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::System)
            .collect_input(true)
            .read_only(false)
            .max_concurrency(1)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        _args: Value,
    ) -> Result<String, ToolError> {
        Err(ToolError::failed(
            "collect_input pauses for the on-screen field; it should not execute directly",
        ))
    }
}

/// Parse collect_input args into a field spec + speakable line.
pub fn parse_collect_args(args: &Value) -> (InputKind, String, String, u32) {
    let obj = require_object(args).ok();
    let kind = obj
        .and_then(|o| o.get("kind"))
        .and_then(|v| v.as_str())
        .map(InputKind::parse)
        .unwrap_or(InputKind::Exact);
    let spoken = obj
        .and_then(|o| o.get("spoken"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Type it on screen. I will not read it back.")
        .to_string();
    let label = obj
        .and_then(|o| o.get("label"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| kind.default_label())
        .to_string();
    (kind, spoken, label, kind.max_chars())
}

/// Observation fed back to the model after the host collected a value.
pub fn format_input_observation(kind: InputKind, value: &str) -> String {
    match kind {
        InputKind::Secret => format!(
            "{SECRET_INPUT_MARK}\nUser provided a secret. Never speak it, store it in memory, \
             or put it in artifacts or URLs.\n{value}"
        ),
        InputKind::Exact => format!("User typed:\n{value}"),
        InputKind::Blob => format!("User pasted:\n{value}"),
    }
}

/// Replace secret collect_input observations before writing transcripts.
pub fn redact_secret_tool_content(content: &Value) -> Value {
    fn redact_str(s: &str) -> Option<String> {
        if s.starts_with(SECRET_INPUT_MARK) || s.contains(SECRET_INPUT_MARK) {
            Some("User provided a secret (redacted).".into())
        } else {
            None
        }
    }
    if let Some(s) = content.as_str() {
        if let Some(r) = redact_str(s) {
            return Value::String(r);
        }
        return content.clone();
    }
    let Some(obj) = content.as_object() else {
        return content.clone();
    };
    let Some(inner) = obj.get("content").and_then(|v| v.as_str()) else {
        return content.clone();
    };
    let Some(redacted) = redact_str(inner) else {
        return content.clone();
    };
    let mut out = obj.clone();
    out.insert("content".into(), Value::String(redacted));
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kind_aliases() {
        let args = json!({"spoken": "Paste the token.", "kind": "token", "label": "API key"});
        let (kind, spoken, label, max) = parse_collect_args(&args);
        assert_eq!(kind, InputKind::Secret);
        assert!(spoken.contains("token"));
        assert_eq!(label, "API key");
        assert_eq!(max, InputKind::Secret.max_chars());
    }

    #[test]
    fn redact_secret_mark() {
        let raw = format_input_observation(InputKind::Secret, "ghp_secret");
        let v = json!({"tool_call_id": "c1", "content": raw});
        let out = redact_secret_tool_content(&v);
        assert_eq!(out["content"], "User provided a secret (redacted).");
        assert!(!out["content"].as_str().unwrap().contains("ghp_"));
    }
}
