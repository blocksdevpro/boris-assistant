//! Boris's canonical local-first memory.
//!
//! - **Ledger** (`memory.sqlite`): source events, evidence-backed records,
//!   lifecycle, retrieval index, and extraction snapshot.
//! - **Long-term** (global `MEMORY.md` + per-session `sessions/…/memory.md`): curated global + chat-local logs.
//! - **Extract**: heuristics + optional side-channel LLM → [`ProfileDelta`].
//!
//! # Module layout
//!
//! The desktop runtime uses [`ledger`] exclusively. The Markdown/profile
//! modules below exist only to import old installations safely.
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`ledger`]    | canonical SQLite source of truth |
//! | [`profile`]   | extraction working-set types |
//! | [`store`]     | canonical extraction snapshot / legacy JSON reader |
//! | [`extract`]   | heuristics + LLM personal extract |
//! | [`long_term`] | global `MEMORY.md` + session `memory.md` + search |

pub mod extract;
pub mod index;
pub mod ledger;
pub mod long_term;
pub mod migration;
pub mod profile;
pub mod store;

pub use extract::{extract_heuristic, extract_with_llm, should_llm_extract, ProfileDelta};
pub use index::{IndexHit, MemoryIndex};
pub use ledger::{
    MemoryEvent, MemoryKind, MemoryPrivacy, MemoryRecord, MemoryScope, MemorySearchHit,
    MemoryStatus as StoredMemoryStatus, MemoryStore, NewMemory,
};
pub use long_term::{LongTermMemory, MemoryHit, SessionMemoryTarget};
pub use migration::{
    discover_legacy_memory, merge_legacy_profile, retire_legacy_files, LegacyMemoryPaths,
    LegacyMemorySource, LegacyMigrationPlan,
};
pub use profile::{now_ms, FactCategory, FactStatus, UserFact, UserProfile};
pub use store::ProfileStore;

/// Default max size of the injected `<personal_context>` block.
pub const PERSONAL_CONTEXT_MAX_CHARS: usize = 900;
