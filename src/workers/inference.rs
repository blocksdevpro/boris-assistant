use std::{
    sync::mpsc::{Receiver, Sender},
    thread::{self, JoinHandle},
    time::Instant,
};

use boris_audio::buffer::SlidingBuffer;
use boris_core::{
    event::Event,
    types::{ArcAudioBuffer, Lifecycle},
    AudioBuffer,
};
use boris_inference::{
    SpeechToText, Vad, WakeWord, VAD_INITIAL_TIMEOUT, VAD_SILENCE_WINDOW, VAD_WINDOW_SIZE,
    WAKEWORD_PROCESSING_INTERVAL, WAKEWORD_THRESHOLD, WAKEWORD_WINDOW_SIZE,
};

// ── Commands ──────────────────────────────────────────────────────────────────
#[derive(Debug)]
pub enum SttCommand {
    /// Prime the STT model so transcription is fast when audio arrives.
    LoadModel,
    /// Transcribe the given audio buffer and emit [`Event::SpeechToTextResult`].
    Transcribe(AudioBuffer),
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
                                    worker: "SttWorker",
                                    message: e.to_string(),
                                })
                                .ok();
                        }
                    }
                    SttCommand::Transcribe(audio) => {
                        match stt.transcribe(&audio) {
                            Ok(text) => {
                                event_tx.send(Event::SpeechToTextResult(text)).ok();
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "SttWorker: transcription failed");
                                event_tx
                                    .send(Event::WorkerError {
                                        worker: "SttWorker",
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
            let mut is_listening = false;
            let mut last_speech_time = Instant::now();
            let mut has_spoken = false;
            let mut audio_buffer: Vec<f32> = Vec::new();

            loop {
                // Drain all pending control signals first.
                while let Ok(cmd) = control_rx.try_recv() {
                    match cmd {
                        Lifecycle::Start => {
                            is_listening = true;
                            has_spoken = false;
                            last_speech_time = Instant::now();
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
                        Ok(is_speech) => {
                            if is_speech {
                                has_spoken = true;
                                last_speech_time = Instant::now();
                            } else {
                                let silence_threshold = if has_spoken {
                                    VAD_SILENCE_WINDOW
                                } else {
                                    VAD_INITIAL_TIMEOUT
                                };

                                if last_speech_time.elapsed() >= silence_threshold {
                                    is_listening = false;
                                    audio_buffer.clear();
                                    event_tx.send(Event::SpeechEnded).ok();
                                }
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
                // Drain all pending control signals first.
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
