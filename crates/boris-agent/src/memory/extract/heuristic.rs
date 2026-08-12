//! High-precision, zero-cost extraction from the user utterance.

use crate::memory::profile::{FactCategory, UserFact};

use super::delta::ProfileDelta;

/// High-precision, zero-cost extraction from the user utterance.
pub fn extract_heuristic(user_text: &str) -> ProfileDelta {
    let mut delta = ProfileDelta::default();
    let raw = user_text.trim();
    if raw.is_empty() {
        return delta;
    }
    let lower = raw.to_ascii_lowercase();

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
                delta.facts_add.push(UserFact::new(
                    format!("Works on / building: {rest}"),
                    FactCategory::Project,
                    "heuristic",
                ));
                delta.ongoing_add.push(rest);
            }
        }
    }

    // Role / identity.
    for prefix in ["i'm a ", "i am a ", "i'm an ", "i am an "] {
        if let Some(rest) = capture_after(&lower, raw, &[prefix]) {
            if looks_like_role(&rest) {
                delta.facts_add.push(UserFact::new(
                    format!("Is a {rest}"),
                    FactCategory::Identity,
                    "heuristic",
                ));
            }
        }
    }

    delta
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
}
