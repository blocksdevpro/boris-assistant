use boris_core::{AudioBuffer, TurnId};

/// Side effects the runtime applies. Session never touches channels itself.
#[derive(Debug)]
pub enum Effect {
    ArmWakeword,
    DisarmWakeword,
    StartListen {
        turn: TurnId,
    },
    StopListen,
    WarmStt,
    Transcribe {
        turn: TurnId,
        audio: AudioBuffer,
    },
    WarmTts,
    Chat {
        turn: TurnId,
        text: String,
    },
    Synthesize {
        turn: TurnId,
        text: String,
    },
    Play {
        turn: TurnId,
        pcm: AudioBuffer,
    },
}
