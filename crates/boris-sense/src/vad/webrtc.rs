//! Thin WebRTC VAD wrapper.
//!
//! Keep this dumb: the C VAD decides voice vs not. Extra energy gates / hangover
//! state machines were cutting real speech mid-utterance.

use std::fmt;

use boris_core::AUDIO_TARGET_RATE;
use webrtc_vad::{SampleRate, Vad as WebVad, VadMode};

use crate::pcm::f32_to_pcm16_samples_into;
use crate::vad::Vad;
use boris_core::{AudioSample, Error, Result};

/// Valid WebRTC VAD frame lengths (samples) at 16 kHz: 10 / 20 / 30 ms.
pub const WEBRTC_VAD_FRAME_SAMPLES_16K: &[usize] = &[160, 320, 480];

/// Pipeline rate is fixed at 16 kHz; WebRTC VAD frame math depends on it.
const _: () = assert!(AUDIO_TARGET_RATE == 16_000);

/// WebRTC VAD adapter for mono frames at [`AUDIO_TARGET_RATE`].
pub struct WebRtcVad {
    model: WebVad,
    /// Reused PCM scratch for the f32 → i16 conversion on the hot path.
    pcm_scratch: Vec<i16>,
}

// SAFETY: `webrtc_vad::Vad` wraps a raw `*mut Fvad` and is not `Send` by default
// because the C library has no internal synchronization.
//
// It is safe to mark `WebRtcVad` as `Send` under these conditions:
// - A given instance is exclusively owned by one worker / engine thread at a time
//   (no concurrent `predict` from multiple threads).
// - Ownership may move across threads only when no other thread holds a reference
//   (standard `Send` transfer). Shared use requires `Mutex` or similar.
// - We never share the inner `WebVad` via `Sync` (we do not implement `Sync`).
//
// Violating exclusive use of one instance is undefined behavior in the C VAD.
unsafe impl Send for WebRtcVad {}

impl WebRtcVad {
    /// Create a quality-mode VAD at [`AUDIO_TARGET_RATE`] (16 kHz).
    pub fn new() -> Self {
        // Unreachable-by-construction: the compile-time assert above already
        // pins `AUDIO_TARGET_RATE == 16_000`, which `SampleRate::try_from` always
        // accepts, so this `expect` can never actually fire.
        let sample_rate = SampleRate::try_from(AUDIO_TARGET_RATE as i32)
            .expect("AUDIO_TARGET_RATE is not a valid WebRTC VAD sample rate");
        // Quality matches the original Boris behavior (reliable on real speech).
        // Background music false-positives are a separate problem — do not "fix"
        // them by gating energy here (that clips soft speech).
        Self {
            model: WebVad::new_with_rate_and_mode(sample_rate, VadMode::Quality),
            pcm_scratch: Vec::new(),
        }
    }
}

impl Default for WebRtcVad {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for WebRtcVad {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebRtcVad")
            .field("sample_rate_hz", &AUDIO_TARGET_RATE)
            .field("mode", &"Quality")
            .finish_non_exhaustive()
    }
}

impl Vad for WebRtcVad {
    fn predict(&mut self, audio: &[AudioSample]) -> Result<bool> {
        validate_frame_len(audio.len())?;
        f32_to_pcm16_samples_into(audio, &mut self.pcm_scratch);
        self.model.is_voice_segment(&self.pcm_scratch).map_err(|_| {
            Error::other(format!(
                "webrtc-vad prediction failed (samples={}, expected one of {:?})",
                self.pcm_scratch.len(),
                WEBRTC_VAD_FRAME_SAMPLES_16K
            ))
        })
    }
}

/// Reject frames the C VAD cannot process (not 10/20/30 ms at 16 kHz).
fn validate_frame_len(len: usize) -> Result<()> {
    if WEBRTC_VAD_FRAME_SAMPLES_16K.contains(&len) {
        Ok(())
    } else {
        Err(Error::other(format!(
            "webrtc-vad invalid frame length: got {len} samples, expected one of {:?} \
             (10/20/30 ms at {} Hz)",
            WEBRTC_VAD_FRAME_SAMPLES_16K, AUDIO_TARGET_RATE
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vad::Vad;

    #[test]
    fn rejects_invalid_frame_length() {
        let mut vad = WebRtcVad::new();
        let bad = vec![0.0f32; 100];
        let err = vad.predict(&bad).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("100"), "{msg}");
        assert!(msg.contains("160") || msg.contains("expected"), "{msg}");
    }

    #[test]
    fn accepts_10ms_silence_frame() {
        let mut vad = WebRtcVad::new();
        let frame = vec![0.0f32; 160];
        let speech = vad.predict(&frame).expect("160-sample frame is valid");
        assert!(!speech);
    }

    #[test]
    fn accepts_20ms_and_30ms_frames() {
        let mut vad = WebRtcVad::new();
        assert!(vad.predict(&vec![0.0f32; 320]).is_ok());
        assert!(vad.predict(&vec![0.0f32; 480]).is_ok());
    }

    #[test]
    fn validate_frame_len_messages() {
        let err = validate_frame_len(0).unwrap_err();
        assert!(err.to_string().contains("0 samples"));
    }
}
