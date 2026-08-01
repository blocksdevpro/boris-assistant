//! LLM tool-calling agent used by Boris.
//!
//! Pure library: HTTP + context + optional tools → [`AgentOutcome`].
//! The assistant binary owns threads, channels, and speech I/O.

pub mod client;
pub mod context;
pub mod engine;
pub mod error;
pub mod observe;
pub mod outcome;
pub mod tool;

pub use client::{LlmClient, OpenRouterClient};
pub use engine::AgentEngine;
pub use error::{AgentError, AgentErrorKind, LlmError, LlmErrorKind};
pub use observe::TurnReport;
pub use outcome::AgentOutcome;
pub use tool::{Tool, ToolError};
