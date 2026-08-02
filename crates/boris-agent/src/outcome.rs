//! What the agent decided after one user message (tool loop finished or paused).
//!
//! The host maps this into pipeline phases; `boris-agent` never emits speech
//! or touches the app event bus.

use crate::runtime::PendingToolCall;

/// What the agent decided after one user message (tool loop finished or paused).
#[derive(Debug, Clone, PartialEq)]
pub enum AgentOutcome {
    /// Final plain-text reply — host should synthesize and play this.
    Speak {
        text: String,
        /// When true, host should open freeform listen **without** another wake
        /// word (AwaitingReply), so the user can answer a question.
        expect_reply: bool,
    },
    /// Model returned no speakable content.
    Silent,
    /// Tool loop paused for HITL. Host should speak `text`, collect yes/no,
    /// then call [`crate::AgentEngine::resume_confirmation`].
    NeedsConfirmation {
        text: String,
        pending: PendingToolCall,
    },
}

impl AgentOutcome {
    /// Build a speak outcome; `expect_reply` is inferred from the text
    /// (short reply ending in `?` → true). Freeform answers, not yes/no only.
    pub fn speak(text: impl Into<String>) -> Self {
        let text = text.into();
        let expect_reply = looks_like_question(&text);
        Self::Speak { text, expect_reply }
    }

    /// Speak with an explicit follow-up policy.
    pub fn speak_with(text: impl Into<String>, expect_reply: bool) -> Self {
        Self::Speak {
            text: text.into(),
            expect_reply,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Speak { text, .. } | Self::NeedsConfirmation { text, .. } => Some(text.as_str()),
            Self::Silent => None,
        }
    }

    pub fn expect_reply(&self) -> bool {
        match self {
            Self::Speak { expect_reply, .. } => *expect_reply,
            // Confirm prompts always need a yes/no (or freeform) answer.
            Self::NeedsConfirmation { .. } => true,
            Self::Silent => false,
        }
    }

    pub fn is_needs_confirmation(&self) -> bool {
        matches!(self, Self::NeedsConfirmation { .. })
    }
}

/// True when the spoken line is a real question the user should answer freely.
///
/// Freeform by design: name, choice, clarify, yes/no — anything speakable.
/// Heuristic only (no LLM). Used when the model does not set policy explicitly.
pub fn looks_like_question(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    let core = t.trim_end_matches(|c: char| c == '!' || c == '.' || c == '…' || c.is_whitespace());
    if !core.ends_with('?') {
        return false;
    }
    let words = core.split_whitespace().count();
    if words == 0 || words > 28 {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn question_marks_expect_reply() {
        assert!(looks_like_question("What should I call you?"));
        assert!(looks_like_question("Rust or TypeScript?"));
        assert!(looks_like_question("Want me to remember that?"));
    }

    #[test]
    fn statements_do_not() {
        assert!(!looks_like_question("I am basically a genius."));
        assert!(!looks_like_question("Done."));
        assert!(!looks_like_question(""));
    }

    #[test]
    fn speak_helper_sets_flag() {
        match AgentOutcome::speak("What is your name?") {
            AgentOutcome::Speak { expect_reply, .. } => assert!(expect_reply),
            _ => panic!("expected Speak"),
        }
        match AgentOutcome::speak("All good.") {
            AgentOutcome::Speak { expect_reply, .. } => assert!(!expect_reply),
            _ => panic!("expected Speak"),
        }
    }
}
