//! Pending HITL tool call state for pause/resume.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::finish_gate::FinishGateBudget;
use crate::tool::ToolRisk;
use crate::types::TokenAccounting;

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
    /// Numbered choice (voice disambiguation). Host speaks options, user says number.
    Choice,
}

impl InputKind {
    /// Parse `exact` / `secret` / `blob` / `choice` (and a few aliases). Unknown → Exact.
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "secret" | "password" | "token" | "key" => Self::Secret,
            "blob" | "text" | "paste" | "multiline" => Self::Blob,
            "choice" | "choose" | "select" | "options" | "option" => Self::Choice,
            _ => Self::Exact,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Secret => "secret",
            Self::Blob => "blob",
            Self::Choice => "choice",
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
            Self::Choice => 128,
        }
    }

    pub fn default_label(self) -> &'static str {
        match self {
            Self::Exact => "Exact text",
            Self::Secret => "Secret",
            Self::Blob => "Paste text",
            Self::Choice => "Choose",
        }
    }
}

/// Field spec carried on a pending collect_input pause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingInput {
    pub kind: InputKind,
    pub label: String,
    pub max_chars: u32,
    /// Numbered options for `Choice` (empty otherwise). Max 4, each ≤40 chars.
    pub options: Vec<String>,
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
    /// Session-bound todos file (`{session_dir}/todos.json`) at pause time.
    /// `None` falls back to the sandbox guess on resume (fresh turns always
    /// populate this; resume must restore it instead of passing `None`).
    pub todos_file: Option<PathBuf>,
    /// Remaining markup-only re-prompt budget at pause time.
    pub markup_left: u32,
    /// Remaining research / todo finish-gate budget at pause time.
    /// Post-HITL resume forces this to `0` (see
    /// [`FinishGateBudget::post_hitl`]) so an approval cannot loop back into
    /// finish-gate nudges, while `markup_left` is preserved separately.
    pub gate_left: u32,
    /// Token accounting accumulated before the pause (request estimates +
    /// provider usage). Resume restores this as the starting accounting so a
    /// HITL pause never discards pre-pause LLM cost.
    pub token_accounting: TokenAccounting,
}

impl PendingTurn {
    /// Current finish-gate budget snapshot.
    pub fn finish_budget(&self) -> FinishGateBudget {
        FinishGateBudget {
            markup: self.markup_left,
            gate: self.gate_left,
        }
    }

    /// Post-HITL budget: preserved markup, gate forced to zero.
    pub fn post_hitl_budget(&self) -> FinishGateBudget {
        FinishGateBudget::post_hitl(self.markup_left)
    }
}
