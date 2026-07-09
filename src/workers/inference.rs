use std::{
    sync::mpsc::{Receiver, Sender},
    thread::{self, JoinHandle},
    time::Instant,
};

use boris_audio::buffer::SlidingBuffer;
use boris_core::{
    event::Event,
    types::{ArcAudioBuffer, Lifecycle},
    AudioBuffer, ServiceKind, TurnId,
};
use boris_inference::{
    vad_initial_timeout_samples, vad_silence_samples, SpeechToText, Vad, WakeWord, VAD_WINDOW_SIZE,
    WAKEWORD_PROCESSING_INTERVAL, WAKEWORD_THRESHOLD, WAKEWORD_WINDOW_SIZE,
};

// ── Commands ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SttCommand {
    /// Prime the STT model so transcription is fast when audio arrives.
    LoadModel,
    /// Transcribe the given audio buffer and emit [`Event::SpeechToTextResult`].
    Transcribe { turn: TurnId, audio: AudioBuffer },
}

// ── STT Worker ────────────────────────────────────────────────────────────────

pub struct SttWorker {
    _handle: JoinHandle<()>,
}

impl SttWorker {
    pub fn spawn(
        command_rx: Receiver<SttCommand>,
        event_tx: Sender<Event>,
        mut stt: impl SpeechToText + 'static,
    ) -> Self {
        let handle = thread::spawn(move || {
            while let Ok(cmd) = command_rx.recv() {
                match cmd {
                    SttCommand::LoadModel => {
                        tracing::debug!("SttWorker: loading model");
                        if let Err(e) = stt.load() {
                            tracing::error!(error = %e, "SttWorker: failed to load model");
                            event_tx
                                .send(Event::WorkerError {
                                    turn: None,
                                    worker: "SttWorker",
                                    kind: ServiceKind::Stt,
                                    message: e.to_string(),
                                })
                                .ok();
                        }
                    }
                    SttCommand::Transcribe { turn, audio } => {
                        match stt.transcribe(&audio) {
                            Ok(text) => {
                                event_tx.send(Event::SpeechToTextResult { turn, text }).ok();
                            }
                            Err(e) => {
                                tracing::error!(error = %e, %turn, "SttWorker: transcription failed");
                                event_tx
                                    .send(Event::WorkerError {
                                        turn: Some(turn),
                                        worker: "SttWorker",
                                        kind: ServiceKind::Stt,
                                        message: e.to_string(),
                                    })
                                    .ok();
                            }
                        }
                        if let Err(e) = stt.unload() {
                            tracing::warn!(error = %e, "SttWorker: failed to unload model");
                        }
                    }
                }
            }
        });
        Self { _handle: handle }
    }
}

// ── VAD Worker ────────────────────────────────────────────────────────────────

pub struct VadWorker {
    _handle: JoinHandle<()>,
}

impl VadWorker {
    pub fn spawn(
        audio_rx: Receiver<ArcAudioBuffer>,
        control_rx: Receiver<Lifecycle>,
        event_tx: Sender<Event>,
        mut detector: impl Vad + 'static,
    ) -> Self {
        let handle = thread::spawn(move || {
            let mut has_spoken = false;
            let mut is_listening = false;
            let mut audio_buffer: Vec<f32> = Vec::new();
            let mut samples_since_speech: usize = 0;

            let silence_after_speech = vad_silence_samples();
            let silence_before_speech = vad_initial_timeout_samples();

            loop {
                while let Ok(cmd) = control_rx.try_recv() {
                    match cmd {
                        Lifecycle::Start => {
                            is_listening = true;
                            has_spoken = false;
                            samples_since_speech = 0;
                            audio_buffer.clear();
                        }
                        Lifecycle::Stop => {
                            is_listening = false;
                        }
                    }
                }

                let Ok(audio) = audio_rx.recv() else { break };

                if !is_listening {
                    continue;
                }

                audio_buffer.extend_from_slice(&audio);

                while audio_buffer.len() >= VAD_WINDOW_SIZE {
                    let chunk: Vec<f32> = audio_buffer.drain(..VAD_WINDOW_SIZE).collect();

                    match detector.predict(&chunk) {
                        Ok(true) => {
                            has_spoken = true;
                            samples_since_speech = 0;
                        }
                        Ok(false) => {
                            samples_since_speech = samples_since_speech.saturating_add(chunk.len());
                            let limit = if has_spoken {
                                silence_after_speech
                            } else {
                                silence_before_speech
                            };

                            if samples_since_speech >= limit {
                                is_listening = false;
                                audio_buffer.clear();
                                samples_since_speech = 0;
                                event_tx.send(Event::SpeechEnded).ok();
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "VadWorker: prediction failed");
                        }
                    }
                }
            }
        });
        Self { _handle: handle }
    }
}

// ── WakeWord Worker ───────────────────────────────────────────────────────────

pub struct WakeWordWorker {
    _handle: JoinHandle<()>,
}

impl WakeWordWorker {
    pub fn spawn(
        audio_rx: Receiver<ArcAudioBuffer>,
        control_rx: Receiver<Lifecycle>,
        event_tx: Sender<Event>,
        mut detector: impl WakeWord + 'static,
    ) -> Self {
        let handle = thread::spawn(move || {
            let mut is_listening = true;
            let mut last_processed = Instant::now();
            let mut audio_buffer = SlidingBuffer::new(WAKEWORD_WINDOW_SIZE);

            loop {
                while let Ok(cmd) = control_rx.try_recv() {
                    match cmd {
                        Lifecycle::Start => is_listening = true,
                        Lifecycle::Stop => is_listening = false,
                    }
                }

                let Ok(audio) = audio_rx.recv() else { break };
                audio_buffer.push(&audio);

                if !is_listening {
                    continue;
                }

                if last_processed.elapsed() >= WAKEWORD_PROCESSING_INTERVAL && audio_buffer.ready()
                {
                    let window = audio_buffer.read();

                    match detector.predict(&window) {
                        Ok(score) => {
                            tracing::debug!(
                                score,
                                elapsed_ms = last_processed.elapsed().as_millis(),
                                "wakeword score"
                            );
                            if score >= WAKEWORD_THRESHOLD {
                                event_tx.send(Event::WakeWordDetected).ok();
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "WakeWordWorker: prediction failed");
                        }
                    }

                    last_processed = Instant::now();
                }
            }
        });

        Self { _handle: handle }
    }
}
