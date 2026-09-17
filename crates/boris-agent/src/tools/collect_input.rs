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
         exact strings (email, path, branch, code), large pastes (logs, JSON, drafts), or numbered \
         choices (which file? which option? — kind choice with 2-4 short options). \
         Never ask them to speak secrets. Speak one short line telling them to use the field. \
         kind: exact | secret | blob | choice."
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
                    "description": "exact (visible short string), secret (masked, redacted later), blob (textarea for a large paste), choice (numbered options, say the number)."
                },
                "label": {
                    "type": "string",
                    "description": "Short field label, e.g. API key, email, stack trace."
                },
                "options": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "2-4 short options for kind=choice (each <=40 chars). Host speaks as numbered list."
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

/// Parsed collect_input field spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectSpec {
    pub kind: InputKind,
    pub spoken: String,
    pub label: String,
    pub max_chars: u32,
    pub options: Vec<String>,
}

/// Parse collect_input args into a field spec + speakable line.
///
/// `options` is clamped to 4 entries × 40 chars for voice speakability.
/// When `options` is non-empty and `kind` is omitted, kind upgrades to Choice.
pub fn parse_collect_args_full(args: &Value) -> CollectSpec {
    let obj = require_object(args).ok();
    let raw_options: Vec<String> = obj
        .and_then(|o| o.get("options"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.chars().take(40).collect::<String>())
                .take(4)
                .collect()
        })
        .unwrap_or_default();
    let mut kind = obj
        .and_then(|o| o.get("kind"))
        .and_then(|v| v.as_str())
        .map(InputKind::parse)
        .unwrap_or(if raw_options.is_empty() {
            InputKind::Exact
        } else {
            InputKind::Choice
        });
    // Non-empty options force Choice (prevents Exact+options confusion).
    if !raw_options.is_empty() && kind != InputKind::Choice && kind != InputKind::Exact {
        // Secret/blob with options is a model error — keep kind but drop options.
    }
    let options = if kind == InputKind::Choice || (kind == InputKind::Exact && !raw_options.is_empty()) {
        if kind == InputKind::Exact && !raw_options.is_empty() {
            kind = InputKind::Choice;
        }
        raw_options
    } else {
        Vec::new()
    };
    let mut spoken = obj
        .and_then(|o| o.get("spoken"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Type it on screen. I will not read it back.")
        .to_string();
    // Append numbered options to the spoken line for voice (host may re-render).
    if !options.is_empty() {
        let numbered: Vec<String> = options
            .iter()
            .enumerate()
            .map(|(i, o)| format!("{}: {o}", i + 1))
            .collect();
        if !spoken.ends_with('.') && !spoken.ends_with('?') {
            spoken.push('.');
        }
        spoken.push_str(&format!(" Options: {}.", numbered.join(", ")));
    }
    let label = obj
        .and_then(|o| o.get("label"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| kind.default_label())
        .to_string();
    CollectSpec {
        max_chars: kind.max_chars(),
        kind,
        spoken,
        label,
        options,
    }
}

/// Parse collect_input args into a field spec + speakable line.
pub fn parse_collect_args(args: &Value) -> (InputKind, String, String, u32, Vec<String>) {
    let spec = parse_collect_args_full(args);
    (
        spec.kind,
        spec.spoken,
        spec.label,
        spec.max_chars,
        spec.options,
    )
}

/// Observation fed back to the model after the host collected a value.
///
/// Secrets never echo the value: the model only learns a length-tagged
/// receipt. The raw value stays with the host; transcripts persist only the
/// redacted receipt via [`redact_secret_tool_content`].
pub fn format_input_observation(kind: InputKind, value: &str) -> String {
    match kind {
        InputKind::Secret => format!(
            "{SECRET_INPUT_MARK}\n\
             [boris-secret received, length={}, redacted from transcript] \
             Never speak it, store it in memory, or put it in artifacts or URLs. \
             Use only via approved tool.",
            value.chars().count()
        ),
        InputKind::Exact => format!("User typed:\n{value}"),
        InputKind::Blob => format!("User pasted:\n{value}"),
        InputKind::Choice => {
            // Host normalizes to `N` or `N: option`; echo back compactly.
            let v = value.trim();
            format!("User chose:\n{v}")
        }
    }
}

/// Format a choice observation with option resolution (`2` → `2: report.md`).
pub fn format_choice_observation(value: &str, options: &[String]) -> String {
    let v = value.trim();
    // Numeric selection: resolve to the option text when in range.
    if let Ok(n) = v.split(':').next().unwrap_or("").trim().parse::<usize>() {
        if n >= 1 && n <= options.len() {
            return format!("User chose:\n{n}: {}", options[n - 1]);
        }
    }
    format!("User chose:\n{v}")
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
        let (kind, spoken, label, max, options) = parse_collect_args(&args);
        assert_eq!(kind, InputKind::Secret);
        assert!(spoken.contains("token"));
        assert_eq!(label, "API key");
        assert_eq!(max, InputKind::Secret.max_chars());
        assert!(options.is_empty());
    }

    #[test]
    fn parse_choice_options() {
        let args = json!({
            "spoken": "Which file?",
            "kind": "choice",
            "label": "File",
            "options": ["a.md", "b.md", "c.md"]
        });
        let spec = parse_collect_args_full(&args);
        assert_eq!(spec.kind, InputKind::Choice);
        assert_eq!(spec.options.len(), 3);
        assert!(spec.spoken.contains("1: a.md"));
        // Options without explicit kind upgrade to Choice.
        let args = json!({"spoken": "Pick.", "options": ["x", "y"]});
        assert_eq!(parse_collect_args_full(&args).kind, InputKind::Choice);
        // Clamped to 4 × 40 chars.
        let args = json!({"spoken": "Pick.", "options": ["1","2","3","4","5","6"]});
        assert_eq!(parse_collect_args_full(&args).options.len(), 4);
    }

    #[test]
    fn redact_secret_mark() {
        let raw = format_input_observation(InputKind::Secret, "ghp_secret");
        // Secret value must never reach the model observation.
        assert!(!raw.contains("ghp_secret"));
        assert!(raw.contains("length=10"));
        assert!(raw.contains(SECRET_INPUT_MARK));
        let v = json!({"tool_call_id": "c1", "content": raw});
        let out = redact_secret_tool_content(&v);
        assert_eq!(out["content"], "User provided a secret (redacted).");
        assert!(!out["content"].as_str().unwrap().contains("ghp_"));
    }

    #[test]
    fn secret_observation_never_echoes_value() {
        let secret = "super-secret-value-123";
        let out = format_input_observation(InputKind::Secret, secret);
        assert!(!out.contains(secret));
        assert!(out.contains("redacted"));
        assert!(out.contains(&secret.chars().count().to_string()));
    }
}
