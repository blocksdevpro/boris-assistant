//! Tolerant detection of short task follow-ups ("continue", "did you finish?").
//!
//! Voice input arrives with fillers and STT noise ("Tss continue", "okay so
//! umm keep going boris"). A message is a follow-up when, after dropping
//! fillers, every remaining word belongs to a small follow-up vocabulary and
//! at least one core phrase is present. Anything with other substance
//! ("continue by searching for Rust news") is a new request. No LLM call.

/// What kind of follow-up a short message is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowupKind {
    /// Continue / resume / keep going / finish it / try again.
    Resume,
    /// A question about the earlier task ("did you finish the task?").
    Status,
    /// A bare affirmation ("yes", "yeah"); used for routing only.
    Affirm,
}

/// Fillers and STT noise tokens dropped before matching.
const FILLERS: &[&str] = &[
    "uh", "uhh", "um", "umm", "hmm", "hm", "mm", "mhm", "erm", "er", "ah", "oh", "okay", "ok", "k",
    "please", "pls", "boris", "just", "so", "now", "then", "well", "hey", "hi", "yo", "alright",
    "right", "tss", "tsk", "ts", "psst", "shh", "like", "actually", "also", "and", "but", "a",
    "an", "the",
];

/// Words allowed around a core phrase without making it a new request.
const VOCABULARY: &[&str] = &[
    "continue",
    "continuing",
    "resume",
    "resuming",
    "proceed",
    "finish",
    "finished",
    "done",
    "complete",
    "completed",
    "retry",
    "again",
    "try",
    "go",
    "on",
    "ahead",
    "keep",
    "going",
    "carry",
    "do",
    "it",
    "that",
    "this",
    "task",
    "work",
    "job",
    "you",
    "did",
    "are",
    "is",
    "was",
    "have",
    "has",
    "where",
    "were",
    "we",
    "left",
    "off",
    "from",
    "there",
    "pick",
    "up",
    "status",
    "whats",
    "what",
    "how",
    "far",
    "yes",
    "yeah",
    "yep",
    "yup",
    "sure",
    "with",
    "can",
    "could",
    "would",
    "will",
    "your",
    "my",
    "of",
    "yet",
];

const RESUME_PHRASES: &[&str] = &[
    "continue",
    "continuing",
    "resume",
    "resuming",
    "proceed",
    "keep going",
    "carry on",
    "go on",
    "go ahead",
    "finish",
    "try again",
    "retry",
    "do it",
    "pick up",
    "left off",
];

/// Only count as a status follow-up when the message is also a question.
const STATUS_PHRASES: &[&str] = &[
    "done",
    "finish",
    "finished",
    "complete",
    "completed",
    "status",
    "how far",
    "where were we",
    "left off",
];

const QUESTION_WORDS: &[&str] = &[
    "did", "are", "is", "was", "have", "has", "what", "whats", "how", "where",
];

const AFFIRMATIONS: &[&str] = &["yes", "yeah", "yep", "yup", "sure"];

fn normalized_words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .replace(['\u{2019}', '\''], "")
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .filter(|w| !FILLERS.contains(w))
        .map(str::to_owned)
        .collect()
}

fn contains_phrase(padded: &str, phrases: &[&str]) -> bool {
    phrases
        .iter()
        .any(|phrase| padded.contains(&format!(" {phrase} ")))
}

/// Classify a short message as a follow-up, or `None` for a new request.
pub fn followup_kind(text: &str) -> Option<FollowupKind> {
    let words = normalized_words(text);
    if words.is_empty() || words.len() > 12 {
        return None;
    }
    if !words.iter().all(|w| VOCABULARY.contains(&w.as_str())) {
        return None;
    }
    let padded = format!(" {} ", words.join(" "));
    // "did you finish?" asks about the task; "pick up where we left off" does not.
    let question = QUESTION_WORDS.contains(&words[0].as_str());
    if question && contains_phrase(&padded, STATUS_PHRASES) {
        return Some(FollowupKind::Status);
    }
    if contains_phrase(&padded, RESUME_PHRASES) {
        return Some(FollowupKind::Resume);
    }
    if words.iter().all(|w| AFFIRMATIONS.contains(&w.as_str())) {
        return Some(FollowupKind::Affirm);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_survives_fillers_and_stt_noise() {
        for text in [
            "continue",
            "Tss continue",
            "Uh, please continue!",
            "okay so umm keep going boris",
            "finish it please",
            "try again",
            "go ahead",
            "carry on with it",
            "can you continue the task",
            "pick up where we left off",
        ] {
            assert_eq!(followup_kind(text), Some(FollowupKind::Resume), "{text}");
        }
    }

    #[test]
    fn task_questions_are_status_followups() {
        for text in [
            "did you finish the task?",
            "Did you finish?",
            "are you done",
            "is it done yet",
            "what's the status",
            "where were we",
        ] {
            assert_ne!(followup_kind(text), None, "{text}");
        }
        assert_eq!(
            followup_kind("did you finish the task?"),
            Some(FollowupKind::Status)
        );
    }

    #[test]
    fn requests_with_other_substance_are_not_followups() {
        for text in [
            "continue by searching for Rust news",
            "yes, open my clipboard",
            "continue the essay about dogs",
            "done",
            "I'm done",
            "finish the report and email it to Sam",
            "",
        ] {
            assert_eq!(followup_kind(text), None, "{text}");
        }
    }

    #[test]
    fn bare_affirmation_is_routing_only() {
        assert_eq!(followup_kind("yeah"), Some(FollowupKind::Affirm));
        assert_eq!(followup_kind("yes please"), Some(FollowupKind::Affirm));
    }
}
