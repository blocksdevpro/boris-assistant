//! Harness types shared by the pure loop and the Agent facade.
//!
//! Inspired by `assets/tau/agent/src/types.rs`, sized for voice + HITL.

use crate::context::Role;
use crate::outcome::AgentOutcome;
use crate::runtime::PendingToolCall;

/// Default hard cap on tool-call rounds per user turn.
///
/// Voice multi-step work (files, web, todos) needs headroom; the loop forces a
/// final spoken reply if the model is still calling tools at this cap.
pub const DEFAULT_MAX_TOOL_ROUNDS: u32 = 16;

/// Higher cap when skills are enabled so multi-step playbooks can finish.
pub const SKILLS_MAX_TOOL_ROUNDS: u32 = 28;

/// Snapshot passed into each loop invocation (messages already include prompts).
#[derive(Debug, Clone)]
pub struct AgentContextSnapshot {
    pub system_prompt: String,
    /// Approximate serialized context size before the loop (for reports).
    pub approx_chars_in: usize,
}

/// Configuration for one loop run.
#[derive(Debug, Clone)]
pub struct AgentLoopConfig {
    pub max_tool_rounds: u32,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    /// Listing / concurrency / progress flags for this run.
    pub features: crate::runtime::ToolRuntimeFeatures,
    /// When true (subagent children), always dump full child tool list.
    pub force_list_all: bool,
    /// Latest user-task traits for routing / listing / finish gates.
    pub task: Option<crate::task::TaskTraits>,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_tool_rounds: DEFAULT_MAX_TOOL_ROUNDS,
            session_id: None,
            turn_id: None,
            features: crate::runtime::ToolRuntimeFeatures::default(),
            force_list_all: false,
            task: None,
        }
    }
}

/// Result of the pure ReAct loop (before personal-memory learning).
#[derive(Debug, Clone)]
pub struct LoopResult {
    pub outcome: AgentOutcome,
    pub tool_rounds: u32,
    pub tools_used: Vec<String>,
    /// When paused for HITL, the loop stores pending turn state on the agent.
    pub pending_turn: Option<crate::runtime::PendingTurn>,
}

/// Lifecycle events for UI / stats (tau-style, voice-sized surface).
#[derive(Debug, Clone)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd {
        outcome: AgentOutcome,
    },
    TurnStart {
        round: u32,
    },
    TurnEnd {
        round: u32,
    },
    MessageEnd {
        role: Role,
        preview: String,
    },
    ToolExecutionStart {
        call_id: String,
        tool_name: String,
        args_summary: String,
    },
    ToolExecutionEnd {
        call_id: String,
        tool_name: String,
        ok: bool,
        duration_ms: u64,
    },
    /// Mid-execution progress (slim UI label; rate-limited).
    ToolProgress {
        call_id: String,
        tool_name: String,
        message: String,
        byte_total: Option<u64>,
    },
    /// Live model reasoning preview (throttled; not spoken, not stored in context).
    Reasoning {
        preview: String,
    },
    NeedsConfirmation {
        pending: PendingToolCall,
    },
    Error {
        message: String,
    },
}

/// Shared emit handle used by the loop and progress sinks.
pub type EmitFn = std::sync::Arc<dyn Fn(AgentEvent) + Send + Sync>;

/// Listener type for [`crate::Agent::subscribe`].
pub type EventListener = Box<dyn Fn(&AgentEvent) + Send + Sync>;
