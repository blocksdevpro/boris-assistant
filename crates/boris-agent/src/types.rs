//! Harness types shared by the pure loop and the Agent facade.
//!
//! Inspired by `assets/tau/agent/src/types.rs`, sized for voice + HITL.

use boris_ai::TokenUsage;

use crate::context::Role;
use crate::outcome::AgentOutcome;
use crate::runtime::PendingToolCall;

/// Default hard cap on tool-call rounds per user turn.
///
/// The loop runs `0..=max_tool_rounds`, so one turn issues up to
/// `max_tool_rounds + 1` LLM completions; when the final round still has no
/// speakable text the loop may issue one extra no-tools forced-speak
/// completion so the host always has something to say.
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
    /// Hard cap on tool-call rounds. The loop iterates `0..=max_tool_rounds`
    /// (up to `max_tool_rounds + 1` completions) plus one optional no-tools
    /// forced-speak completion when the cap round comes back empty.
    pub max_tool_rounds: u32,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    /// Listing / concurrency / progress flags for this run.
    pub features: crate::runtime::ToolRuntimeFeatures,
    /// When true (subagent children), always dump full child tool list.
    pub force_list_all: bool,
    /// Latest user-task traits for routing / listing / finish gates.
    pub task: Option<crate::task::TaskTraits>,
    /// Eager tool bundles chosen locally from the human objective.
    pub tool_selection: Option<crate::runtime::ToolSelection>,
    /// Optional, process-local developer capture.
    pub debug: Option<std::sync::Arc<crate::debug::DebugCapture>>,
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
            tool_selection: None,
            debug: None,
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
    /// Request estimates plus provider-reported usage observed this invocation.
    pub token_accounting: TokenAccounting,
}

/// Token accounting for one loop invocation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenAccounting {
    /// Sum of provider-reported usage across all model calls in the loop.
    pub provider_usage: TokenUsage,
    /// Largest provider-reported prompt seen (the best exact context-meter value).
    pub peak_provider_prompt_tokens: Option<u64>,
    /// Largest complete request estimate, including serialized tool schemas.
    pub peak_estimated_request_tokens: u64,
    /// Configured combined prompt + completion model window.
    pub context_limit_tokens: Option<u32>,
}

impl TokenAccounting {
    pub fn record_request_estimate(&mut self, tokens: usize, context_limit_tokens: u32) {
        self.peak_estimated_request_tokens = self
            .peak_estimated_request_tokens
            .max(u64::try_from(tokens).unwrap_or(u64::MAX));
        self.context_limit_tokens = Some(context_limit_tokens);
    }

    pub fn record_provider_usage(&mut self, usage: &TokenUsage) {
        self.peak_provider_prompt_tokens = Some(
            self.peak_provider_prompt_tokens
                .unwrap_or(0)
                .max(usage.prompt_tokens),
        );
        self.provider_usage.prompt_tokens = self
            .provider_usage
            .prompt_tokens
            .saturating_add(usage.prompt_tokens);
        self.provider_usage.completion_tokens = self
            .provider_usage
            .completion_tokens
            .saturating_add(usage.completion_tokens);
        self.provider_usage.total_tokens = self
            .provider_usage
            .total_tokens
            .saturating_add(usage.total_tokens);
        self.provider_usage.cached_tokens = self
            .provider_usage
            .cached_tokens
            .saturating_add(usage.cached_tokens);
        self.provider_usage.cache_write_tokens = self
            .provider_usage
            .cache_write_tokens
            .saturating_add(usage.cache_write_tokens);
    }

