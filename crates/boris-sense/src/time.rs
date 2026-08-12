//! Convert wall durations into sample counts at the pipeline rate.
//!
//! VAD / wake thresholds are authored in milliseconds but enforced in
//! **audio time** (samples processed), not wall-clock `Instant`.

use std::time::Duration;

/// Convert a wall duration into a sample count at `sample_rate`.
pub fn duration_to_samples(d: Duration, sample_rate: u32) -> usize {
    if sample_rate == 0 {
        return 0;
    }
    let secs = d.as_secs_f64();
    (secs * f64::from(sample_rate)).round() as usize
}

/// Samples of non-speech after speech before endpointing (`VAD_SILENCE_WINDOW`).
#[cfg(feature = "vad")]
pub fn vad_silence_samples() -> usize {
    duration_to_samples(
        crate::vad::VAD_SILENCE_WINDOW,
        boris_core::AUDIO_TARGET_RATE,
    )
}

/// Samples of non-speech before any speech before giving up (`VAD_INITIAL_TIMEOUT`).
#[cfg(feature = "vad")]
pub fn vad_initial_timeout_samples() -> usize {
    duration_to_samples(
        crate::vad::VAD_INITIAL_TIMEOUT,
        boris_core::AUDIO_TARGET_RATE,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_second_at_16k() {
        assert_eq!(duration_to_samples(Duration::from_secs(1), 16_000), 16_000);
    }

    #[test]
    fn zero_rate_is_zero() {
        assert_eq!(duration_to_samples(Duration::from_secs(1), 0), 0);
    }

    #[test]
    fn subsecond_rounding() {
        // 40 ms @ 16 kHz = 640 samples
        assert_eq!(duration_to_samples(Duration::from_millis(40), 16_000), 640);
    }

    #[cfg(feature = "vad")]
    #[test]
    fn vad_helpers_positive() {
        assert!(vad_silence_samples() > 0);
        assert!(vad_initial_timeout_samples() > vad_silence_samples());
    }
}
