use std::{
    sync::mpsc::{Receiver, Sender},
    thread::JoinHandle,
    time::Instant,
};

use crate::{
    WAKEWORD_PROCESSING_INTERVAL, WAKEWORD_THRESHOLD, WAKEWORD_WINDOW_SIZE, WakeWordDetector,
};
use boris_audio::buffer::AudioSlidingBuffer;
use livekit_wakeword::WakeWordModel;

use boris_core::{AudioSample, AudioSampleBuffer, error::BorisResult, event::BorisEvent};

pub struct BorisWakeWord {
    model: WakeWordModel,
}

impl BorisWakeWord {
    pub fn new(model_name: &str, model_bytes: &[u8], sample_rate: u32) -> Self {
        Self {
            model: WakeWordModel::with_bytes(model_name, model_bytes, sample_rate).unwrap(),
        }
    }
}

impl WakeWordDetector for BorisWakeWord {
    fn predict(&mut self, audio: &[AudioSample]) -> BorisResult<f32> {
        // convert the f32 audio to i16 audio;
        let audio_i16: Vec<i16> = audio.iter().map(|&x| (x * 32767.0) as i16).collect();
        let result = self.model.predict(&audio_i16).unwrap();

        Ok(result.values().copied().next().unwrap_or(0.0))
    }
}

pub struct BorisWakeWordProcessor {
    _handle: JoinHandle<()>,
}
//
//
//
//
impl BorisWakeWordProcessor {
    pub fn spawn(
        audio_rx: Receiver<AudioSampleBuffer>,
        event_tx: Sender<BorisEvent>,
        mut detector: impl WakeWordDetector + 'static,
    ) -> Self {
        let handle = std::thread::spawn(move || {
            let mut last_processing_time = Instant::now();
            let mut audio_buffer = AudioSlidingBuffer::new(WAKEWORD_WINDOW_SIZE);

            loop {
                if let Ok(audio) = audio_rx.recv() {
                    audio_buffer.push(&audio);

                    if last_processing_time.elapsed().as_millis()
                        >= WAKEWORD_PROCESSING_INTERVAL.as_millis()
                        && audio_buffer.ready()
                    {
                        let audio = audio_buffer.read();

                        let result = detector.predict(&audio).unwrap();

                        println!(
                            "[BORIS] score: {result}, took {}ms",
                            last_processing_time.elapsed().as_millis()
                        );
                        if result >= WAKEWORD_THRESHOLD {
                            event_tx.send(BorisEvent::WakeWordDetected).unwrap();
                        }
                        last_processing_time = Instant::now();
                    }
                }
            }
        });

        Self { _handle: handle }
    }
}