    /// Fold accounting from a separate model call (for example maintenance
    /// compaction) into the report for the user-visible turn.
    ///
    /// Usage counters sum; peaks take the max. The merged context limit is the
    /// active request limit — the max of both sides (`Some` wins over `None`,
    /// larger window wins over smaller) — so a narrower maintenance call can
    /// never shrink the turn's reported model window.
    pub fn merge(&mut self, other: &Self) {
        self.provider_usage.prompt_tokens = self
            .provider_usage
            .prompt_tokens
            .saturating_add(other.provider_usage.prompt_tokens);
        self.provider_usage.completion_tokens = self
            .provider_usage
            .completion_tokens
            .saturating_add(other.provider_usage.completion_tokens);
        self.provider_usage.total_tokens = self
            .provider_usage
            .total_tokens
            .saturating_add(other.provider_usage.total_tokens);
        self.provider_usage.cached_tokens = self
            .provider_usage
            .cached_tokens
            .saturating_add(other.provider_usage.cached_tokens);
        self.provider_usage.cache_write_tokens = self
            .provider_usage
            .cache_write_tokens
            .saturating_add(other.provider_usage.cache_write_tokens);
        self.peak_provider_prompt_tokens = match (
            self.peak_provider_prompt_tokens,
            other.peak_provider_prompt_tokens,
        ) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (value @ Some(_), None) | (None, value @ Some(_)) => value,
            (None, None) => None,
        };
        self.peak_estimated_request_tokens = self
            .peak_estimated_request_tokens
            .max(other.peak_estimated_request_tokens);
        self.context_limit_tokens = match (self.context_limit_tokens, other.context_limit_tokens) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (value @ Some(_), None) | (None, value @ Some(_)) => value,
            (None, None) => None,
        };
    }

    pub fn context_used_tokens(&self) -> u32 {
        let used = self
            .peak_provider_prompt_tokens
            .unwrap_or(self.peak_estimated_request_tokens);
        u32::try_from(used).unwrap_or(u32::MAX)
    }

    pub fn context_is_estimated(&self) -> bool {
        self.peak_provider_prompt_tokens.is_none()
    }
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
    /// Short, silent progress note from the same assistant message as tool calls.
    ToolNote {
        text: String,
    },
    /// Full report delivered on screen even when normal card presentation failed.
    ReportFallback {
        meta: Option<crate::ArtifactMeta>,
        title: String,
        kind: String,
        language: Option<String>,
        body: String,
    },
    /// A bounded parallel dispatch, with child call IDs for debug correlation.
    ToolBatchStart {
        call_ids: Vec<String>,
        max_parallel: u32,
    },
    ToolBatchEnd {
        tool_count: usize,
        duration_ms: u64,
        failed: usize,
        paused: usize,
        cancelled: bool,
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
    NeedsInput {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_prompt_usage_replaces_estimate_for_meter() {
        let mut accounting = TokenAccounting::default();
        accounting.record_request_estimate(10_000, 128_000);
        assert_eq!(accounting.context_used_tokens(), 10_000);
        assert!(accounting.context_is_estimated());

        accounting.record_provider_usage(&TokenUsage {
            prompt_tokens: 9_432,
            completion_tokens: 120,
            total_tokens: 9_552,
            ..Default::default()
        });
        assert_eq!(accounting.context_used_tokens(), 9_432);
        assert!(!accounting.context_is_estimated());
        assert_eq!(accounting.provider_usage.total_tokens, 9_552);
    }

    #[test]
    fn merge_sums_usage_but_keeps_peak_context() {
        let mut turn = TokenAccounting::default();
        turn.record_request_estimate(8_000, 128_000);
        turn.record_provider_usage(&TokenUsage {
            prompt_tokens: 7_000,
            completion_tokens: 200,
            total_tokens: 7_200,
            ..Default::default()
        });
        let mut compact = TokenAccounting::default();
        compact.record_request_estimate(12_000, 64_000);
        compact.record_provider_usage(&TokenUsage {
            prompt_tokens: 11_500,
            completion_tokens: 300,
            total_tokens: 11_800,
            ..Default::default()
        });

        turn.merge(&compact);

        assert_eq!(turn.provider_usage.total_tokens, 19_000);
        assert_eq!(turn.context_used_tokens(), 11_500);
        // Merged limit is the active request limit (max), never the narrower
        // maintenance window.
        assert_eq!(turn.context_limit_tokens, Some(128_000));
    }

    #[test]
    fn merge_takes_max_context_limit() {
        // Some wins over None on either side.
        let mut a = TokenAccounting {
            context_limit_tokens: Some(64_000),
            ..Default::default()
        };
        a.merge(&TokenAccounting::default());
        assert_eq!(a.context_limit_tokens, Some(64_000));

        let mut b = TokenAccounting::default();
        b.merge(&TokenAccounting {
            context_limit_tokens: Some(32_000),
            ..Default::default()
        });
        assert_eq!(b.context_limit_tokens, Some(32_000));

        // Both Some: larger window wins regardless of merge order.
        let mut lo = TokenAccounting {
            context_limit_tokens: Some(64_000),
            ..Default::default()
        };
        lo.merge(&TokenAccounting {
            context_limit_tokens: Some(128_000),
            ..Default::default()
        });
        assert_eq!(lo.context_limit_tokens, Some(128_000));

        let mut hi = TokenAccounting {
            context_limit_tokens: Some(128_000),
            ..Default::default()
        };
        hi.merge(&TokenAccounting {
            context_limit_tokens: Some(64_000),
            ..Default::default()
        });
        assert_eq!(hi.context_limit_tokens, Some(128_000));

        // None + None stays None.
        let mut none = TokenAccounting::default();
        none.merge(&TokenAccounting::default());
        assert_eq!(none.context_limit_tokens, None);
    }
}
