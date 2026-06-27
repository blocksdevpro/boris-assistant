use boris_audio::AUDIO_TARGET_RATE;

use webrtc_vad::SampleRate;
use webrtc_vad::Vad as WebVad;

use crate::AudioSample;
use crate::Result;
use crate::Vad;
use crate::f32_to_pcm16_samples;

pub enum VadResult {
    Speech,
    Silence,
}

pub enum VadCommand {
    StartListening,
    StopListening,
}

pub struct WebRtcVad {
    model: WebVad,
}

unsafe impl Send for WebRtcVad {}

impl WebRtcVad {
    pub fn new() -> Self {
        let sample_rate =
            SampleRate::try_from(AUDIO_TARGET_RATE as i32).expect("[ERROR] invalid sample_rate");
        let model = WebVad::new_with_rate(sample_rate);
        Self { model }
    }
}

impl Vad for WebRtcVad {
    fn predict(&mut self, audio: &[AudioSample]) -> Result<bool> {
        let pcm_samples = f32_to_pcm16_samples(audio);
        let result = self
            .model
            .is_voice_segment(&pcm_samples)
            .expect("[ERROR] vad predict");
        Ok(result)
    }
}
