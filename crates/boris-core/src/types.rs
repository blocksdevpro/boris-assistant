//! Shared value types used across speech and pipeline crates.

use std::fmt;
use std::sync::Arc;

/// Mono PCM sample type used after resampling into the pipeline rate.
///
/// Samples are interleaved mono `f32` amplitudes in approximately `[-1.0, 1.0]`.
/// Values outside that range may appear briefly from device gain or resampling;
/// consumers should treat them as soft-clipped full scale, not integer PCM.
pub type AudioSample = f32;

/// Owned mono PCM buffer (`Vec<f32>`).
///
/// Layout: mono interleaved `f32` samples at [`AUDIO_TARGET_RATE`] (16 kHz),
/// amplitudes approximately in `[-1.0, 1.0]`.
pub type AudioBuffer = Vec<AudioSample>;

/// Cheaply cloneable mono PCM buffer (`Arc<[f32]>`).
///
/// Prefer this when the same buffer is handed to multiple consumers
/// (e.g. capture → STT and a debug sink) without copying samples.
///
/// # Construction
///
/// Move an owned buffer without copying samples:
///
/// ```
/// use std::sync::Arc;
/// use boris_core::{ArcAudioBuffer, AudioBuffer};
///
/// let owned: AudioBuffer = vec![0.0, 0.5, -0.25];
/// let shared: ArcAudioBuffer = Arc::from(owned); // moves, does not copy samples
/// assert_eq!(shared.len(), 3);
/// ```
///
/// Building from a slice (`Arc::from(slice)`) **does** allocate and copy.
/// Prefer `Arc::from(owned_vec)` when you already own the samples.
///
/// Layout matches [`AudioBuffer`]: mono interleaved `f32` at 16 kHz,
/// amplitudes approximately in `[-1.0, 1.0]`.
pub type ArcAudioBuffer = Arc<[AudioSample]>;

/// Sample rate (Hz) every pipeline stage uses after input resampling.
///
/// Mic devices may run at other rates; `boris-audio` resamples into this
/// target before VAD, wake, and STT see the signal.
///
/// Buffers at this rate are mono interleaved `f32` with amplitudes
/// approximately in `[-1.0, 1.0]`.
pub const AUDIO_TARGET_RATE: u32 = 16_000;

/// Identifies one full interaction cycle: wake → listen → STT → agent → speak.
///
/// The engine increments this per turn. Late async results whose id no longer
/// matches the current turn are dropped so a slow STT/TTS cannot speak into a
/// newer conversation.
///
/// The inner counter is private; construct with [`TurnId::new`] or [`From<u64>`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TurnId(u64);

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

    /// Next turn id (saturating at [`u64::MAX`]).
    ///
    /// Saturation is preferred over wrapping so turn counters never silently
    /// reuse low ids after overflow (which would re-accept stale async work).
    #[must_use]
    #[inline]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_id_next_and_display() {
        let t = TurnId::new(7);
        assert_eq!(t.get(), 7);
        assert_eq!(t.next(), TurnId::new(8));
        assert_eq!(t.to_string(), "7");
        assert_eq!(TurnId::from(3u64), TurnId::new(3));
    }

    #[test]
    fn turn_id_next_saturates_at_max() {
        let max = TurnId::new(u64::MAX);
        assert_eq!(max.get(), u64::MAX);
        assert_eq!(max.next(), TurnId::new(u64::MAX));
        assert_eq!(max.next().next(), max);

        let near = TurnId::new(u64::MAX - 1);
        assert_eq!(near.next(), TurnId::new(u64::MAX));
        assert_eq!(near.next().next(), TurnId::new(u64::MAX));
    }

    #[test]
    fn audio_aliases_are_f32() {
        let buf: AudioBuffer = vec![0.0, 0.5, -0.5];
        // Prefer Arc::from(owned) — moves the allocation, no sample copy.
        let arc: ArcAudioBuffer = Arc::from(buf);
        assert_eq!(arc.len(), 3);
        assert_eq!(&arc[..], &[0.0, 0.5, -0.5]);
        assert_eq!(AUDIO_TARGET_RATE, 16_000);
    }

    #[test]
    fn arc_audio_buffer_from_owned_vec_preserves_samples() {
        let owned: AudioBuffer = vec![0.1, -0.2, 0.3, -0.4];
        let expected = owned.clone();
        let shared: ArcAudioBuffer = Arc::from(owned);
        assert_eq!(shared.as_ref(), expected.as_slice());
    }
}
