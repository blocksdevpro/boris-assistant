use std::fmt;

use serde_json::{Map, Value};

// ── ToolError ─────────────────────────────────────────────────────────────────

/// Classification of a [`ToolError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolErrorKind {
    /// Arguments failed validation (missing/wrong type/shape).
    InvalidArgs,
    /// Tool ran but failed to produce a useful result.
    Failed,
    /// Tool exceeded its time budget.
    Timeout,
    /// Result (or intermediate data) was truncated past a hard limit.
    Truncated,
    /// Unclassified / catch-all.
    Other,
}

/// Failure while executing a registered tool.
///
/// `message` is public for back-compat with the engine tool loop
/// (`format!("Error: {}", e.message)`). Prefer constructors over struct literals.
#[derive(Debug)]
pub struct ToolError {
    pub message: String,
    kind: ToolErrorKind,
}

impl ToolError {
    /// Create an error with kind [`ToolErrorKind::Other`].
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ToolErrorKind::Other,
        }
    }

    /// Arguments failed validation.
    pub fn invalid_args(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ToolErrorKind::InvalidArgs,
        }
    }

    /// Tool execution failed after args were accepted.
    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ToolErrorKind::Failed,
        }
    }

    /// Tool timed out.
    pub fn timeout(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ToolErrorKind::Timeout,
        }
    }

    /// Output or payload was truncated past a hard limit.
    pub fn truncated(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ToolErrorKind::Truncated,
        }
    }

    pub fn kind(&self) -> ToolErrorKind {
        self.kind
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ToolError {}

// ── Author helpers ────────────────────────────────────────────────────────────

/// Cap tool observation length so context doesn't explode.
pub const MAX_TOOL_RESULT_CHARS: usize = 4000;

const TRUNCATED_SUFFIX: &str = "\n…[truncated]";

/// Cap tool observation length so context doesn't explode (e.g. 4000 chars).
///
/// When cut, appends a short marker so the model knows the result was truncated.
pub fn truncate_tool_result(s: String) -> String {
    let count = s.chars().count();
    if count <= MAX_TOOL_RESULT_CHARS {
        return s;
    }
    let keep = MAX_TOOL_RESULT_CHARS.saturating_sub(TRUNCATED_SUFFIX.chars().count());
    let head: String = s.chars().take(keep).collect();
    format!("{head}{TRUNCATED_SUFFIX}")
}

/// Require `args` to be a JSON object; return the map or [`ToolError::invalid_args`].
pub fn require_object(args: &Value) -> Result<&Map<String, Value>, ToolError> {
    args.as_object().ok_or_else(|| {
        ToolError::invalid_args(format!(
            "tool args must be a JSON object, got {}",
            value_type_name(args)
        ))
    })
}

/// Optional string field: `None` if missing or not a JSON string.
pub fn optional_string(obj: &Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// Required string field; error if missing or not a JSON string.
pub fn require_string(obj: &Map<String, Value>, key: &str) -> Result<String, ToolError> {
    match obj.get(key) {
        None => Err(ToolError::invalid_args(format!(
            "missing required string argument `{key}`"
        ))),
        Some(v) => match v.as_str() {
            Some(s) => Ok(s.to_string()),
            None => Err(ToolError::invalid_args(format!(
                "argument `{key}` must be a string, got {}",
                value_type_name(v)
            ))),
        },
    }
}

fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

// ── Tool trait ────────────────────────────────────────────────────────────────

/// Capability the LLM may invoke during the engine tool loop.
///
/// # Observation-only contract
///
/// Implementations return data **to the model** (tool observations). They must
/// **never speak** to the user: no TTS, no playback, no app event bus. Final
/// speech is always [`crate::AgentOutcome`] from [`crate::AgentEngine::chat`].
///
/// Keep results **short** (prefer under [`MAX_TOOL_RESULT_CHARS`]; use
/// [`truncate_tool_result`]) — Boris is a voice agent and long tool payloads
/// bloat context and slow the turn.
pub trait Tool: Send + Sync {
    /// Snake_case name the LLM uses to invoke this tool.
    fn name(&self) -> &str;

    /// Plain-English description so the LLM knows when to use it.
    fn description(&self) -> &str;

    /// JSON Schema object describing the tool's accepted arguments.
    /// Use `json!({ "type": "object", "properties": {}, "required": [] })`
    /// for tools that take no arguments.
    fn parameters(&self) -> Value;

    /// Run the tool with the JSON args the LLM supplied.
    ///
    /// The returned string is sent back to the LLM as the tool result only —
    /// never treated as user-facing speech. Prefer short, factual observations.
    fn execute(&self, args: Value) -> Result<String, ToolError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truncate_leaves_short_strings_unchanged() {
        let s = "hello".to_string();
        assert_eq!(truncate_tool_result(s.clone()), s);
    }

    #[test]
    fn truncate_caps_long_strings() {
        let long: String = "a".repeat(MAX_TOOL_RESULT_CHARS + 500);
        let out = truncate_tool_result(long);
        assert!(out.chars().count() <= MAX_TOOL_RESULT_CHARS);
        assert!(out.ends_with(TRUNCATED_SUFFIX));
        assert!(out.starts_with('a'));
    }

    #[test]
    fn truncate_at_exact_limit_is_unchanged() {
        let exact: String = "x".repeat(MAX_TOOL_RESULT_CHARS);
        let out = truncate_tool_result(exact.clone());
        assert_eq!(out, exact);
        assert!(!out.contains("[truncated]"));
    }

    #[test]
    fn require_string_ok() {
        let obj = json!({ "name": "boris", "n": 1 })
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(require_string(&obj, "name").unwrap(), "boris");
    }

    #[test]
    fn require_string_missing() {
        let obj = Map::new();
        let err = require_string(&obj, "name").unwrap_err();
        assert_eq!(err.kind(), ToolErrorKind::InvalidArgs);
        assert!(err.message.contains("missing"));
        assert!(err.message.contains("name"));
    }

    #[test]
    fn require_string_wrong_type() {
        let obj = json!({ "name": 42 }).as_object().unwrap().clone();
        let err = require_string(&obj, "name").unwrap_err();
        assert_eq!(err.kind(), ToolErrorKind::InvalidArgs);
        assert!(err.message.contains("string"));
    }

    #[test]
    fn optional_string_behaviour() {
        let obj = json!({ "a": "yes", "b": 1 }).as_object().unwrap().clone();
        assert_eq!(optional_string(&obj, "a").as_deref(), Some("yes"));
        assert_eq!(optional_string(&obj, "b"), None);
        assert_eq!(optional_string(&obj, "missing"), None);
    }

    #[test]
    fn require_object_ok_and_err() {
        let v = json!({ "x": 1 });
        let map = require_object(&v).unwrap();
        assert_eq!(map.get("x").and_then(|n| n.as_i64()), Some(1));

        let err = require_object(&json!([])).unwrap_err();
        assert_eq!(err.kind(), ToolErrorKind::InvalidArgs);
    }

    #[test]
    fn tool_error_constructors_set_kind() {
        assert_eq!(ToolError::new("x").kind(), ToolErrorKind::Other);
        assert_eq!(
            ToolError::invalid_args("x").kind(),
            ToolErrorKind::InvalidArgs
        );
        assert_eq!(ToolError::failed("x").kind(), ToolErrorKind::Failed);
        assert_eq!(ToolError::timeout("x").kind(), ToolErrorKind::Timeout);
        assert_eq!(ToolError::truncated("x").kind(), ToolErrorKind::Truncated);
        assert_eq!(ToolError::new("msg").message, "msg");
    }
}
