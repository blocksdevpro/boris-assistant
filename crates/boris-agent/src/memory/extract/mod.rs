//! Active personal-context extraction.
//!
//! Two layers:
//! 1. **Heuristics** — free, high-precision patterns from the user utterance
//!    ("my name is…", "I prefer…", "call me…").
//! 2. **LLM extract** — side-channel JSON call (does not touch chat context)
//!    when the turn looks personal or on a cadence.
//!
//! Both produce a [`ProfileDelta`] applied onto [`crate::memory::UserProfile`].
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`delta`]     | [`ProfileDelta`] + apply |
//! | [`heuristic`] | zero-cost utterance patterns |
//! | [`classify`]  | [`should_llm_extract`] gate |
//! | [`llm`]       | side-channel LLM parse / call |

mod classify;
mod delta;
mod heuristic;
mod llm;

pub use classify::should_llm_extract;
pub use delta::ProfileDelta;
pub use heuristic::extract_heuristic;
pub use llm::extract_with_llm;
