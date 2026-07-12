use crate::{AudioBuffer, ServiceKind, TurnId};

/// Facts workers emit toward the main runtime.
///
/// The binary maps each variant into a session input (`map_event` in `main`).
/// Product policy lives in the Session FSM, not in this enum.
pub enum Event {
    // ── Audio / sensors ───────────────────────────────────────────────────────
    /// Wake sensor crossed the detection threshold.
    WakeWordDetected,
    /// Endpoint sensor decided the user stopped talking (silence in audio-time).
    SpeechEnded,
    /// Utterance capture finished; PCM is ready for STT.
    RecordingResult { turn: TurnId, audio: AudioBuffer },

    // ── Services ──────────────────────────────────────────────────────────────
    /// STT produced a transcript for this turn.
    SpeechToTextResult { turn: TurnId, text: String },
    /// Agent finished with speakable text for this turn (→ TTS).
    ///
    /// Emitted once by `AgentWorker` after the engine returns speakable text,
    /// not by tools or the agent crate itself.
    AgentResponse { turn: TurnId, text: String },
    /// TTS produced PCM; Session applies [`Effect::Play`] / queues a play job.
    PlaybackReady { turn: TurnId, audio: AudioBuffer },
    /// Playback sink drained this turn's audio (real underrun, not a wall-clock guess).
    /// Session may return to Idle and re-arm the wakeword.
    PlaybackFinished { turn: TurnId },

    // ── Errors ────────────────────────────────────────────────────────────────
    /// Non-fatal worker failure. Session recovers to Idle when the turn matches.
    WorkerError {
        turn: Option<TurnId>,
        worker: &'static str,
        kind: ServiceKind,
        message: String,
    },
}
