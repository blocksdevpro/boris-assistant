//! WebRTC VAD with aggressiveness + adaptive energy gate.
//!
//! Plain WebRTC Quality mode often classifies music / TV / room hum as "voice",
//! so silence never accumulates and capture runs until the max utterance.
//! We combine:
//! 1. **Aggressive** WebRTC mode (less sensitive to steady noise)
//! 2. **RMS energy gate** vs an adaptive noise floor (low background stays "not speech")
//! 3. **Speech hangover** — need a few consecutive speech frames to start / hold

use boris_core::AUDIO_TARGET_RATE;
use webrtc_vad::{SampleRate, Vad as WebVad, VadMode};

use crate::pcm::f32_to_pcm16_samples;
use crate::vad::Vad;
use boris_core::{error::Result, AudioSample};

/// Absolute RMS floor: below this is never speech (near-silence).
const ABS_SPEECH_RMS: f32 = 0.012;
/// Frame must exceed `noise_floor * this` to count as speech.
const SPEECH_OVER_NOISE: f32 = 2.8;
/// EMA for noise floor when frame is non-speech.
const NOISE_EMA: f32 = 0.05;
/// Slow floor rise even during "speech" so a long music bed still adapts.
const NOISE_EMA_SLOW: f32 = 0.004;
/// Consecutive speech frames required to enter speech (reduces music blips).
const SPEECH_ON_FRAMES: u8 = 3;
/// Consecutive non-speech frames before leaving speech hangover.
const SPEECH_OFF_FRAMES: u8 = 4;

pub struct WebRtcVad {
    model: WebVad,
    noise_floor: f32,
    speech_run: u8,
    silence_run: u8,
    in_speech: bool,
}

// The underlying C library is not Send by default; it is safe to mark it Send
// because each WebRtcVad instance is exclusively owned by a single worker thread.
unsafe impl Send for WebRtcVad {}

impl WebRtcVad {
    pub fn new() -> Self {
        let sample_rate = SampleRate::try_from(AUDIO_TARGET_RATE as i32)
            .expect("AUDIO_TARGET_RATE is not a valid WebRTC VAD sample rate");
        // Aggressive: rejects more non-speech than Quality (default).
        // VeryAggressive can clip soft speech; Aggressive is a better default.
        Self {
            model: WebVad::new_with_rate_and_mode(sample_rate, VadMode::Aggressive),
            noise_floor: ABS_SPEECH_RMS,
            speech_run: 0,
            silence_run: 0,
            in_speech: false,
        }
    }

    fn frame_rms(audio: &[AudioSample]) -> f32 {
        if audio.is_empty() {
            return 0.0;
        }
        let sum: f32 = audio.iter().map(|s| s * s).sum();
        (sum / audio.len() as f32).sqrt()
    }

    fn energy_ok(&self, rms: f32) -> bool {
        let gate = (self.noise_floor * SPEECH_OVER_NOISE).max(ABS_SPEECH_RMS);
        rms >= gate
    }
}

impl Default for WebRtcVad {
    fn default() -> Self {
        Self::new()
    }
}

impl Vad for WebRtcVad {
    fn predict(&mut self, audio: &[AudioSample]) -> Result<bool> {
        let rms = Self::frame_rms(audio);
        let pcm = f32_to_pcm16_samples(audio);
        let webrtc_voice = self
            .model
            .is_voice_segment(&pcm)
            .map_err(|_| boris_core::error::Error::Other("webrtc-vad prediction failed".into()))?;

        // Candidate speech: WebRTC agrees AND energy is above ambient.
        let raw_speech = webrtc_voice && self.energy_ok(rms);

        if raw_speech {
            self.speech_run = self.speech_run.saturating_add(1);
            self.silence_run = 0;
        } else {
            self.silence_run = self.silence_run.saturating_add(1);
            self.speech_run = 0;
            // Update noise floor on clear non-speech.
            self.noise_floor = self.noise_floor * (1.0 - NOISE_EMA) + rms * NOISE_EMA;
        }

        // Always creep the floor slowly so a continuous music bed is tracked.
        self.noise_floor = self.noise_floor * (1.0 - NOISE_EMA_SLOW) + rms * NOISE_EMA_SLOW;
        // Never let the floor sink below a tiny epsilon or explode.
        self.noise_floor = self.noise_floor.clamp(0.002, 0.15);

        if !self.in_speech {
            if self.speech_run >= SPEECH_ON_FRAMES {
                self.in_speech = true;
            }
        } else if self.silence_run >= SPEECH_OFF_FRAMES {
            self.in_speech = false;
        }

        Ok(self.in_speech)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_not_speech() {
        let mut vad = WebRtcVad::new();
        let frame = vec![0.0f32; 160];
        // A few frames of silence should stay non-speech.
        for _ in 0..5 {
            assert!(!vad.predict(&frame).unwrap());
        }
    }

    #[test]
    fn low_level_noise_stays_non_speech() {
        let mut vad = WebRtcVad::new();
        // Quiet background (~RMS 0.005) — should not open speech hangover.
        let frame: Vec<f32> = (0..160)
            .map(|i| 0.005 * ((i as f32 * 0.3).sin()))
            .collect();
        for _ in 0..20 {
            // Warm noise floor then check.
            let _ = vad.predict(&frame);
        }
        assert!(
            !vad.predict(&frame).unwrap(),
            "low background should not be treated as speech"
        );
    }
}
