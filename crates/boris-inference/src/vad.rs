use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::thread;
use std::thread::JoinHandle;
use std::time::Instant;

use boris_audio::AUDIO_TARGET_RATE;
use boris_core::event::BorisEvent;

use boris_core::types::ArcAudioBuffer;
use webrtc_vad::SampleRate;
use webrtc_vad::Vad;

use crate::AudioSample;
use crate::BorisResult;
use crate::VAD_INITIAL_TIMEOUT;
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
        audio_rx: Receiver<ArcAudioBuffer>,
        control_rx: Receiver<VadCommand>,
        event_tx: Sender<BorisEvent>,
        mut detector: impl VoiceActivityDetector + 'static,
    ) -> Self {
        let handle = thread::spawn(move || {
            let mut is_listening = false;
            let mut last_speech_time = Instant::now();
            let mut has_spoken = false;

            let mut audio_buffer: Vec<f32> = Vec::new();
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

                // If the channel is closed (returns Err), break the loop!
                let Ok(audio) = audio_rx.recv() else {
                    break;
                };
                if !is_listening {
                    continue;
                }

                audio_buffer.extend_from_slice(&audio);
                while audio_buffer.len() >= VAD_WINDOW_SIZE as usize {
                    let chunk: Vec<f32> = audio_buffer.drain(..VAD_WINDOW_SIZE as usize).collect();

                    if let Ok(result) = detector.predict(&chunk) {
                        if result {
                            has_spoken = true;
                            last_speech_time = Instant::now();
                        } else {
                            let threshold = if has_spoken {
                                VAD_SILENCE_WINDOW.as_millis()
                            } else {
                                VAD_INITIAL_TIMEOUT.as_millis()
                            };

                            if last_speech_time.elapsed().as_millis() >= threshold {
                                is_listening = false;
                                audio_buffer.clear();
                                event_tx.send(BorisEvent::SpeechEnded).ok();
                            }
                        }
                    }
                }
            }
        });
        Self { _handle: handle }
    }
}
