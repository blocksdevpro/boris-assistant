//! Voice yes/no interpretation for HITL tool confirmations.
//!
//! Pure string helpers — no audio or agent I/O. Used by [`super::outcome`] after
//! STT returns a freeform confirm answer.

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

/// Interpret freeform STT as yes/no. `None` = ambiguous.
///
/// Accepts natural variants ("yeah go ahead", "nope cancel that", "yes.") not only
/// bare yes/no.
pub(super) fn interpret_yes_no(text: &str) -> Option<bool> {
    let t = normalize_confirm_text(text);
    if t.is_empty() {
        return None;
    }
    let words: Vec<&str> = t.split_whitespace().collect();
    let head: String = words.iter().take(5).cloned().collect::<Vec<_>>().join(" ");

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
        if head == *p || head.starts_with(&format!("{p} ")) || t.contains(p) {
            return Some(false);
        }
    }
    for p in YES_PHRASES {
        if head == *p || head.starts_with(&format!("{p} ")) || t.contains(p) {
            return Some(true);
        }
    }

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
        "y",
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
        "right",
        "true",
        "uh huh",
        "mhmm",
        "mm hmm",
    ];
    const NO: &[&str] = &[
        "no", "nope", "nah", "cancel", "stop", "never", "n", "negative", "decline", "abort",
        "refuse", "reject", "denied", "pass", "skip",
    ];

    // First-word / head match.
    for n in NO {
        if head == *n || head.starts_with(&format!("{n} ")) {
            return Some(false);
        }
    }
    for y in YES {
        if head == *y || head.starts_with(&format!("{y} ")) {
            return Some(true);
        }
    }

    // Any clear token in the utterance (handles "uh yes bro", "mm no thanks").
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
    // Prefer deny if both appear ("yes no wait" → ambiguous → None, re-ask).
    match (saw_yes, saw_no) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
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
    }

    #[test]
    fn normalize_strips_punctuation() {
        assert_eq!(normalize_confirm_text("Yes!"), "yes");
        assert_eq!(normalize_confirm_text("  OK — go ahead. "), "ok go ahead");
    }
}
