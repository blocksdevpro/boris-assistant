//! Pending HITL tool call state for pause/resume.

use serde_json::Value;

use crate::tool::ToolRisk;

/// A tool call waiting for user confirmation (not yet executed).
#[derive(Debug, Clone, PartialEq)]
pub struct PendingToolCall {
    /// Stable id for resume matching (not the LLM tool_call id).
    pub id: String,
    pub name: String,
    pub args: Value,
    /// Voice-safe one-liner for prompts / UI detail.
    pub args_summary: String,
    pub risk: ToolRisk,
    /// Provider tool_call id for the observation message.
    pub call_id: String,
}

impl PendingToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        args: Value,
        args_summary: impl Into<String>,
        risk: ToolRisk,
        call_id: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            args,
            args_summary: args_summary.into(),
            risk,
            call_id: call_id.into(),
        }
    }
}

/// One raw tool call still waiting in the same model round.
#[derive(Debug, Clone)]
pub struct RawToolCall {
    pub call_id: String,
    pub name: String,
    pub args: Value,
}

/// Engine state while paused for confirmation.
#[derive(Debug, Clone)]
pub struct PendingTurn {
    pub pending: PendingToolCall,
    /// Sibling tool calls in the same assistant message after the pending one.
    pub remaining_calls: Vec<RawToolCall>,
    pub tools_used: Vec<String>,
    pub tool_rounds: u32,
    /// Confirms already used this user turn (including this pending one).
    pub confirms_used: u32,
    /// Original user text for post-turn learn after final outcome.
    pub user_text: String,
}
