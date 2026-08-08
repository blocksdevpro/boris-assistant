//! OpenRouter Chat Completions provider.
//!
//! Public surface re-exported through [`crate`] / `boris_agent` for hosts.

mod client;
mod complete;
mod request;
mod sse;

pub use client::{OpenRouterClient, DEFAULT_CONNECT_TIMEOUT, DEFAULT_TIMEOUT};

// Re-export prefs that historically lived next to the client so old
// `providers::openrouter::parse_provider_list` paths keep working.
pub use crate::model_pref::{parse_provider_list, split_model_and_provider};
pub use crate::usage::TokenUsage;
