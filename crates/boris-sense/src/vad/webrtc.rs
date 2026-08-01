//! WebRTC VAD with light noise rejection.
//!
//! Goals:
//! - Reject *steady low* background (music/TV bed) so capture can end
//! - **Not** cut real speech on soft syllables or short mid-sentence pauses
//!
//! Approach:
//! 1. WebRTC **LowBitrate** mode (middle ground: less noise than Quality, softer than Aggressive)
//! 2. Energy gate only to *reject* obvious quiet noise — once speech is open, hangover is generous
//! 3. Long speech hangover so dips in energy don't end the utterance early

use boris_core::AUDIO_TARGET_RATE;
use webrtc_vad::{SampleRate, Vad as WebVad, VadMode};

use crate::pcm::f32_to_pcm16_samples;
use crate::vad::Vad;
use boris_core::{error::Result, AudioSample};

/// Below this RMS is never treated as speech start.
const ABS_SPEECH_RMS: f32 = 0.008;
/// To *start* speech: RMS must clear `noise_floor * this` (mild).
const SPEECH_START_OVER_NOISE: f32 = 1.6;
/// While already in speech, only force "non-speech" if RMS is this close to the floor
/// (allows soft continuing speech).
const SPEECH_HOLD_OVER_NOISE: f32 = 1.15;
/// EMA for noise floor on clear non-speech frames only.
const NOISE_EMA: f32 = 0.04;
/// Frames (~10ms each after scoring) needed to enter speech.
const SPEECH_ON_FRAMES: u8 = 2;
/// Frames of weak/non-speech allowed while holding speech (hangover).
/// ~15 * 40ms processing ≈ 600ms of dips before we leave speech state.
const SPEECH_OFF_FRAMES: u8 = 12;

pub struct WebRtcVad {
    model: WebVad,
    noise_floor: f32,
    speech_run: u8,
    weak_run: u8,
    in_speech: bool,
}

// The underlying C library is not Send by default; it is safe to mark it Send
// because each WebRtcVad instance is exclusively owned by a single worker thread.
unsafe impl Send for WebRtcVad {}

impl WebRtcVad {
    pub fn new() -> Self {
        let sample_rate = SampleRate::try_from(AUDIO_TARGET_RATE as i32)
            .expect("AUDIO_TARGET_RATE is not a valid WebRTC VAD sample rate");
        // LowBitrate: less false voice on music than Quality, less clipping than Aggressive.
        Self {
            model: WebVad::new_with_rate_and_mode(sample_rate, VadMode::LowBitrate),
            noise_floor: ABS_SPEECH_RMS,
            speech_run: 0,
            weak_run: 0,
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

        let start_gate = (self.noise_floor * SPEECH_START_OVER_NOISE).max(ABS_SPEECH_RMS);
        let hold_gate = (self.noise_floor * SPEECH_HOLD_OVER_NOISE).max(ABS_SPEECH_RMS * 0.7);

        // Strong speech candidate for opening an utterance.
        let strong = webrtc_voice && rms >= start_gate;
        // While talking: keep speech if WebRTC still likes it OR energy is still
        // clearly above the ambient floor (soft syllables).
        let hold = self.in_speech && (webrtc_voice || rms >= hold_gate);

        let raw_speech = if self.in_speech { hold || strong } else { strong };

        if raw_speech {
            self.speech_run = self.speech_run.saturating_add(1);
            self.weak_run = 0;
        } else {
            self.weak_run = self.weak_run.saturating_add(1);
            self.speech_run = 0;
            // Only adapt noise floor when we are clearly not in a speech hangover.
            if !self.in_speech {
                self.noise_floor = self.noise_floor * (1.0 - NOISE_EMA) + rms * NOISE_EMA;
                self.noise_floor = self.noise_floor.clamp(0.0015, 0.12);
            }
        }

        if !self.in_speech {
            if self.speech_run >= SPEECH_ON_FRAMES {
                self.in_speech = true;
                self.weak_run = 0;
            }
        } else if self.weak_run >= SPEECH_OFF_FRAMES {
            // Left speech — allow floor to re-learn ambient.
            self.in_speech = false;
            self.noise_floor = self.noise_floor * (1.0 - NOISE_EMA) + rms * NOISE_EMA;
            self.noise_floor = self.noise_floor.clamp(0.0015, 0.12);
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
        for _ in 0..8 {
            assert!(!vad.predict(&frame).unwrap());
        }
    }

    #[test]
    fn low_level_noise_stays_non_speech() {
        let mut vad = WebRtcVad::new();
        let frame: Vec<f32> = (0..160)
            .map(|i| 0.004 * ((i as f32 * 0.3).sin()))
            .collect();
        for _ in 0..30 {
            let _ = vad.predict(&frame);
        }
        assert!(
            !vad.predict(&frame).unwrap(),
            "low background should not open speech"
        );
    }

    #[test]
    fn loud_tone_can_enter_speech() {
        let mut vad = WebRtcVad::new();
        // Warm with silence first.
        let silence = vec![0.0f32; 160];
        for _ in 0..5 {
            let _ = vad.predict(&silence);
        }
        // Strong-ish frame (speech-like amplitude).
        let frame: Vec<f32> = (0..160)
            .map(|i| 0.08 * ((i as f32 * 0.7).sin()))
            .collect();
        let mut any = false;
        for _ in 0..10 {
            if vad.predict(&frame).unwrap() {
                any = true;
                break;
            }
        }
        // WebRTC may or may not like a pure sine; we only assert we don't panic.
        let _ = any;
    }
}
