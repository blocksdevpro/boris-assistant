//! [`Tool`] trait — observation-only capabilities for the LLM.

use async_trait::async_trait;
use serde_json::Value;

use super::error::ToolError;
use super::meta::{ToolMeta, ToolRisk};

/// Capability the LLM may invoke during the engine tool loop.
///
/// # Observation-only contract
///
/// Implementations return data **to the model** (tool observations). They must
/// **never speak** to the user: no TTS, no playback, no app event bus. Final
/// speech is always [`crate::AgentOutcome`] from [`crate::Agent::prompt`].
///
/// Keep results **short** (prefer under [`crate::MAX_TOOL_RESULT_CHARS`]; use
/// [`crate::truncate_tool_result`]) — Boris is a voice agent and long tool
/// payloads bloat context and slow the turn.
///
/// # Safety
///
/// Bodies stay dumb. Policy, sandbox, timeouts, truncation, audit, and HITL
/// live in [`crate::runtime::ToolRuntime`] — not inside `execute`.
///
/// That policy runtime enforces gates purely from what [`Tool::meta`] declares
/// (`Permission`s, `ToolRisk`, `requires_confirmation`) — it never inspects
/// `execute`'s actual behavior. The default `meta()` is default-deny-ish
/// (`Moderate`, no permissions, requires confirmation); see [`Tool::meta`]'s
/// own doc comment before implementing a tool with real side effects.
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
/// poll [`crate::tool_context::ToolCallContext::is_cancelled`].
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
    /// Default: [`ToolMeta::deny_default`] (`Moderate`, `Other`, no permissions,
    /// requires confirmation). Override with accurate `Permission`s and `ToolRisk`
    /// for every tool. Production tools should set
    /// [`.read_only(...)`](ToolMeta::read_only) so wave scheduling can fan them out.
    ///
    /// # Safety
    ///
    /// The runtime's hard gates (path allowlists, [`crate::runtime::ShellPolicy`],
    /// [`crate::runtime::NetworkPolicy`]) and HITL confirmation are **only as
    /// strong as the `Permission`s and `risk` a tool declares here** — they never
    /// inspect what `execute` actually does. The default (`Moderate` risk, no
    /// permissions, requires confirmation) is **default-deny-ish**: an
    /// implementation that forgets to override `meta()` pauses for HITL instead
    /// of being silently auto-allowed, even if `execute` only reads local facts.
    /// Any tool that touches the filesystem, network, clipboard, or shell —
    /// or has other side effects a user would want to approve — **must**
    /// override `meta()` with accurate `Permission`s and `ToolRisk`.
    /// `#[must_use]` on the returned [`ToolMeta`] is not enforceable on trait
    /// defaults; treat a missing `meta()` override as a bug and add explicit
    /// `meta()` to every production tool.
    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Moderate).confirm(true)
    }

    /// Progressive listing opt-in. Default **`false`**.
    ///
    /// When progressive listing is on, only core tools, activated tools, and
    /// tools that return `true` here appear in the model tool list.
    fn should_list(&self, _ctx: &crate::runtime::ListToolsContext) -> bool {
        false
    }

    /// Run the tool with the JSON args the LLM supplied.
    ///
    /// The returned string is sent back to the LLM as the tool result only —
    /// never treated as user-facing speech. Prefer short, factual observations.
    /// Long tools may call [`crate::tool_context::ToolCallContext::report`] for
    /// host progress.
    async fn execute(
        &self,
        ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolKind;

    struct DefaultMetaTool;

    #[async_trait]
    impl Tool for DefaultMetaTool {
        fn name(&self) -> &str {
            "default_meta_probe"
        }
        fn description(&self) -> &str {
            "probe"
        }
        fn parameters(&self) -> Value {
            serde_json::json!({"type":"object","properties":{},"required":[]})
        }
        async fn execute(
            &self,
            _ctx: &crate::tool_context::ToolCallContext,
            _args: Value,
        ) -> Result<String, ToolError> {
            Ok("ok".into())
        }
    }

    #[test]
    fn default_meta_is_deny_ish() {
        let m = DefaultMetaTool.meta();
        assert_eq!(m.risk, ToolRisk::Moderate);
        assert!(m.requires_confirmation);
        assert_eq!(m.kind, ToolKind::Other);
        assert!(!m.is_read_only());
    }
}
