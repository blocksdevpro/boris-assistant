//! Live status publisher for the UI (`StatusPicture` snapshots).

use std::sync::mpsc::Sender;

use boris_core::TurnId;

use crate::status::{
    ArtifactPeek, DeviceHealth, EngineState, Phase, StatusPicture, DEFAULT_CONTEXT_LIMIT_TOKENS,
};

/// Mutable engine-side status that publishes a full [`StatusPicture`] on change.
pub(super) struct Picture {
    pub engine: EngineState,
    pub phase: Phase,
    pub detail: Option<String>,
    pub heard: Option<String>,
    pub said: Option<String>,
    pub mic: DeviceHealth,
    pub speaker: DeviceHealth,
    pub turn: Option<TurnId>,
    pub activity: Option<String>,
    pub context_used: Option<u32>,
    pub context_limit: Option<u32>,
    pub artifact: Option<ArtifactPeek>,
    pub status_tx: Sender<StatusPicture>,
}

impl Picture {
    pub fn publish(&self) {
        let _ = self.status_tx.send(StatusPicture {
            engine: self.engine,
            phase: self.phase,
            detail: self.detail.clone(),
            heard: self.heard.clone(),
            said: self.said.clone(),
            mic: self.mic.clone(),
            speaker: self.speaker.clone(),
            turn: self.turn.map(|t| t.to_string()),
            activity: self.activity.clone(),
            context_used: self.context_used,
            context_limit: self.context_limit,
            artifact: self.artifact.clone(),
        });
    }

    pub fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        self.publish();
    }

    pub fn clear_activity(&mut self) {
        if self.activity.take().is_some() {
            self.publish();
        }
    }

    /// Rough token estimate (chars/4) for the overlay context meter only.
    pub fn update_context_from_chars(&mut self, approx_chars: usize) {
        let used = (approx_chars as u32 / 4).max(1);
        self.context_used = Some(used);
        self.context_limit = Some(DEFAULT_CONTEXT_LIMIT_TOKENS);
        self.publish();
    }
}
