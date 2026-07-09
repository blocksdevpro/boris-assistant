use crate::{AudioBuffer, ServiceKind, TurnId};

/// Messages workers send to the runtime (mapped into [`crate`] session inputs).
pub enum Event {
    // ── Audio / sensors ───────────────────────────────────────────────────────
    WakeWordDetected,
    SpeechEnded,
    RecordingResult {
        turn: TurnId,
        audio: AudioBuffer,
    },

    // ── Services ──────────────────────────────────────────────────────────────
    SpeechToTextResult {
        turn: TurnId,
        text: String,
    },
    AgentResponse {
        turn: TurnId,
        text: String,
    },
    /// TTS produced PCM; runtime plays it then schedules [`PlaybackFinished`].
    PlaybackReady {
        turn: TurnId,
        audio: AudioBuffer,
    },
    /// Speaker is done (or duration estimate elapsed). Safe to re-arm wakeword.
    PlaybackFinished {
        turn: TurnId,
    },

    // ── Errors ────────────────────────────────────────────────────────────────
    WorkerError {
        turn: Option<TurnId>,
        worker: &'static str,
        kind: ServiceKind,
        message: String,
    },
}
