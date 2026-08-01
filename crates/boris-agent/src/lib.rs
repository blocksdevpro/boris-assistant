//! LLM tool-calling agent used by Boris.
//!
//! Pure library: HTTP + context + optional tools → [`AgentOutcome`].
//! The assistant binary owns threads, channels, and speech I/O.
//!
//! Personal context ([`memory`]) is a durable model of the human user, injected
//! into the system prompt and updated actively after turns.

pub mod client;
pub mod context;
pub mod engine;
pub mod error;
pub mod memory;
pub mod observe;
pub mod outcome;
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
pub use session::{generate_session_id, SessionId, SessionMeta, SessionStatus, SessionStore};
pub use tool::{Tool, ToolError};
pub use tools::{
    builtin_tools, register_builtin_tools, register_builtin_tools_with_options, BuiltinToolPaths,
};
