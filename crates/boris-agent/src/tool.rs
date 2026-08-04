use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
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

/// Cap tool observation length so context doesn't explode (voice-sized default).
pub const MAX_TOOL_RESULT_CHARS: usize = 4000;

/// Higher cap for skill bodies (playbooks must stay intact for multi-step work).
pub const MAX_SKILL_RESULT_CHARS: usize = 24_000;

/// Soft-wrap width for long single lines (bash / dumps) — content preserved.
pub const DEFAULT_SOFT_WRAP_WIDTH: usize = 2_000;

const TRUNCATED_SUFFIX: &str = "\n…[truncated]";

/// Cap tool observation length so context doesn't explode (e.g. 4000 chars).
///
/// When cut, appends a short marker so the model knows the result was truncated.
pub fn truncate_tool_result(s: String) -> String {
    truncate_tool_result_to(s, MAX_TOOL_RESULT_CHARS)
}

/// Cap to an explicit character budget (UTF-8 safe by char count).
///
/// Output is always ≤ `max_chars` characters (including the truncation marker).
pub fn truncate_tool_result_to(s: String, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max_chars {
        return s;
    }
    let suffix_len = TRUNCATED_SUFFIX.chars().count();
    if max_chars <= suffix_len {
        return TRUNCATED_SUFFIX.chars().take(max_chars).collect();
    }
    let keep = max_chars - suffix_len;
    let head: String = s.chars().take(keep).collect();
    format!("{head}{TRUNCATED_SUFFIX}")
}

/// Soft-wrap a long line by inserting newlines every `wrap_width` characters.
/// **All content is preserved** (Grok bash strategy for long lines).
pub fn soft_wrap_line(line: &str, wrap_width: usize) -> String {
    if wrap_width == 0 || line.chars().count() <= wrap_width {
        return line.to_string();
    }
    let mut result = String::with_capacity(line.len() + line.len() / wrap_width);
    let mut on_line = 0;
    for ch in line.chars() {
        if on_line >= wrap_width {
            result.push('\n');
            on_line = 0;
        }
        result.push(ch);
        on_line += 1;
    }
    result
}

/// Soft-wrap every line of a multi-line string that exceeds `wrap_width`.
pub fn soft_wrap_text(text: &str, wrap_width: usize) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&soft_wrap_line(line, wrap_width));
    }
    out
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

// ── Tool metadata (risk / permissions / timeout / kind) ──────────────────────

/// High-level tool category (Grok `ToolKind`, used for capability presets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ToolKind {
    Read,
    Write,
    Search,
    Execute,
    Web,
    Memory,
    Skill,
    System,
    Plan,
    #[default]
    Other,
}

impl ToolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Search => "search",
            Self::Execute => "execute",
            Self::Web => "web",
            Self::Memory => "memory",
            Self::Skill => "skill",
            Self::System => "system",
            Self::Plan => "plan",
            Self::Other => "other",
        }
    }

    /// True when the tool does not mutate external state by design.
    pub fn is_read_only(self) -> bool {
        matches!(
            self,
            Self::Read | Self::Search | Self::Memory | Self::Skill | Self::System | Self::Other
        )
    }
}

/// How dangerous a tool is for policy and HITL defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolRisk {
    /// Read-only local facts (time, recall notes, get profile).
    Safe = 0,
    /// Local durable writes in Boris data (notes, profile updates).
    Moderate = 1,
    /// External or mutable side effects (shell, write outside memory, open URL).
    Dangerous = 2,
    /// Irreversible / high-impact (delete, send, admin) — always confirm.
    Critical = 3,
}

impl ToolRisk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Moderate => "moderate",
            Self::Dangerous => "dangerous",
            Self::Critical => "critical",
        }
    }

    /// Default wall-clock budget for tools at this risk level.
    pub fn default_timeout(self) -> Duration {
        match self {
            Self::Safe => Duration::from_secs(5),
            Self::Moderate => Duration::from_secs(15),
            Self::Dangerous | Self::Critical => Duration::from_secs(60),
        }
    }
}

/// Capability scopes a tool may need. Policy gates these independently of risk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    None,
    FsRead,
    FsWrite,
    Network,
    Shell,
    Clipboard,
    UiControl,
}

