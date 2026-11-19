//! Short "I'm listening" chime played the moment a wake is accepted.
//!
//! Synthesized in code (no asset): two soft sine partials a fifth apart, the
//! second entering slightly late so it reads as a rising "ba-ding". It goes
//! through the normal output pipeline, so AEC sees it as far-end audio and
//! removes it from the capture that starts right after.

use std::f32::consts::TAU;
use std::time::Duration;

use boris_core::AudioBuffer;
use boris_sense::duration_to_samples;

/// Whole chime, including the release tail.
const LENGTH: Duration = Duration::from_millis(190);
/// Second partial starts this late.
const SECOND_DELAY: Duration = Duration::from_millis(55);
/// Attack ramp per partial (removes the click at note-on).
const ATTACK: Duration = Duration::from_millis(6);
/// Peak amplitude of each partial. Two overlapping partials stay well under
/// full scale, and quieter than Boris's voice.
const PEAK: f32 = 0.11;
/// Base pitch (A5) and the fifth above it.
const FIRST_HZ: f32 = 880.0;
const SECOND_HZ: f32 = 1318.5;
/// Exponential decay rate (per second) once a partial has attacked.
const DECAY_PER_SEC: f32 = 22.0;

/// Render the chime as mono PCM at `sample_rate` (the TTS / output rate).
pub(super) fn wake_chime(sample_rate: u32) -> AudioBuffer {
    let samples = |d: Duration| duration_to_samples(d, sample_rate);
    let rate = sample_rate.max(1) as f32;
    let len = samples(LENGTH);
    let delay = samples(SECOND_DELAY);
    let attack = samples(ATTACK).max(1) as f32;
    // Fade the whole buffer out over its last 20 ms so it always ends on 0.
    let fade = (0.020 * rate).max(1.0);

    let partial = |i: usize, start: usize, hz: f32| -> f32 {
        let Some(n) = i.checked_sub(start) else {
            return 0.0;
        };
        let t = n as f32 / rate;
        let env = (n as f32 / attack).min(1.0) * (-DECAY_PER_SEC * t).exp();
        PEAK * env * (TAU * hz * t).sin()
    };

    (0..len)
        .map(|i| {
            let tail = ((len - i) as f32 / fade).min(1.0);
            (partial(i, 0, FIRST_HZ) + partial(i, delay, SECOND_HZ)) * tail
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chime_length_follows_the_sample_rate() {
        assert_eq!(wake_chime(44_100).len(), 8379);
        assert_eq!(wake_chime(16_000).len(), 3040);
    }

    #[test]
    fn chime_is_quiet_and_starts_and_ends_silent() {
        let pcm = wake_chime(44_100);
        let peak = pcm.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.05 && peak < 0.25, "peak {peak}");
        assert!(pcm[0].abs() < 1e-6, "no click at start");
        assert!(pcm.last().unwrap().abs() < 1e-3, "no click at end");
    }

    #[test]
    fn degenerate_rate_does_not_panic() {
        assert!(wake_chime(0).is_empty());
    }
}
