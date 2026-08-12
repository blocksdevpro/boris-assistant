//! Concrete LLM providers.
//!
//! Today: OpenRouter. Add a sibling module (e.g. `openai`) and re-export from
//! crate root when a second host is needed — keep [`crate::LlmClient`] stable.

pub mod openrouter;

pub use openrouter::OpenRouterClient;
