use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::thread;
use std::thread::JoinHandle;
use std::time::Instant;

use boris_audio::AUDIO_TARGET_RATE;
use boris_core::AudioSampleBuffer;
use boris_core::event::BorisEvent;

use webrtc_vad::SampleRate;
use webrtc_vad::Vad;

use crate::AudioSample;
use crate::BorisResult;
use crate::VAD_INITIAL_TIMEOUT;
use crate::VAD_PROCESSING_INTERVAL;
use crate::VAD_SILENCE_WINDOW;
use crate::VAD_WINDOW_SIZE;
use crate::VoiceActivityDetector;
use crate::f32_to_pcm16_samples;

pub enum VadResult {
    Speech,
    Silence,
}

pub enum VadCommand {
    StartListening,
    StopListening,
}

pub struct BorisVad {
    model: Vad,
}

unsafe impl Send for BorisVad {}

impl BorisVad {
    pub fn new() -> Self {
        let sample_rate =
            SampleRate::try_from(AUDIO_TARGET_RATE as i32).expect("[ERROR] invalid sample_rate");
        let model = Vad::new_with_rate(sample_rate);
        Self { model }
    }
}

impl VoiceActivityDetector for BorisVad {
    fn predict(&mut self, audio: &[AudioSample]) -> BorisResult<bool> {
        let pcm_samples = f32_to_pcm16_samples(audio);
        let result = self
            .model
            .is_voice_segment(&pcm_samples)
            .expect("[ERROR] vad predict");
        Ok(result)
    }
}

pub struct BorisVadProcessor {
    _handle: JoinHandle<()>,
}

impl BorisVadProcessor {
    pub fn spawn(
        audio_rx: Receiver<AudioSampleBuffer>,
        control_rx: Receiver<VadCommand>,
        event_tx: Sender<BorisEvent>,
        mut detector: impl VoiceActivityDetector + 'static,
    ) -> Self {
        let handle = thread::spawn(move || {
            let mut last_processing_time = Instant::now();
            let mut is_listening = false;
            let mut last_speech_time = Instant::now();
            let mut has_spoken = false;
            loop {
                // should i clear all pending commands here?
                while let Ok(command) = control_rx.try_recv() {
                    match command {
                        VadCommand::StartListening => {
                            is_listening = true;
                            has_spoken = false;
                            last_speech_time = Instant::now();
                        }
                        VadCommand::StopListening => is_listening = false,
                    }
                }
                if let Ok(audio) = audio_rx.recv() {
                    if audio.len() >= VAD_WINDOW_SIZE as usize && is_listening {
                        if last_processing_time.elapsed().as_millis()
                            >= VAD_PROCESSING_INTERVAL.as_millis()
                        {
                            // Predict whether the audio is voice or silence
                            if let Ok(result) = detector.predict(&audio[..VAD_WINDOW_SIZE]) {
                                if result == true {
                                    has_spoken = true;
                                    last_speech_time = Instant::now();
                                } else if result == false {
                                    let threshold = if has_spoken {
                                        VAD_SILENCE_WINDOW.as_millis()
                                    } else {
                                        VAD_INITIAL_TIMEOUT.as_millis()
                                    };

                                    if last_speech_time.elapsed().as_millis() >= threshold {
                                        is_listening = false;
                                        event_tx.send(BorisEvent::SpeechEnded).ok();
                                    }
                                }
                            }
                            last_processing_time = Instant::now();
                        }
                    }
                }
            }
        });
        Self { _handle: handle }
    }
}
