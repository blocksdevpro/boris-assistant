//! OpenRouter Chat Completions provider.
//!
//! Public surface re-exported through [`crate`] / `boris_agent` for hosts.

mod client;
mod complete;
mod key;
mod reasoning;
mod request;
mod sse;

pub use client::{
    OpenRouterClient, DEFAULT_BASE_URL, DEFAULT_CONNECT_TIMEOUT, DEFAULT_CONTEXT_WINDOW_TOKENS,
    DEFAULT_MODEL, DEFAULT_TIMEOUT,
};
pub use key::{check_key, KeyCheck};
pub use reasoning::{ReasoningConfig, ReasoningEffort, DEFAULT_MAX_TOKENS};
