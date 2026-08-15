//! LLM provider plane for Boris.
//!
//! Keeps HTTP/provider concerns out of the agent harness (`boris-agent`).
//!
//! # Modules
//!
//! | Module | Role |
//! |--------|------|
//! | [`client`] | [`LlmClient`] trait (provider-agnostic) |
//! | [`error`] | [`LlmError`] / [`LlmErrorKind`] |
//! | [`message`] | Content extraction + message normalization |
//! | [`model_pref`] | `model@provider` + provider list parsing |
//! | [`usage`] | Token / cache usage |
//! | [`providers`] | Concrete backends (OpenRouter) |
//! | [`stream`] | Optional in-process event channel (not used by voice loop) |
//!
//! # Host imports
//!
//! Prefer crate-root re-exports (also re-exported from `boris-agent`):
//!
//! ```ignore
//! use boris_ai::{LlmClient, OpenRouterClient, LlmError, parse_provider_list};
//! ```

mod client;
pub mod error;
mod message;
mod model_pref;
mod providers;
mod request;
pub mod stream;
mod usage;

pub use client::LlmClient;
pub use error::{LlmError, LlmErrorKind};
pub use model_pref::{parse_provider_list, split_model_and_provider};
pub use providers::OpenRouterClient;
pub use request::{CompleteOptions, RequestStage};
pub use stream::LlmStreamEvent;
pub use usage::TokenUsage;

// OpenRouter timeout / base-url / reasoning (hosts may tune with builders).
pub use providers::openrouter::{
    ReasoningConfig, ReasoningEffort, DEFAULT_BASE_URL, DEFAULT_CONNECT_TIMEOUT,
    DEFAULT_MAX_TOKENS, DEFAULT_MODEL, DEFAULT_TIMEOUT,
};
