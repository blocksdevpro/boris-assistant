use boris_core::TurnId;

/// Product phases for a single voice interaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// Waiting for the wake word.
    Idle,
    /// Capturing user speech (VAD + recorder running).
    Listening { turn: TurnId },
    /// Endpoint detected; waiting for the recorder to emit the clip.
    AwaitingClip { turn: TurnId },
    /// STT in flight.
    Transcribing { turn: TurnId },
    /// Agent / LLM in flight.
    Thinking { turn: TurnId },
    /// TTS + playback in flight.
    Speaking { turn: TurnId },
}

impl SessionState {
    pub fn turn(&self) -> Option<TurnId> {
        match *self {
            SessionState::Idle => None,
            SessionState::Listening { turn }
            | SessionState::AwaitingClip { turn }
            | SessionState::Transcribing { turn }
            | SessionState::Thinking { turn }
            | SessionState::Speaking { turn } => Some(turn),
        }
    }

    #[allow(dead_code)] // used by unit tests + debugging
    pub fn is_idle(&self) -> bool {
        matches!(self, SessionState::Idle)
    }
}
