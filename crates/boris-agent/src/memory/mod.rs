//! Personal context + long-term markdown memory.
//!
//! - **Profile** (`profile.json`): compact who-is-the-human facts for every turn.
//! - **Long-term** (global `MEMORY.md` + per-session `sessions/…/memory.md`): curated global + chat-local logs.
//! - **Extract**: heuristics + optional side-channel LLM → [`ProfileDelta`].
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`profile`]   | [`UserProfile`] / facts / prompt block |
//! | [`store`]     | load/save `profile.json` |
//! | [`extract`]   | heuristics + LLM personal extract |
//! | [`long_term`] | global `MEMORY.md` + session `memory.md` + search |

pub mod extract;
pub mod index;
pub mod long_term;
pub mod profile;
pub mod store;

pub use extract::{extract_heuristic, extract_with_llm, should_llm_extract, ProfileDelta};
pub use index::{IndexHit, MemoryIndex};
pub use long_term::{LongTermMemory, MemoryHit, SessionMemoryTarget};
pub use profile::{FactCategory, UserFact, UserProfile};
pub use store::ProfileStore;

/// Default max size of the injected `<personal_context>` block.
pub const PERSONAL_CONTEXT_MAX_CHARS: usize = 900;
