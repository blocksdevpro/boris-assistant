//! Structured personal context about the human user.
//!
//! Designed for a voice assistant: compact, high-signal, durable facts — not
//! a dump of the full chat log. Updated actively by heuristics, tools, and
//! optional post-turn LLM extraction.
//!
//! # On-disk format
//!
//! Hosts persist via [`crate::memory::ProfileStore`] as pretty JSON
//! (`profile.json`). Field names and [`UserProfile::version`] are part of the
//! wire contract — do not rename without a migration.
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`types`]   | [`FactCategory`], [`UserFact`], [`UserProfile`] + mutators |
//! | [`helpers`] | pure normalize / id / similarity |
//! | [`render`]  | `<personal_context>` prompt block |

mod helpers;
mod render;
mod types;

pub use helpers::now_ms;
pub use types::{FactCategory, UserFact, UserProfile};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_fact_dedupes() {
        let mut p = UserProfile::default();
        p.add_or_refresh_fact(UserFact::new(
            "Works on Boris in Rust",
            FactCategory::Project,
            "test",
        ));
        p.add_or_refresh_fact(UserFact::new(
            "Works on Boris in Rust",
            FactCategory::Project,
            "test",
        ));
        assert_eq!(p.facts.len(), 1);
    }

    #[test]
    fn category_parse_aliases() {
        assert_eq!(FactCategory::parse("name"), FactCategory::Identity);
        assert_eq!(FactCategory::parse("likes"), FactCategory::Preference);
        assert_eq!(FactCategory::parse("nope"), FactCategory::Other);
    }
}
