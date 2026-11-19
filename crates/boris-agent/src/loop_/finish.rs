//! Terminal emissions for spoken replies and HITL pauses.

use std::sync::Arc;

use crate::context::Role;
use crate::error::AgentError;
use crate::outcome::AgentOutcome;
use crate::runtime::PendingTurn;
use crate::types::{AgentEvent, EmitFn, LoopResult, TokenAccounting};

use super::message_parse::log_preview;

pub(super) fn noop_emit() -> EmitFn {
    Arc::new(|_| {})
}

pub(super) fn finish_paused(
    emit: &EmitFn,
    round: u32,
    outcome: AgentOutcome,
    tool_rounds: u32,
    tools_used: Vec<String>,
    pending_turn: PendingTurn,
    token_accounting: TokenAccounting,
) -> Result<LoopResult, AgentError> {
    match &outcome {
        AgentOutcome::NeedsInput { pending, .. } => {
            emit(AgentEvent::NeedsInput {
                pending: pending.clone(),
            });
        }
        _ => {
            emit(AgentEvent::NeedsConfirmation {
                pending: pending_turn.pending.clone(),
            });
        }
    }
    emit(AgentEvent::TurnEnd { round });
    emit(AgentEvent::AgentEnd {
        outcome: outcome.clone(),
    });
    Ok(LoopResult {
        outcome,
        tool_rounds,
        tools_used,
        pending_turn: Some(pending_turn),
        token_accounting,
    })
}

pub(super) fn finish_with_speech(
    emit: &EmitFn,
    round: u32,
    reply: String,
    tool_rounds: u32,
    tools_used: Vec<String>,
    token_accounting: TokenAccounting,
) -> Result<LoopResult, AgentError> {
    emit(AgentEvent::MessageEnd {
        role: Role::Assistant,
        preview: log_preview(&reply, 80),
    });
    emit(AgentEvent::TurnEnd { round });

    // Tool work always gets spoken; silence is only for non-directed speech.
    let outcome = if crate::outcome::is_silence_reply(&reply) && tools_used.is_empty() {
        AgentOutcome::Silent
    } else if reply.is_empty() || crate::outcome::is_silence_reply(&reply) {
        // Never return Silent after tool work if we can offer a fallback line.
        if tools_used.is_empty() {
            AgentOutcome::speak("I blanked on that one — say it again and I'll pick it up.")
        } else {
            AgentOutcome::speak(
                "I hit a snag wrapping up, but I did run some tools. Ask me to continue.",
            )
        }
    } else {
        AgentOutcome::speak(reply)
    };
    emit(AgentEvent::AgentEnd {
        outcome: outcome.clone(),
    });
    Ok(LoopResult {
        outcome,
        tool_rounds,
        tools_used,
        pending_turn: None,
        token_accounting,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome_for(reply: &str, tools: &[&str]) -> AgentOutcome {
        finish_with_speech(
            &noop_emit(),
            0,
            reply.to_string(),
            0,
            tools.iter().map(|t| t.to_string()).collect(),
            TokenAccounting::default(),
        )
        .unwrap()
        .outcome
    }

    #[test]
    fn silence_token_is_silent_only_without_tool_work() {
        assert_eq!(outcome_for("[silent]", &[]), AgentOutcome::Silent);
        assert_eq!(outcome_for("  [SILENT]. ", &[]), AgentOutcome::Silent);
        // Tool work must always be reported, never swallowed by silence.
        assert!(matches!(
            outcome_for("[silent]", &["file_write"]),
            AgentOutcome::Speak { .. }
        ));
        assert!(matches!(
            outcome_for("Silent night is a song.", &[]),
            AgentOutcome::Speak { .. }
        ));
    }
}
