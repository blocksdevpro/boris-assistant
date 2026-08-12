//! OpenRouter Chat Completions provider.
//!
//! Public surface re-exported through [`crate`] / `boris_agent` for hosts.

mod client;
mod complete;
mod reasoning;
mod request;
mod sse;

pub use client::{
    OpenRouterClient, DEFAULT_BASE_URL, DEFAULT_CONNECT_TIMEOUT, DEFAULT_MODEL, DEFAULT_TIMEOUT,
};
pub use reasoning::{ReasoningConfig, ReasoningEffort, DEFAULT_MAX_TOKENS};
