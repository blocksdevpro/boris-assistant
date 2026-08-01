use boris_core::AUDIO_TARGET_RATE;
use webrtc_vad::{SampleRate, Vad as WebVad};

use crate::pcm::f32_to_pcm16_samples;
use crate::vad::Vad;
use boris_core::{error::Result, AudioSample};

pub struct WebRtcVad {
    model: WebVad,
}

// The underlying C library is not Send by default; it is safe to mark it Send
// because each WebRtcVad instance is exclusively owned by a single worker thread.
unsafe impl Send for WebRtcVad {}

impl WebRtcVad {
    pub fn new() -> Self {
        let sample_rate = SampleRate::try_from(AUDIO_TARGET_RATE as i32)
            .expect("AUDIO_TARGET_RATE is not a valid WebRTC VAD sample rate");
        Self {
            model: WebVad::new_with_rate(sample_rate),
        }
    }
}

impl Default for WebRtcVad {
    fn default() -> Self {
        Self::new()
    }
}

impl Vad for WebRtcVad {
    fn predict(&mut self, audio: &[AudioSample]) -> Result<bool> {
        let pcm = f32_to_pcm16_samples(audio);
        self.model
            .is_voice_segment(&pcm)
            .map_err(|_| boris_core::error::Error::Other("webrtc-vad prediction failed".into()))
    }
}