/// Static metadata the runtime uses for policy, timeout, and confirmation.
#[derive(Debug, Clone)]
pub struct ToolMeta {
    pub risk: ToolRisk,
    pub permissions: &'static [Permission],
    pub default_timeout: Duration,
    /// When true, runtime always pauses for HITL before execute (unless granted).
    pub requires_confirmation: bool,
    /// Category for capability presets and parallel scheduling.
    pub kind: ToolKind,
    /// Override observation char cap after execute (`None` → [`MAX_TOOL_RESULT_CHARS`]).
    pub max_result_chars: Option<usize>,
}

impl ToolMeta {
    /// Safe, no special permissions, 5s timeout, no confirmation.
    pub fn safe_default() -> Self {
        Self {
            risk: ToolRisk::Safe,
            permissions: &[Permission::None],
            default_timeout: ToolRisk::Safe.default_timeout(),
            requires_confirmation: false,
            kind: ToolKind::Other,
            max_result_chars: None,
        }
    }

    pub fn with_risk(risk: ToolRisk) -> Self {
        Self {
            risk,
            permissions: &[Permission::None],
            default_timeout: risk.default_timeout(),
            requires_confirmation: false,
            kind: ToolKind::Other,
            max_result_chars: None,
        }
    }

    pub fn permissions(mut self, permissions: &'static [Permission]) -> Self {
        self.permissions = permissions;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    pub fn confirm(mut self, requires: bool) -> Self {
        self.requires_confirmation = requires;
        self
    }

    pub fn kind(mut self, kind: ToolKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn max_result_chars(mut self, max: usize) -> Self {
        self.max_result_chars = Some(max);
        self
    }

    /// Prefer kind-derived read-only when set; else treat Safe risk as read-only.
    pub fn is_read_only(&self) -> bool {
        self.kind.is_read_only() && self.risk <= ToolRisk::Moderate && !self.requires_confirmation
    }

    pub fn result_char_budget(&self) -> usize {
        self.max_result_chars.unwrap_or(MAX_TOOL_RESULT_CHARS)
    }
}

// ── Tool trait ────────────────────────────────────────────────────────────────

/// Capability the LLM may invoke during the engine tool loop.
///
/// # Observation-only contract
///
/// Implementations return data **to the model** (tool observations). They must
/// **never speak** to the user: no TTS, no playback, no app event bus. Final
/// speech is always [`crate::AgentOutcome`] from [`crate::Agent::prompt`].
///
/// Keep results **short** (prefer under [`MAX_TOOL_RESULT_CHARS`]; use
/// [`truncate_tool_result`]) — Boris is a voice agent and long tool payloads
/// bloat context and slow the turn.
///
/// # Safety
///
/// Bodies stay dumb. Policy, sandbox, timeouts, truncation, audit, and HITL
/// live in [`crate::runtime::ToolRuntime`] — not inside `execute`.
///
/// # Async
///
/// `execute` is async so I/O tools (web, shell, MCP) can await without blocking
/// the agent runtime. Call only via [`crate::runtime::ToolRuntime`].
///
/// # Context
///
/// Every call receives [`crate::tool_context::ToolCallContext`] (call id,
/// session, cwd, cancel). Most tools ignore it; long-running tools should
/// poll [`ToolCallContext::is_cancelled`].
#[async_trait]
pub trait Tool: Send + Sync {
    /// Snake_case name the LLM uses to invoke this tool.
    fn name(&self) -> &str;

    /// Plain-English description so the LLM knows when to use it.
    fn description(&self) -> &str;

    /// JSON Schema object describing the tool's accepted arguments.
    /// Use `json!({ "type": "object", "properties": {}, "required": [] })`
    /// for tools that take no arguments.
    fn parameters(&self) -> Value;

    /// Risk / permission / timeout metadata for the tool runtime.
    ///
    /// Default: [`ToolMeta::safe_default`]. Override for any tool that writes,
    /// networks, or needs confirmation.
    fn meta(&self) -> ToolMeta {
        ToolMeta::safe_default()
    }

    /// Run the tool with the JSON args the LLM supplied.
    ///
    /// The returned string is sent back to the LLM as the tool result only —
    /// never treated as user-facing speech. Prefer short, factual observations.
    async fn execute(
        &self,
        ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError>;
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
    fn truncate_to_custom_budget() {
        let s = "abcdefghij".to_string();
        let out = truncate_tool_result_to(s, 6);
        assert!(out.chars().count() <= 6);
        assert!(out.contains("…") || out.contains("truncated"));
    }

    #[test]
    fn soft_wrap_preserves_content() {
        let line = "a".repeat(5000);
        let wrapped = soft_wrap_line(&line, 2000);
        assert_eq!(wrapped.replace('\n', "").len(), 5000);
        assert!(wrapped.contains('\n'));
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
