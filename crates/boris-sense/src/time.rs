use std::time::Duration;

use boris_core::AUDIO_TARGET_RATE;

use crate::vad::{VAD_INITIAL_TIMEOUT, VAD_SILENCE_WINDOW};

/// Convert a wall duration into a sample count at `sample_rate`.
///
/// Used so VAD / wakeword thresholds are expressed in ms but enforced in
/// **audio time** (samples processed), not `Instant` wall clock.
pub fn duration_to_samples(d: Duration, sample_rate: u32) -> usize {
    let secs = d.as_secs_f64();
    (secs * sample_rate as f64).round() as usize
}

/// Samples of non-speech after speech before endpointing (`VAD_SILENCE_WINDOW`).
pub fn vad_silence_samples() -> usize {
    duration_to_samples(VAD_SILENCE_WINDOW, AUDIO_TARGET_RATE)
}

/// Samples of non-speech before any speech before giving up (`VAD_INITIAL_TIMEOUT`).
pub fn vad_initial_timeout_samples() -> usize {
    duration_to_samples(VAD_INITIAL_TIMEOUT, AUDIO_TARGET_RATE)
}
