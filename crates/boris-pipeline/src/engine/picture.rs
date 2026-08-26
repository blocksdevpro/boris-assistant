//! Live status publisher for the UI (`StatusPicture` snapshots).

use std::sync::mpsc::Sender;
use std::time::Instant;

use boris_core::TurnId;

use crate::status::{
    ArtifactPeek, DeviceHealth, EngineState, InputPeek, Phase, StatusPicture, WakeEnrollPeek,
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
    pub thinking: Option<String>,
    pub context_used: Option<u32>,
    pub context_limit: Option<u32>,
    pub context_estimated: bool,
    pub artifact: Option<ArtifactPeek>,
    pub wake_enroll: Option<WakeEnrollPeek>,
    pub input: Option<InputPeek>,
    pub status_tx: Sender<StatusPicture>,
    /// When the current [`Phase`] began — used to log how long each status lasted.
    pub phase_started: Instant,
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
            thinking: self.thinking.clone(),
            context_used: self.context_used,
            context_limit: self.context_limit,
            context_estimated: self.context_estimated,
            artifact: self.artifact.clone(),
            wake_enroll: self.wake_enroll.clone(),
            input: self.input.clone(),
        });
    }

    pub fn set_wake_enroll(&mut self, peek: Option<WakeEnrollPeek>) {
        if self.wake_enroll != peek {
            self.wake_enroll = peek;
            self.publish();
        }
    }

    pub fn set_phase(&mut self, phase: Phase) {
        if self.phase != phase {
            let ms = self.phase_started.elapsed().as_millis() as u64;
            tracing::info!(from = ?self.phase, to = ?phase, ms, "status phase");
            self.phase = phase;
            self.phase_started = Instant::now();
            if phase != Phase::Thinking {
                self.thinking = None;
            }
            if phase != Phase::AwaitingInput {
                self.input = None;
            }
        }
        self.publish();
    }

    pub fn clear_activity(&mut self) {
        let activity = self.activity.take();
        let thinking = self.thinking.take();
        if activity.is_some() || thinking.is_some() {
            self.publish();
        }
    }

    /// Rough token estimate (chars/4) for the overlay context meter only.
    pub fn update_context_from_chars(&mut self, approx_chars: usize) {
        let used = (approx_chars as u32 / 4).max(1);
        self.context_used = Some(used);
        self.context_estimated = true;
        self.publish();
    }

    pub fn update_context(&mut self, used: u32, limit: Option<u32>, estimated: bool) {
        self.context_used = Some(used.max(1));
        if limit.is_some() {
            self.context_limit = limit;
        }
        self.context_estimated = estimated;
        self.publish();
    }
}
