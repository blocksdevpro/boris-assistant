//! LLM tool-calling agent harness used by Boris.
//!
//! # Architecture (read this first)
//!
//! ```text
//! Host (pipeline/desktop)
//!   â””â”€ Agent facade          agent/        memory, HITL, session, prompt profile
//!        â””â”€ agent_loop       loop_         pure ReAct: complete â†’ tools â†’ events
//!             â””â”€ ToolRuntime runtime/      policy, timeout, audit, confirmation
//!                  â””â”€ dyn Tool             tools/* + tool.rs
//! ```
//!
//! | Layer | Module(s) | Responsibility |
//! |-------|-----------|----------------|
//! | Loop | [`loop_`], [`finish_gate`], [`types`] | ReAct complete + tool batches |
//! | Facade | [`agent`], [`outcome`], [`observe`], [`stats`] | Stateful host API |
//! | Runtime | [`runtime`] | Policy / timeout / audit / HITL |
//! | Tools | [`tools`], [`tool`], [`tool_context`], [`capability`] | Observation-only tools |
//! | Memory | [`memory`] | Profile + long-term facts |
//! | Session | [`session`] | Persist / transcript |
//! | Skills | [`skills`] | Playbooks |
//! | Routing | [`routing`], [`prompt_profile`], [`reminder`] | Prompt helpers |
//!
//! Provider HTTP lives in `boris-ai` and is re-exported here for hosts.
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
pub use routing::{classify_route, RouteMode, RoutingClient};
pub use runtime::{
    default_user_read_roots, ActivationSet, JsonlAuditSink, ListToolsContext, NetworkPolicy,
    NullAuditSink, PendingToolCall, ProgressEvent, SandboxConfig, ShellPolicy, ToolRuntime,
    ToolRuntimeFeatures,
};
pub use session::{generate_session_id, SessionId, SessionMeta, SessionStatus, SessionStore};
pub use skills::{
    ensure_default_skills, format_skills_catalog, load_skill_body, load_skills, user_skills_dir,
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

// â”€â”€ Temporary aliases (migration window) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// Deprecated name for [`Agent`]. Prefer `Agent`.
#[deprecated(note = "renamed to Agent")]
pub type AgentEngine = Agent;
