//! Pure profile helpers (normalize, ids, similarity) — no I/O.

use super::types::FactCategory;

/// Wall-clock milliseconds since UNIX epoch (0 if clock is broken).
pub fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(super) fn default_salience(cat: FactCategory) -> u8 {
    match cat {
        FactCategory::Identity => 9,
        FactCategory::Preference => 7,
        FactCategory::Project => 6,
        FactCategory::Relationship => 6,
        FactCategory::Habit => 5,
        FactCategory::Other => 4,
    }
}

/// Collapse whitespace, strip quotes, cap length for the voice prompt budget.
pub(super) fn normalize_fact_text(s: String) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim().trim_matches(|c: char| c == '"' || c == '\'');
    // Cap individual fact length for voice budget.
    if s.chars().count() > 160 {
        s.chars().take(157).collect::<String>() + "…"
    } else {
        s.to_string()
    }
}

/// Keep short display names only (≤3 tokens, ≤40 chars).
pub(super) fn clean_name(s: String) -> String {
    let s = s
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == '.' || c == '!');
    // First token / short name only.
    let s = s.split_whitespace().take(3).collect::<Vec<_>>().join(" ");
    if s.chars().count() > 40 {
        s.chars().take(40).collect()
    } else {
        s
    }
}

/// Stable-ish id from normalized lowercase text (FNV-1a, no extra deps).
pub(super) fn fact_id(text: &str) -> String {
    let key = text.to_ascii_lowercase();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("f{h:016x}")
}

/// Near-duplicate detector for merge-on-refresh.
pub(super) fn similar_fact(a: &str, b: &str) -> bool {
    let a = a.to_ascii_lowercase();
    let b = b.to_ascii_lowercase();
    if a == b {
        return true;
    }
    // Containment for short updates ("likes rust" vs "likes rust and go").
    if a.len() >= 12 && b.len() >= 12 && (a.contains(&b) || b.contains(&a)) {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_ws_and_caps() {
        assert_eq!(normalize_fact_text("  hello   world  ".into()), "hello world");
        let long = "x".repeat(200);
        let n = normalize_fact_text(long);
        assert!(n.chars().count() <= 160);
        assert!(n.ends_with('…'));
    }

    #[test]
    fn clean_name_takes_few_tokens() {
        assert_eq!(clean_name("  Ada Lovelace, Esq.  ".into()), "Ada Lovelace, Esq");
        assert_eq!(clean_name("\"Sam\"".into()), "Sam");
    }

    #[test]
    fn fact_id_stable() {
        assert_eq!(fact_id("Works on Boris"), fact_id("Works on Boris"));
        assert_ne!(fact_id("a"), fact_id("b"));
    }

    #[test]
    fn similar_fact_equality_and_containment() {
        assert!(similar_fact("likes rust", "likes rust"));
        assert!(similar_fact("likes rust and go", "likes rust and go tools"));
        assert!(!similar_fact("a", "b"));
        assert!(!similar_fact("short", "other"));
    }

    #[test]
    fn salience_by_category() {
        assert_eq!(default_salience(FactCategory::Identity), 9);
        assert_eq!(default_salience(FactCategory::Other), 4);
    }
}
