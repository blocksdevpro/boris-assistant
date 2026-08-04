//! Personal context + long-term markdown memory.
//!
//! - **Profile** (`profile.json`): compact who-is-the-human facts for every turn.
//! - **Long-term** (`MEMORY.md` + `sessions/*.md`): Grok-lite cross-session knowledge.

pub mod extract;
pub mod long_term;
pub mod profile;
pub mod store;

pub use extract::{extract_heuristic, extract_with_llm, should_llm_extract, ProfileDelta};
pub use long_term::{LongTermMemory, MemoryHit};
pub use profile::{FactCategory, UserFact, UserProfile};
pub use store::ProfileStore;

/// Default max size of the injected `<personal_context>` block.
pub const PERSONAL_CONTEXT_MAX_CHARS: usize = 900;
