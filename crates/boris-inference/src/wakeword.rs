use std::{
    sync::mpsc::{Receiver, Sender},
    thread::JoinHandle,
    time::Instant,
};

use crate::{
    WAKEWORD_PROCESSING_INTERVAL, WAKEWORD_THRESHOLD, WAKEWORD_WINDOW_SIZE, WakeWordDetector,
    f32_to_pcm16_samples,
};
use boris_audio::buffer::AudioSlidingBuffer;
use livekit_wakeword::WakeWordModel;

use boris_core::{AudioSample, AudioSampleBuffer, error::BorisResult, event::BorisEvent};

pub struct BorisWakeWord {
    model: WakeWordModel,
}

pub enum WakeWordCommand {
    StartListening,
    StopListening,
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
        let pcm_samples = f32_to_pcm16_samples(audio);
        let result = self.model.predict(&pcm_samples).unwrap();

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
        control_rx: Receiver<WakeWordCommand>,
        event_tx: Sender<BorisEvent>,
        mut detector: impl WakeWordDetector + 'static,
    ) -> Self {
        let handle = std::thread::spawn(move || {
            let mut is_listening = true;
            let mut last_processing_time = Instant::now();
            let mut audio_buffer = AudioSlidingBuffer::new(WAKEWORD_WINDOW_SIZE);

            loop {
                while let Ok(command) = control_rx.try_recv() {
                    match command {
                        WakeWordCommand::StartListening => is_listening = true,
                        WakeWordCommand::StopListening => is_listening = false,
                    }
                }
                if let Ok(audio) = audio_rx.recv() {
                    audio_buffer.push(&audio);

                    if !is_listening {
                        continue;
                    }

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
