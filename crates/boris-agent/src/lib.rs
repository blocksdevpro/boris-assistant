//! LLM tool-calling agent harness used by Boris.
//!
//! Layered like tau's `agent` crate:
//! - [`loop_`] — pure ReAct loop (complete + tools + events)
//! - [`agent::Agent`] — stateful facade (memory, HITL, session helpers)
//! - [`runtime`] — policy / timeout / audit / confirmation
//!
//! Provider HTTP lives in `boris-ai` and is re-exported here for hosts.
//!
//! Tool bodies stay observation-only; speech is always [`AgentOutcome`].

pub mod agent;
pub mod client;
pub mod context;
pub mod error;
pub mod loop_;
pub mod memory;
pub mod observe;
pub mod outcome;
pub mod runtime;
pub mod session;
pub mod stats;
pub mod tool;
pub mod tools;
pub mod types;

// Re-export AI plane so hosts keep `boris_agent::{LlmClient, OpenRouterClient}`.
pub use boris_ai::{LlmClient, LlmError, LlmErrorKind, OpenRouterClient};

pub use agent::{Agent, AgentOptions};
pub use context::{Context, Message, Role};
pub use error::{AgentError, AgentErrorKind};
pub use loop_::{agent_loop, resume_pending_tool, LoopState};
pub use memory::{FactCategory, ProfileStore, UserFact, UserProfile, PERSONAL_CONTEXT_MAX_CHARS};
pub use observe::TurnReport;
pub use outcome::AgentOutcome;
pub use runtime::{
    default_user_read_roots, PendingToolCall, SandboxConfig, ToolRuntime, JsonlAuditSink,
    NullAuditSink, NetworkPolicy, ShellPolicy,
};
pub use session::{generate_session_id, SessionId, SessionMeta, SessionStatus, SessionStore};
pub use stats::AgentStats;
pub use tool::{Permission, Tool, ToolError, ToolMeta, ToolRisk};
pub use tools::{
    bash_tools, builtin_tools, fs_tools, os_tools, register_builtin_tools,
    register_builtin_tools_with_options, shell_tools, web_tools, BuiltinToolPaths,
};
pub use types::{AgentEvent, AgentLoopConfig, LoopResult, DEFAULT_MAX_TOOL_ROUNDS};

// ── Temporary aliases (migration window) ─────────────────────────────────────

/// Deprecated name for [`Agent`]. Prefer `Agent`.
#[deprecated(note = "renamed to Agent")]
pub type AgentEngine = Agent;
