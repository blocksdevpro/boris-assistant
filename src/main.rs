use std::sync::mpsc;

use boris_audio::{AUDIO_TARGET_RATE, pipeline::AudioPipeline, processor::AudioProcessor};
use boris_core::{AudioSampleBuffer, event::BorisEvent};
use boris_inference::wakeword::{BorisWakeWord, BorisWakeWordProcessor};

static WAKEWORD_MODEL_BYTES: &[u8] = include_bytes!("../assets/models/livekit/boris-large.onnx");

fn main() {
    let (audio_tx, audio_rx) = mpsc::channel::<AudioSampleBuffer>();
    let (event_tx, event_rx) = mpsc::channel::<BorisEvent>();

    let (wakeword_audio_tx, wakeword_audio_rx) = mpsc::channel::<AudioSampleBuffer>();

    let _audio_pipeline = AudioPipeline::spawn(audio_tx);
    let wakeword = BorisWakeWord::new("boris", WAKEWORD_MODEL_BYTES, AUDIO_TARGET_RATE);
    let _audio_processor = AudioProcessor::spawn(audio_rx, vec![wakeword_audio_tx]);
    let _wakeword_processor = BorisWakeWordProcessor::spawn(wakeword_audio_rx, event_tx, wakeword);

    loop {
        if let Ok(event) = event_rx.recv() {
            match event {
                BorisEvent::WakeWordDetected => {
                    println!("[BORIS] wakeword detected");
                }
            }
        }
    }
}
