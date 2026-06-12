use std::sync::mpsc;

use boris_audio::pipeline::AudioPipeline;
use boris_core::{
    AudioSampleBuffer,
    event::{BorisEvent, InferenceEvent},
};

fn main() {
    let (audio_tx, audio_rx) = mpsc::channel::<AudioSampleBuffer>();
    let (inference_tx, inference_rx) = mpsc::sync_channel::<InferenceEvent>(1);
    let (event_tx, event_rx) = mpsc::channel::<BorisEvent>();

    let _pipeline = AudioPipeline::spawn(audio_tx);
}
