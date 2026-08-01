//! Personal context: durable profile + active extraction.
//!
//! Boris keeps a compact, high-signal model of the human user (name, prefs,
//! projects, facts) separate from the rolling session transcript. That model
//! is injected into the system prompt every turn and updated actively via:
//! - heuristics on user speech
//! - model tools (`save_user_fact`, …)
//! - optional side-channel LLM extraction after turns

pub mod extract;
pub mod profile;
pub mod store;

pub use extract::{
    extract_heuristic, extract_with_llm, should_llm_extract, ProfileDelta,
};
pub use profile::{FactCategory, UserFact, UserProfile};
pub use store::ProfileStore;

/// Default max size of the injected `<personal_context>` block.
pub const PERSONAL_CONTEXT_MAX_CHARS: usize = 900;
