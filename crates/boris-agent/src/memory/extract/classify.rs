//! When a turn is worth an LLM extract pass.

/// Whether this turn is worth an LLM extract (beyond heuristics).
pub fn should_llm_extract(
    user_text: &str,
    tools_used: &[String],
    turns_seen: u64,
    heuristic_nonempty: bool,
) -> bool {
    let t = user_text.trim();
    if t.chars().count() < 12 {
        return false;
    }
    // Pure time/date questions — skip.
    let lower = t.to_ascii_lowercase();
    if is_ephemeral_query(&lower) {
        return false;
    }
    // Explicit memory tools already ran — still extract structured profile.
    if tools_used
        .iter()
        .any(|n| n == "remember_note" || n == "save_user_fact" || n == "update_user_profile")
    {
        return true;
    }
    // Heuristics already got signal — optional LLM polish only every few turns.
    if heuristic_nonempty {
        return turns_seen % 2 == 0;
    }
    // Personal language markers.
    let personal = [
        " my ",
        " i ",
        " i'm ",
        " i am ",
        " me ",
        " mine ",
        " wife ",
        " husband ",
        " kids ",
        " job ",
        " work ",
        " project ",
        " prefer ",
        " always ",
        " never ",
    ];
    let padded = format!(" {lower} ");
    let score = personal.iter().filter(|p| padded.contains(*p)).count();
    if score >= 2 && t.chars().count() >= 20 {
        return true;
    }
    // Slow cadence for ambient learning.
    turns_seen > 0 && turns_seen % 4 == 0 && t.chars().count() >= 24
}

pub(super) fn is_ephemeral_query(lower: &str) -> bool {
    let ephemeral = [
        "what time",
        "what's the time",
        "whats the time",
        "what date",
        "what's the date",
        "whats the date",
        "what day is",
        "how are you",
        "hello",
        "hey boris",
        "hi boris",
        "good morning",
        "good night",
    ];
    ephemeral.iter().any(|p| lower.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_short_and_ephemeral() {
        assert!(!should_llm_extract("hi", &[], 4, false));
        assert!(!should_llm_extract("what time is it right now", &[], 4, false));
    }

    #[test]
    fn memory_tools_force_true() {
        assert!(should_llm_extract(
            "please remember that for later on",
            &["remember_note".into()],
            1,
            false
        ));
    }

    #[test]
    fn personal_markers_trigger() {
        assert!(should_llm_extract(
            "I always prefer short answers at my job",
            &[],
            1,
            false
        ));
    }
}
