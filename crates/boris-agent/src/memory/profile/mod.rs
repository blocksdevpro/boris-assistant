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
pub use types::{FactCategory, FactStatus, UserFact, UserProfile};

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

    #[test]
    fn keyed_correction_supersedes_old_value() {
        let mut profile = UserProfile::default();
        profile.add_or_refresh_fact(
            UserFact::new("Home city: Paris", FactCategory::Identity, "test")
                .with_memory_key("home_city"),
        );
        profile.add_or_refresh_fact(
            UserFact::new("Home city: Berlin", FactCategory::Identity, "test")
                .with_memory_key("home_city"),
        );

        assert_eq!(profile.facts.len(), 2);
        assert_eq!(profile.facts[0].status, FactStatus::Superseded);
        assert_eq!(profile.facts[1].status, FactStatus::Active);
        assert_eq!(
            profile.facts[0].superseded_by,
            Some(profile.facts[1].id.clone())
        );
    }

    #[test]
    fn forgetting_tombstones_instead_of_deleting() {
        let mut profile = UserProfile::default();
        profile.add_or_refresh_fact(UserFact::new(
            "Lives in Paris",
            FactCategory::Identity,
            "test",
        ));
        assert_eq!(profile.forget_matching("I live in Paris"), 1);
        assert_eq!(profile.facts.len(), 1);
        assert_eq!(profile.facts[0].status, FactStatus::Forgotten);
        assert!(!profile.render_block(800).contains("Lives in Paris"));
    }

    #[test]
    fn expired_facts_are_excluded_from_active_context() {
        let mut profile = UserProfile::default();
        profile.add_or_refresh_fact(
            UserFact::new("Temporary city: Rome", FactCategory::Other, "test")
                .with_expiry_ms(Some(1)),
        );
        profile.expire_due_facts();
        assert_eq!(profile.facts[0].status, FactStatus::Expired);
        assert!(profile.render_block(800).is_empty());
    }
}
