use boris_core::{AudioBuffer, TurnId};

/// Inputs the Session FSM accepts (mapped from worker [`boris_core::event::Event`]s).
///
/// Anything with a [`TurnId`] is dropped when it does not match the active turn.
#[derive(Debug)]
pub enum SessionInput {
    /// Wake sensor fired.
    WakeHit,
    /// Endpoint sensor reported end of user speech.
    Endpoint,
    /// Utterance PCM ready for STT.
    ClipReady {
        turn: TurnId,
        audio: AudioBuffer,
    },
    /// STT finished.
    Transcript {
        turn: TurnId,
        text: String,
    },
    /// Agent returned speakable text (from `AgentOutcome::Speak` via the worker).
    AgentDone {
        turn: TurnId,
        text: String,
    },
    /// TTS produced PCM.
    TtsReady {
        turn: TurnId,
        pcm: AudioBuffer,
    },
    /// Playback sink drained this turn.
    PlaybackFinished {
        turn: TurnId,
    },
    /// STT / agent / TTS (or similar) failed; recover toward Idle.
    ServiceFailed {
        turn: Option<TurnId>,
        worker: &'static str,
        message: String,
    },
}
