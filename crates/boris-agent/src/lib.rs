//! LLM tool-calling agent used by Boris.
//!
//! Pure library: HTTP + context + optional tools → [`AgentOutcome`].
//! The assistant binary owns threads, channels, and speech I/O.

pub mod client;
pub mod context;
pub mod engine;
pub mod error;
pub mod outcome;
pub mod tool;

pub use client::{LlmClient, OpenRouterClient};
pub use engine::AgentEngine;
pub use error::{AgentError, LlmError};
pub use outcome::AgentOutcome;
pub use tool::{Tool, ToolError};
