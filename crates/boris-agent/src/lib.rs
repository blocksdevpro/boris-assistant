//! LLM tool-calling agent used by Boris.
//!
//! Pure library: async HTTP + context + optional tools → [`AgentOutcome`].
//! The host (pipeline) owns the Tokio runtime boundary, audio I/O, and speech.
//!
//! Personal context ([`memory`]) is a durable model of the human user, injected
//! into the system prompt and updated actively after turns.
//!
//! Tool execution always goes through [`runtime::ToolRuntime`] (policy, timeout,
//! audit, HITL). Tool bodies stay observation-only and dumb.

pub mod client;
pub mod context;
pub mod engine;
pub mod error;
pub mod memory;
pub mod observe;
pub mod outcome;
pub mod runtime;
pub mod session;
pub mod tool;
pub mod tools;

pub use client::{LlmClient, OpenRouterClient};
pub use context::{Context, Message, Role};
pub use engine::AgentEngine;
pub use error::{AgentError, AgentErrorKind, LlmError, LlmErrorKind};
pub use memory::{FactCategory, ProfileStore, UserFact, UserProfile, PERSONAL_CONTEXT_MAX_CHARS};
pub use observe::TurnReport;
pub use outcome::AgentOutcome;
pub use runtime::{
    default_user_read_roots, PendingToolCall, SandboxConfig, ToolRuntime, JsonlAuditSink,
    NullAuditSink, NetworkPolicy, ShellPolicy,
};
pub use session::{generate_session_id, SessionId, SessionMeta, SessionStatus, SessionStore};
pub use tool::{Permission, Tool, ToolError, ToolMeta, ToolRisk};
pub use tools::{
    builtin_tools, fs_tools, os_tools, register_builtin_tools, register_builtin_tools_with_options,
    shell_tools, web_tools, BuiltinToolPaths,
};
