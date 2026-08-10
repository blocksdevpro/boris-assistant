//! Terminal emissions for spoken replies and HITL pauses.

use std::sync::Arc;

use crate::context::Role;
use crate::error::AgentError;
use crate::outcome::AgentOutcome;
use crate::runtime::PendingTurn;
use crate::types::{AgentEvent, EmitFn, LoopResult};

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
) -> Result<LoopResult, AgentError> {
    emit(AgentEvent::NeedsConfirmation {
        pending: pending_turn.pending.clone(),
    });
    emit(AgentEvent::TurnEnd { round });
    emit(AgentEvent::AgentEnd {
        outcome: outcome.clone(),
    });
    Ok(LoopResult {
        outcome,
        tool_rounds,
        tools_used,
        pending_turn: Some(pending_turn),
    })
}

pub(super) fn finish_with_speech(
    emit: &EmitFn,
    round: u32,
    reply: String,
    tool_rounds: u32,
    tools_used: Vec<String>,
) -> Result<LoopResult, AgentError> {
    emit(AgentEvent::MessageEnd {
        role: Role::Assistant,
        preview: log_preview(&reply, 80),
    });
    emit(AgentEvent::TurnEnd { round });

    let outcome = if reply.is_empty() {
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
    })
}
