use std::{sync::mpsc, time::Instant};

use boris_audio::{AUDIO_TARGET_RATE, buffer::AudioSlidingBuffer, pipeline::AudioPipeline};
use boris_core::AudioSampleBuffer;
use boris_inference::{WakeWordDetector, wakeword::BorisWakeWord};

static WAKEWORD_MODEL_BYTES: &[u8] = include_bytes!("../assets/models/livekit/boris-large.onnx");

fn main() {
    let (audio_tx, audio_rx) = mpsc::channel::<AudioSampleBuffer>();

    let _pipeline = AudioPipeline::spawn(audio_tx);
    let mut wakeword = BorisWakeWord::new("boris", WAKEWORD_MODEL_BYTES, AUDIO_TARGET_RATE);
    let mut sliding_buffer = AudioSlidingBuffer::new(AUDIO_TARGET_RATE as usize * 2);

    let mut last_processed = Instant::now();
    loop {
        if let Ok(audio) = audio_rx.recv() {
            sliding_buffer.push(&audio);
            if last_processed.elapsed() >= std::time::Duration::from_millis(80)
                && sliding_buffer.ready()
            {
                last_processed = Instant::now();
                let audio = sliding_buffer.read();

                if let Ok(result) = wakeword.predict(&audio) {
                    if result > 0.5 {
                        println!("[BORIS] wakeword detected, score: {}", result);
                    }
                }
                println!(
                    "[DEBUG] took {} ms to process wakeword.",
                    last_processed.elapsed().as_millis()
                )
            }
        }
    }
}
