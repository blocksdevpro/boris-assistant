//! Pending HITL tool call state for pause/resume.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tool::ToolRisk;

/// What the on-screen field should collect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputKind {
    /// Short exact string (email, path, branch). Visible.
    Exact,
    /// Secret (password, token). Masked. Redacted on persist.
    Secret,
    /// Large paste. Textarea.
    Blob,
}

impl InputKind {
    /// Parse `exact` / `secret` / `blob` (and a few aliases). Unknown → Exact.
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "secret" | "password" | "token" | "key" => Self::Secret,
            "blob" | "text" | "paste" | "multiline" => Self::Blob,
            _ => Self::Exact,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Secret => "secret",
            Self::Blob => "blob",
        }
    }

    pub fn is_secret(self) -> bool {
        matches!(self, Self::Secret)
    }

    pub fn multiline(self) -> bool {
        matches!(self, Self::Blob)
    }

    pub fn max_chars(self) -> u32 {
        match self {
            Self::Exact => 512,
            Self::Secret => 2048,
            Self::Blob => 12_000,
        }
    }

    pub fn default_label(self) -> &'static str {
        match self {
            Self::Exact => "Exact text",
            Self::Secret => "Secret",
            Self::Blob => "Paste text",
        }
    }
}

/// Field spec carried on a pending collect_input pause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingInput {
    pub kind: InputKind,
    pub label: String,
    pub max_chars: u32,
}

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
    /// When set, the host collects typed input instead of yes/no.
    pub input: Option<PendingInput>,
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
            input: None,
        }
    }

    pub fn with_input(mut self, input: PendingInput) -> Self {
        self.input = Some(input);
        self
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
    /// Extra confirm-needed calls approved by the same yes (NOT including pending).
    pub batch_with: Vec<RawToolCall>,
    /// Sibling tool calls after `pending` + `batch_with` (may still need their own confirms).
    pub remaining_calls: Vec<RawToolCall>,
    pub tools_used: Vec<String>,
    pub tool_rounds: u32,
    /// Confirms already used this user turn (including this pending HITL decision).
    pub confirms_used: u32,
    /// Original user text for post-turn learn after final outcome.
    pub user_text: String,
}
