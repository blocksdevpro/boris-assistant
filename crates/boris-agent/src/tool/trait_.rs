//! [`Tool`] trait — observation-only capabilities for the LLM.

use async_trait::async_trait;
use serde_json::Value;

use super::error::ToolError;
use super::meta::ToolMeta;

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
    /// Default: [`ToolMeta::safe_default`]. Override for any tool that writes,
    /// networks, or needs confirmation. Production tools should set
    /// [`.read_only(...)`](ToolMeta::read_only) so wave scheduling can fan them out.
    fn meta(&self) -> ToolMeta {
        ToolMeta::safe_default()
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
