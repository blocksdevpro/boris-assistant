//! Voice yes/no interpretation for HITL tool confirmations.
//!
//! Pure string helpers — no audio or agent I/O. Used by [`super::outcome`] after
//! STT returns a freeform confirm answer.
//!
//! Matching is **whole-word / whole-phrase** only (no substring scan of the
//! full utterance), so short tokens like `"no"` do not fire inside `"know"`.

/// Normalize STT: lowercase, strip most punctuation, collapse spaces.
pub(super) fn normalize_confirm_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch.is_whitespace() || ch == '\'' {
            out.push(ch.to_ascii_lowercase());
        } else if ch == '-' {
            out.push(' ');
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// True when `phrase` appears as consecutive whole words in `words`.
fn words_contain_phrase(words: &[&str], phrase: &str) -> bool {
    let p: Vec<&str> = phrase.split_whitespace().collect();
    if p.is_empty() || p.len() > words.len() {
        return false;
    }
    words.windows(p.len()).any(|w| w == p.as_slice())
}

/// True when `words` starts with the phrase (as whole words).
fn head_is_phrase(words: &[&str], phrase: &str) -> bool {
    let p: Vec<&str> = phrase.split_whitespace().collect();
    if p.is_empty() || p.len() > words.len() {
        return false;
    }
    words[..p.len()] == p[..]
}

/// Interpret freeform STT as yes/no. `None` = ambiguous.
///
/// Accepts natural variants ("yeah go ahead", "nope cancel that", "yes.") not only
/// bare yes/no. Short tokens only match as whole words.
pub(super) fn interpret_yes_no(text: &str) -> Option<bool> {
    let t = normalize_confirm_text(text);
    if t.is_empty() {
        return None;
    }
    let words: Vec<&str> = t.split_whitespace().collect();
    // Multi-word phrases first (order matters for "do not" / "go ahead").
    const YES_PHRASES: &[&str] = &[
        "go ahead",
        "go for it",
        "do it",
        "do that",
        "sounds good",
        "all right",
        "alright",
        "for sure",
        "why not",
        "yes please",
        "yeah sure",
        "yep sure",
        "ok go",
        "okay go",
        "uh huh",
        "mm hmm",
    ];
    const NO_PHRASES: &[&str] = &[
        "do not",
        "don't",
        "no way",
        "no thanks",
        "not now",
        "hell no",
        "nope cancel",
        "cancel that",
        "stop that",
        "don't do",
        "do not do",
    ];

    for p in NO_PHRASES {
        if head_is_phrase(&words, p) || words_contain_phrase(&words, p) {
            return Some(false);
        }
    }
    for p in YES_PHRASES {
        if head_is_phrase(&words, p) || words_contain_phrase(&words, p) {
            return Some(true);
        }
    }

    // Single-token vocabulary (whole-word only).
    // Ultra-short tokens ("y", "n") only count as the *entire* utterance.
    const YES: &[&str] = &[
        "yes",
        "yeah",
        "yep",
        "yup",
        "sure",
        "ok",
        "okay",
        "please",
        "affirmative",
        "yea",
        "confirmed",
        "confirm",
        "approve",
        "approved",
        "fine",
        "proceed",
        "continue",
        "absolutely",
        "definitely",
        "correct",
        "true",
        "mhmm",
    ];
    const NO: &[&str] = &[
        "no", "nope", "nah", "cancel", "stop", "never", "negative", "decline", "abort",
        "refuse", "reject", "denied", "pass", "skip",
    ];
    const YES_ULTRA: &[&str] = &["y"];
    const NO_ULTRA: &[&str] = &["n"];

    if words.len() == 1 {
        let w = words[0];
        if YES_ULTRA.contains(&w) || YES.contains(&w) {
            return Some(true);
        }
        if NO_ULTRA.contains(&w) || NO.contains(&w) {
            return Some(false);
        }
        return None;
    }

    // Multi-word: scan all clear tokens first so "yes no wait" stays ambiguous.
    let mut saw_yes = false;
    let mut saw_no = false;
    for w in &words {
        if matches!(
            *w,
            "yes" | "yeah" | "yep" | "yup" | "yea" | "sure" | "ok" | "okay" | "affirmative"
        ) {
            saw_yes = true;
        }
        if matches!(
            *w,
            "no" | "nope" | "nah" | "cancel" | "stop" | "never" | "negative" | "abort" | "decline"
        ) {
            saw_no = true;
        }
    }
    match (saw_yes, saw_no) {
        (true, true) => return None,
        (true, false) => return Some(true),
        (false, true) => return Some(false),
        (false, false) => {}
    }

    // First-word match for remaining vocabulary (e.g. "approve the change").
    // Ultra-short tokens never apply as multi-word heads.
    if let Some(first) = words.first() {
        if NO.contains(first) {
            return Some(false);
        }
        if YES.contains(first) {
            return Some(true);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpret_yes_no_variants() {
        assert_eq!(interpret_yes_no("yes"), Some(true));
        assert_eq!(interpret_yes_no("Yes."), Some(true));
        assert_eq!(interpret_yes_no("yeah go ahead"), Some(true));
        assert_eq!(interpret_yes_no("sure thing bro"), Some(true));
        assert_eq!(interpret_yes_no("ok do it"), Some(true));
        assert_eq!(interpret_yes_no("no"), Some(false));
        assert_eq!(interpret_yes_no("Nope!"), Some(false));
        assert_eq!(interpret_yes_no("nah cancel that"), Some(false));
        assert_eq!(interpret_yes_no("don't"), Some(false));
        assert_eq!(interpret_yes_no("uh maybe later"), None);
        assert_eq!(interpret_yes_no(""), None);
        assert_eq!(interpret_yes_no("y"), Some(true));
        assert_eq!(interpret_yes_no("n"), Some(false));
    }

    #[test]
    fn no_false_positives_from_substrings() {
        // "no" must not match inside other words.
        assert_eq!(interpret_yes_no("know"), None);
        assert_eq!(interpret_yes_no("nothing"), None);
        assert_eq!(interpret_yes_no("another"), None);
        assert_eq!(interpret_yes_no("yesterday"), None);
        // "ok" must not match inside longer tokens (already whole-word).
        assert_eq!(interpret_yes_no("okra"), None);
        // Ambiguous mixed signal.
        assert_eq!(interpret_yes_no("yes no wait"), None);
        // "y" / "n" only as sole token.
        assert_eq!(interpret_yes_no("yep"), Some(true));
        assert_eq!(interpret_yes_no("y please"), None); // ultra-short not as head of multi-word
        assert_eq!(interpret_yes_no("n thanks"), None);
    }

    #[test]
    fn phrase_whole_words_only() {
        assert_eq!(interpret_yes_no("please go ahead"), Some(true));
        assert_eq!(interpret_yes_no("cancel that now"), Some(false));
        // Partial phrase fragments should not match.
        assert_eq!(interpret_yes_no("ahead"), None);
    }

    #[test]
    fn normalize_strips_punctuation() {
        assert_eq!(normalize_confirm_text("Yes!"), "yes");
        assert_eq!(normalize_confirm_text("  OK — go ahead. "), "ok go ahead");
    }
}
