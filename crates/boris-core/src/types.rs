use std::sync::Arc;

pub type AudioSample = f32;
pub type AudioBuffer = Vec<AudioSample>;

pub type ArcAudioBuffer = Arc<[AudioSample]>;
