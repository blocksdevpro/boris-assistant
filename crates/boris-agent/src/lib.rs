//! LLM tool-calling agent harness used by Boris.
//!
//! Layers (Grok-inspired, voice-sized):
//! - [`loop_`] — pure ReAct loop (complete + tools + events; parallel safe batches)
//! - [`agent::Agent`] — stateful facade (memory, HITL, session, prompt profile)
//! - [`runtime`] — policy / timeout / audit / confirmation / reminders
//! - [`tool_context`] — per-call cwd / cancel / session
//! - [`prompt_profile`] — structured system prompt sections
//! - [`capability`] — tool kinds + capability presets
//!
//! Provider HTTP lives in `boris-ai` and is re-exported here for hosts.
//!
//! Tool bodies stay observation-only; speech is always [`AgentOutcome`].

pub mod agent;
pub mod capability;
pub mod client;
pub mod context;
pub mod error;
pub mod finish_gate;
pub mod loop_;
pub mod memory;
pub mod observe;
pub mod outcome;
pub mod prompt_profile;
pub mod reminder;
pub mod routing;
pub mod runtime;
pub mod session;
pub mod skills;
pub mod stats;
pub mod tool;
pub mod tool_context;
pub mod tools;
pub mod types;

// Re-export AI plane so hosts keep `boris_agent::{LlmClient, OpenRouterClient}`.
pub use boris_ai::{
    parse_provider_list, split_model_and_provider, LlmClient, LlmError, LlmErrorKind,
    OpenRouterClient, TokenUsage,
};

pub use agent::{Agent, AgentOptions};
pub use capability::{filter_tools_for_preset, CapabilityPreset};
pub use routing::{classify_route, RouteMode, RoutingClient};
pub use context::{Context, Message, Role};
pub use error::{AgentError, AgentErrorKind};
pub use loop_::{agent_loop, resume_pending_tool, LoopState};
pub use memory::{
    FactCategory, LongTermMemory, MemoryHit, ProfileStore, UserFact, UserProfile,
    PERSONAL_CONTEXT_MAX_CHARS,
};
pub use observe::TurnReport;
pub use outcome::AgentOutcome;
pub use prompt_profile::{PromptContext, UserInfo};
pub use runtime::{
    default_user_read_roots, PendingToolCall, SandboxConfig, ToolRuntime, JsonlAuditSink,
    NullAuditSink, NetworkPolicy, ShellPolicy,
};
pub use session::{generate_session_id, SessionId, SessionMeta, SessionStatus, SessionStore};
pub use skills::{
    ensure_default_skills, format_skills_catalog, load_skills, load_skill_body, user_skills_dir,
    LoadedSkills, Skill, SkillSource,
};
pub use stats::AgentStats;
pub use tool::{
    Permission, Tool, ToolError, ToolKind, ToolMeta, ToolRisk, MAX_SKILL_RESULT_CHARS,
    MAX_TOOL_RESULT_CHARS,
};
pub use tool_context::ToolCallContext;
pub use tools::{
    bash_tools, builtin_tools, fs_tools, os_tools, register_builtin_tools,
    register_builtin_tools_with_options, register_builtin_tools_with_preset, shell_tools,
    web_tools, BuiltinToolPaths,
};
pub use types::{
    AgentEvent, AgentLoopConfig, LoopResult, DEFAULT_MAX_TOOL_ROUNDS, SKILLS_MAX_TOOL_ROUNDS,
};

// ── Temporary aliases (migration window) ─────────────────────────────────────

/// Deprecated name for [`Agent`]. Prefer `Agent`.
#[deprecated(note = "renamed to Agent")]
pub type AgentEngine = Agent;
