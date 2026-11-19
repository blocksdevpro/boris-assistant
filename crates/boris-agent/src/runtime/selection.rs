//! Follow-up detection for request routing.
//!
//! Tool listing no longer depends on the request text: every builtin is listed
//! on every request (see [`super::listing`]). This module only recognizes
//! short follow-ups ("continue", "try again") so routing can classify them by
//! the preceding objective instead of the two-word utterance.

/// Short follow-ups inherit the preceding objective's traits, not old greetings.
pub(crate) fn is_task_followup(text: &str) -> bool {
    crate::context::followup::followup_kind(text).is_some()
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
