//! High-precision, zero-cost extraction from the user utterance.

use crate::memory::profile::{now_ms, FactCategory, UserFact};

use super::delta::ProfileDelta;

/// High-precision, zero-cost extraction from the user utterance.
pub fn extract_heuristic(user_text: &str) -> ProfileDelta {
    let mut delta = ProfileDelta::default();
    let raw = user_text.trim();
    if raw.is_empty() {
        return delta;
    }
    let lower = raw.to_ascii_lowercase();

    // Explicit deletion requests are lifecycle events, not physical removal.
    if [
        "forget everything about me",
        "forget everything you know about me",
        "clear all my memory",
        "delete all my personal data",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
    {
        delta.forget_all = true;
        return delta;
    }
    if ["forget my name", "delete my name", "remove my name"]
        .iter()
        .any(|phrase| lower.contains(phrase))
    {
        delta.forget_preferred_name = true;
    }
    if let Some(query) = [
        "forget that ",
        "forget about ",
        "please forget ",
        "remove from memory ",
        "delete from memory ",
        "i no longer ",
    ]
    .iter()
    .find_map(|prefix| capture_after(&lower, raw, &[*prefix]))
    {
        if query.len() >= 3 {
            delta.facts_remove_query.push(query);
        }
    }

    let expiry = inferred_expiry_ms(&lower);

    // Name patterns.
    if let Some(name) = capture_after(&lower, raw, &["my name is ", "i am ", "i'm ", "im "]) {
        // Avoid "I am tired" etc. — only accept short name-like captures.
        if looks_like_name(&name) {
            delta.preferred_name = Some(name);
        }
    }
    if let Some(name) = capture_after(&lower, raw, &["call me ", "please call me "]) {
        if looks_like_name(&name) {
            delta.preferred_name = Some(name.clone());
            delta.address_as = Some(name);
        }
    }

    // Preferences.
    for prefix in [
        "i prefer ",
        "i like ",
        "i love ",
        "i hate ",
        "i don't like ",
        "i do not like ",
        "please don't ",
        "please do not ",
        "never call me ",
        "don't call me ",
    ] {
        if let Some(rest) = capture_after(&lower, raw, &[prefix]) {
            if rest.len() >= 3 && rest.len() <= 120 {
                delta.preferences_add.push(rest);
            }
        }
    }

    // Work / project.
    for prefix in [
        "i work on ",
        "i'm working on ",
        "i am working on ",
        "i build ",
        "i'm building ",
        "my project is ",
        "my project ",
    ] {
        if let Some(rest) = capture_after(&lower, raw, &[prefix]) {
            if rest.len() >= 3 {
                delta.facts_add.push(
                    UserFact::new(
                        format!("Works on / building: {rest}"),
                        FactCategory::Project,
                        "heuristic",
                    )
                    .with_expiry_ms(expiry),
                );
                delta.ongoing_add.push(rest);
            }
        }
    }

    // Role / identity.
    for prefix in ["i'm a ", "i am a ", "i'm an ", "i am an "] {
        if let Some(rest) = capture_after(&lower, raw, &[prefix]) {
            if looks_like_role(&rest) {
                delta.facts_add.push(
                    UserFact::new(format!("Is a {rest}"), FactCategory::Identity, "heuristic")
                        .with_expiry_ms(expiry),
                );
            }
        }
    }

    delta
}

fn inferred_expiry_ms(lower: &str) -> Option<u64> {
    const DAY_MS: u64 = 24 * 60 * 60 * 1_000;
    let days = if lower.contains("today") || lower.contains("for the day") {
        1
    } else if lower.contains("this week") {
        7
    } else if lower.contains("this month") || lower.contains("for now") {
        30
    } else if lower.contains("temporarily") || lower.contains("temporary") {
        14
    } else {
        return None;
    };
    Some(now_ms().saturating_add(days * DAY_MS))
}

pub(super) fn capture_after(lower: &str, original: &str, prefixes: &[&str]) -> Option<String> {
    for p in prefixes {
        if let Some(idx) = lower.find(p) {
            let start = idx + p.len();
            // Map byte index carefully — prefixes are ascii.
            let rest = original.get(start..)?.trim();
            let rest = rest
                .split(['.', '!', '?', ',', ';'])
                .next()
                .unwrap_or(rest)
                .trim();
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

pub(super) fn looks_like_name(s: &str) -> bool {
    let s = s.trim();
    let words: Vec<_> = s.split_whitespace().collect();
    if words.is_empty() || words.len() > 3 {
        return false;
    }
    if s.len() < 2 || s.len() > 40 {
        return false;
    }
    // Reject common false positives for "i am …"
    let lower = s.to_ascii_lowercase();
    const BAD: &[&str] = &[
        "tired", "fine", "good", "ok", "okay", "here", "back", "ready", "done", "busy", "hungry",
        "sorry", "sure", "confused", "lost", "home", "going", "trying",
    ];
    if words.len() == 1 && BAD.contains(&lower.as_str()) {
        return false;
    }
    words.iter().all(|w| {
        w.chars()
            .all(|c| c.is_alphabetic() || c == '-' || c == '\'')
    })
}

pub(super) fn looks_like_role(s: &str) -> bool {
    let s = s.trim();
    s.len() >= 3 && s.len() <= 60 && !s.contains("http")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristic_name() {
        let d = extract_heuristic("Hey, my name is Uttam");
        assert_eq!(d.preferred_name.as_deref(), Some("Uttam"));
    }

    #[test]
    fn heuristic_skips_i_am_tired() {
        let d = extract_heuristic("I am tired");
        assert!(d.preferred_name.is_none());
    }

    #[test]
    fn heuristic_prefer() {
        let d = extract_heuristic("I prefer short answers please");
        assert!(!d.preferences_add.is_empty());
    }

    #[test]
    fn looks_like_name_accepts_short_names() {
        assert!(looks_like_name("Ada"));
        assert!(looks_like_name("Mary-Jane"));
        assert!(!looks_like_name("tired"));
        assert!(!looks_like_name("one two three four"));
    }

    #[test]
    fn heuristic_extracts_forgetting_and_temporary_expiry() {
        let forget = extract_heuristic("Please forget that I live in Paris");
        assert_eq!(forget.facts_remove_query, vec!["I live in Paris"]);

        let temporary = extract_heuristic("I'm working on a launch this week");
        assert!(temporary.facts_add[0].expires_at_ms.is_some());
    }
}
