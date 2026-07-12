//! Product policy for one voice interaction lives here — nowhere else.
//!
//! Pipeline:
//! 1. Runtime maps worker [`boris_core::event::Event`]s → [`SessionInput`]
//! 2. [`Session::handle`] returns [`Effect`]s (pure-ish transitions)
//! 3. Runtime applies effects to sensors / STT / agent / TTS / playback
//!
//! Speech text reaches Session only as [`SessionInput::AgentDone`] after the
//! agent worker maps [`boris_agent::AgentOutcome`] — not via tool side-channels.

mod effect;
mod input;
mod state;

#[cfg(test)]
mod tests;

pub use effect::Effect;
pub use input::SessionInput;
pub use state::SessionState;

use boris_core::TurnId;

/// Pure-ish session orchestrator: `(state, input) → (new state, effects)`.
pub struct Session {
    state: SessionState,
    next_turn: u64,
}

impl Session {
    pub fn new() -> Self {
        Self {
            state: SessionState::Idle,
            next_turn: 1,
        }
    }

    #[allow(dead_code)] // useful for debugging / tests
    pub fn state(&self) -> &SessionState {
        &self.state
    }

    fn alloc_turn(&mut self) -> TurnId {
        let id = TurnId(self.next_turn);
        self.next_turn = self.next_turn.saturating_add(1);
        id
    }

    fn current_turn(&self) -> Option<TurnId> {
        self.state.turn()
    }

    /// Drop results that do not belong to the active turn.
    fn is_current(&self, turn: TurnId) -> bool {
        self.current_turn() == Some(turn)
    }

    /// Handle one input. Returns effects the runtime must apply in order.
    pub fn handle(&mut self, input: SessionInput) -> Vec<Effect> {
        match input {
            SessionInput::WakeHit => self.on_wake(),
            SessionInput::Endpoint => self.on_endpoint(),
            SessionInput::ClipReady { turn, audio } => self.on_clip(turn, audio),
            SessionInput::Transcript { turn, text } => self.on_transcript(turn, text),
            SessionInput::AgentDone { turn, text } => self.on_agent_done(turn, text),
            SessionInput::TtsReady { turn, pcm } => self.on_tts_ready(turn, pcm),
            SessionInput::PlaybackFinished { turn } => self.on_playback_finished(turn),
            SessionInput::ServiceFailed {
                turn,
                worker,
                message,
            } => self.on_service_failed(turn, worker, message),
        }
    }

    fn on_wake(&mut self) -> Vec<Effect> {
        if !matches!(self.state, SessionState::Idle) {
            tracing::debug!(state = ?self.state, "ignoring wake while busy");
            return vec![];
        }

        let turn = self.alloc_turn();
        self.state = SessionState::Listening { turn };
        tracing::info!(%turn, "session → Listening");

        vec![
            Effect::DisarmWakeword,
            Effect::StartListen { turn },
            Effect::WarmStt,
        ]
    }

    fn on_endpoint(&mut self) -> Vec<Effect> {
        let SessionState::Listening { turn } = self.state else {
            tracing::debug!(state = ?self.state, "ignoring endpoint outside Listening");
            return vec![];
        };

        self.state = SessionState::AwaitingClip { turn };
        tracing::info!(%turn, "session → AwaitingClip");
        vec![Effect::StopListen]
    }

    fn on_clip(&mut self, turn: TurnId, audio: boris_core::AudioBuffer) -> Vec<Effect> {
        if !self.is_current(turn) {
            tracing::debug!(%turn, "dropping stale ClipReady");
            return vec![];
        }

        match self.state {
            SessionState::AwaitingClip { .. } | SessionState::Listening { .. } => {
                // Listening + clip is defensive if StopListen races.
                self.state = SessionState::Transcribing { turn };
                tracing::info!(%turn, samples = audio.len(), "session → Transcribing");
                vec![Effect::Transcribe { turn, audio }]
            }
            _ => {
                tracing::debug!(state = ?self.state, %turn, "ignoring ClipReady");
                vec![]
            }
        }
    }

    fn on_transcript(&mut self, turn: TurnId, text: String) -> Vec<Effect> {
        if !self.is_current(turn) {
            tracing::debug!(%turn, "dropping stale Transcript");
            return vec![];
        }

        if !matches!(self.state, SessionState::Transcribing { .. }) {
            tracing::debug!(state = ?self.state, %turn, "ignoring Transcript");
            return vec![];
        }

        self.state = SessionState::Thinking { turn };
        tracing::info!(%turn, text = %text, "session → Thinking");
        vec![Effect::Chat { turn, text }, Effect::WarmTts]
    }

    fn on_agent_done(&mut self, turn: TurnId, text: String) -> Vec<Effect> {
        if !self.is_current(turn) {
            tracing::debug!(%turn, "dropping stale AgentDone");
            return vec![];
        }

        if !matches!(self.state, SessionState::Thinking { .. }) {
            tracing::debug!(state = ?self.state, %turn, "ignoring AgentDone");
            return vec![];
        }

        if text.trim().is_empty() {
            tracing::warn!(%turn, "agent returned empty speech — recovering to Idle");
            self.state = SessionState::Idle;
            return vec![Effect::ArmWakeword];
        }

        self.state = SessionState::Speaking { turn };
        tracing::info!(%turn, text = %text, "session → Speaking");
        vec![Effect::DisarmWakeword, Effect::Synthesize { turn, text }]
    }

    fn on_tts_ready(&mut self, turn: TurnId, pcm: boris_core::AudioBuffer) -> Vec<Effect> {
        if !self.is_current(turn) {
            tracing::debug!(%turn, "dropping stale TtsReady");
            return vec![];
        }

        if !matches!(self.state, SessionState::Speaking { .. }) {
            tracing::debug!(state = ?self.state, %turn, "ignoring TtsReady");
            return vec![];
        }

        // Stay in Speaking until PlaybackFinished.
        vec![Effect::Play { turn, pcm }]
    }

    fn on_playback_finished(&mut self, turn: TurnId) -> Vec<Effect> {
        if !self.is_current(turn) {
            tracing::debug!(%turn, "dropping stale PlaybackFinished");
            return vec![];
        }

        if !matches!(self.state, SessionState::Speaking { .. }) {
            tracing::debug!(state = ?self.state, %turn, "ignoring PlaybackFinished");
            return vec![];
        }

        self.state = SessionState::Idle;
        tracing::info!(%turn, "session → Idle (playback finished)");
        vec![Effect::ArmWakeword]
    }

    fn on_service_failed(
        &mut self,
        turn: Option<TurnId>,
        worker: &'static str,
        message: String,
    ) -> Vec<Effect> {
        if let Some(t) = turn {
            if !self.is_current(t) {
                tracing::debug!(%t, worker, "ignoring stale ServiceFailed");
                return vec![];
            }
        } else if matches!(self.state, SessionState::Idle) {
            tracing::error!(worker, message = %message, "worker error while Idle");
            return vec![];
        }

        tracing::error!(
            worker,
            message = %message,
            state = ?self.state,
            "service failed — recovering to Idle"
        );

        let mut effects = Vec::new();
        match self.state {
            SessionState::Listening { .. } | SessionState::AwaitingClip { .. } => {
                effects.push(Effect::StopListen);
            }
            _ => {}
        }

        self.state = SessionState::Idle;
        effects.push(Effect::ArmWakeword);
        effects
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}
