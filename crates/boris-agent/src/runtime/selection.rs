//! Follow-up detection for request routing.
//!
//! Tool listing no longer depends on the request text: every builtin is listed
//! on every request (see [`super::listing`]). This module only recognizes
//! short follow-ups ("continue", "try again") so routing can classify them by
//! the preceding objective instead of the two-word utterance.

/// Short follow-ups inherit the preceding objective's traits, not old greetings.
pub(crate) fn is_task_followup(text: &str) -> bool {
    let words = text
        .to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let phrase = words
        .iter()
        .filter(|word| {
            !matches!(
                word.as_str(),
                "uh" | "um" | "please" | "boris" | "just" | "okay" | "ok"
            )
        })
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    matches!(
        phrase.as_str(),
        "continue"
            | "go on"
            | "keep going"
            | "carry on"
            | "resume"
            | "try again"
            | "retry"
            | "do it"
            | "yes"
            | "yeah"
            | "yep"
            | "proceed"
            | "finish it"
            | "continue with it"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn followups_are_whole_requests() {
        assert!(is_task_followup("Uh, please continue!"));
        assert!(is_task_followup("try again"));
        assert!(!is_task_followup("continue by searching for Rust news"));
        assert!(!is_task_followup("yes, open my clipboard"));
    }
}
