//! LLM provider plane for Boris.
//!
//! Keeps HTTP/provider concerns out of the agent harness (`boris-agent`).
//! Architecture mirrors tau's `ai` crate at a smaller scale: client trait,
//! OpenRouter provider, and optional event-stream primitives.

pub mod client;
pub mod error;
pub mod providers;
pub mod stream;

pub use client::LlmClient;
pub use error::{LlmError, LlmErrorKind};
pub use providers::openrouter::{
    parse_provider_list, split_model_and_provider, TokenUsage,
};
pub use providers::OpenRouterClient;
pub use stream::{event_stream, EventStream, EventStreamSender};
