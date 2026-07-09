use std::fmt;
use std::sync::Arc;

pub type AudioSample = f32;
pub type AudioBuffer = Vec<AudioSample>;

pub type ArcAudioBuffer = Arc<[AudioSample]>;

pub enum Lifecycle {
    Start,
    Stop,
}

/// Identifies one full interaction cycle: wake → listen → STT → agent → speak.
/// Late async results with a non-current id are dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TurnId(pub u64);

impl fmt::Display for TurnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Which backend reported a failure (for logging / Session recovery).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    Stt,
    Agent,
    Tts,
    Audio,
    Unknown,
}

impl ServiceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceKind::Stt => "stt",
            ServiceKind::Agent => "agent",
            ServiceKind::Tts => "tts",
            ServiceKind::Audio => "audio",
            ServiceKind::Unknown => "unknown",
        }
    }
}
