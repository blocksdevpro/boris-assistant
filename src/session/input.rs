use boris_core::{AudioBuffer, TurnId};

/// Everything the session is allowed to react to.
#[derive(Debug)]
pub enum SessionInput {
    WakeHit,
    Endpoint,
    ClipReady {
        turn: TurnId,
        audio: AudioBuffer,
    },
    Transcript {
        turn: TurnId,
        text: String,
    },
    AgentDone {
        turn: TurnId,
        text: String,
    },
    TtsReady {
        turn: TurnId,
        pcm: AudioBuffer,
    },
    PlaybackFinished {
        turn: TurnId,
    },
    ServiceFailed {
        turn: Option<TurnId>,
        worker: &'static str,
        message: String,
    },
}
