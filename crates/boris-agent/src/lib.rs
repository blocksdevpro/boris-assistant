//! LLM tool-calling agent harness used by Boris.
//!
//! # Architecture (read this first)
//!
//! ```text
//! Host (pipeline/desktop)
//!   └─ Agent facade          agent/        memory, HITL, session, prompt profile
//!        └─ agent_loop       loop_/        pure ReAct: complete → tools → events
//!             └─ ToolRuntime runtime/      policy, timeout, audit, confirmation
//!                  └─ dyn Tool             tools/* + tool.rs
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
//!
//! # Public API surface
//!
//! **Prefer the crate-root re-exports** below for host integration
//! (`Agent`, `SandboxConfig`, `register_builtin_tools`, …). Nested modules
//! (`session::`, `tools::`, `runtime::`, …) are also `pub` because the pipeline
//! and tests need them; treat leaf internals as unstable unless re-exported
//! here. Module names `loop_` and `tool::trait_` use trailing underscores as an
//! intentional Rust keyword escape (not planned renames).
//!
//! # Security (summary)
//!
//! Hosts inject [`SandboxConfig`] (path roots, [`NetworkPolicy`], [`ShellPolicy`]).
//! HITL confirmation only skips the confirm UI — path/shell/network hard gates
//! still run after a user grant. See the crate README “Security model” section.

pub mod agent;
pub mod capability;
pub mod client;
pub mod context;
pub mod error;
pub mod finish_gate;
/// Pure ReAct loop (`loop` is a keyword → `loop_`).
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
pub mod speech_sanitize;
pub mod stats;
pub mod tool;
pub mod tool_context;
pub mod tools;
pub mod types;

// Re-export AI plane so hosts keep `boris_agent::{LlmClient, OpenRouterClient}`.
pub use boris_ai::{
    parse_provider_list, split_model_and_provider, LlmClient, LlmError, LlmErrorKind,
    OpenRouterClient, ReasoningConfig, ReasoningEffort, TokenUsage, DEFAULT_MAX_TOKENS,
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
pub use observe::{TurnOutcomeKind, TurnReport};
pub use outcome::AgentOutcome;
pub use prompt_profile::{PromptContext, UserInfo};
pub use routing::{classify_route, RouteMode, RoutingClient};
pub use runtime::{
    default_user_read_roots, ActivationSet, JsonlAuditSink, ListToolsContext, NetworkPolicy,
    NullAuditSink, PendingToolCall, ProgressEvent, SandboxConfig, ShellPolicy, ToolRuntime,
    ToolRuntimeFeatures,
};
pub use session::{
    generate_session_id, ArtifactIndex, ArtifactKind, ArtifactMeta, ArtifactStore, PresentRequest,
    PresentedArtifact, SessionId, SessionMeta, SessionStatus, SessionStore,
};
pub use skills::{
    ensure_default_skills, format_skills_catalog, load_skill_body, load_skills, user_skills_dir,
    LoadedSkills, Skill, SkillSource,
};
pub use speech_sanitize::{
    contains_tool_markup, is_markup_only_speech, strip_tool_markup, TOOL_PROTOCOL_REMINDER,
};
pub use stats::AgentStats;
pub use tool::{
    Permission, Tool, ToolError, ToolKind, ToolMeta, ToolRisk, MAX_SKILL_RESULT_CHARS,
    MAX_TOOL_RESULT_CHARS,
};
pub use tool_context::ToolCallContext;
pub use tools::{
    artifact_tools, artifact_tools_at, bash_tools, builtin_tools, fs_tools, os_tools,
    register_builtin_tools, register_builtin_tools_with_options,
    register_builtin_tools_with_preset, web_tools, BuiltinToolPaths,
};

/// Deprecated alias for [`bash_tools`].
#[allow(deprecated)]
#[deprecated(note = "use bash_tools")]
pub use tools::shell_tools;
pub use types::{
    AgentEvent, AgentLoopConfig, LoopResult, DEFAULT_MAX_TOOL_ROUNDS, SKILLS_MAX_TOOL_ROUNDS,
};

// ── Temporary aliases (migration window) ─────────────────────────────────────

/// Deprecated name for [`Agent`]. Prefer `Agent`.
#[deprecated(note = "renamed to Agent")]
pub type AgentEngine = Agent;
