//! Shared value types used across speech and pipeline crates.

use std::fmt;
use std::sync::Arc;

/// Mono PCM sample type used after resampling into the pipeline rate.
pub type AudioSample = f32;

/// Owned mono PCM buffer (`Vec<f32>`).
pub type AudioBuffer = Vec<AudioSample>;

/// Cheaply cloneable mono PCM buffer (`Arc<[f32]>`).
///
/// Prefer this when the same buffer is handed to multiple consumers
/// (e.g. capture → STT and a debug sink) without copying samples.
pub type ArcAudioBuffer = Arc<[AudioSample]>;

/// Sample rate (Hz) every pipeline stage uses after input resampling.
///
/// Mic devices may run at other rates; `boris-audio` resamples into this
/// target before VAD, wake, and STT see the signal.
pub const AUDIO_TARGET_RATE: u32 = 16_000;

/// Identifies one full interaction cycle: wake → listen → STT → agent → speak.
///
/// The engine increments this per turn. Late async results whose id no longer
/// matches the current turn are dropped so a slow STT/TTS cannot speak into a
/// newer conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TurnId(pub u64);

impl TurnId {
    /// Create a turn id from a raw counter value.
    #[inline]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// Raw counter value.
    #[inline]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Next turn id (wrapping).
    #[inline]
    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

impl fmt::Display for TurnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for TurnId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

/// Which backend reported a failure (logging / recovery).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceKind {
    /// Speech-to-text adapter.
    Stt,
    /// LLM / agent harness.
    Agent,
    /// Text-to-speech adapter.
    Tts,
    /// Capture or playback path.
    Audio,
    /// Unclassified / unknown backend.
    Unknown,
}

impl ServiceKind {
    /// Stable lowercase label for logs and UI.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stt => "stt",
            Self::Agent => "agent",
            Self::Tts => "tts",
            Self::Audio => "audio",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for ServiceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_id_next_and_display() {
        let t = TurnId::new(7);
        assert_eq!(t.get(), 7);
        assert_eq!(t.next(), TurnId(8));
        assert_eq!(t.to_string(), "7");
        assert_eq!(TurnId::from(3u64), TurnId(3));
    }

    #[test]
    fn service_kind_labels() {
        assert_eq!(ServiceKind::Stt.as_str(), "stt");
        assert_eq!(ServiceKind::Agent.to_string(), "agent");
        assert_eq!(ServiceKind::Unknown.as_str(), "unknown");
    }

    #[test]
    fn audio_aliases_are_f32() {
        let buf: AudioBuffer = vec![0.0, 0.5, -0.5];
        let arc: ArcAudioBuffer = Arc::from(buf.as_slice());
        assert_eq!(arc.len(), 3);
        assert_eq!(AUDIO_TARGET_RATE, 16_000);
    }
}
