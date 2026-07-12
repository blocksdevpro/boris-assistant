use std::{
    sync::mpsc::{Receiver, Sender},
    thread::{self, JoinHandle},
    time::Instant,
};

use boris_audio::buffer::SlidingBuffer;
use boris_core::{
    event::Event,
    types::{ArcAudioBuffer, Lifecycle},
    AudioBuffer, ServiceKind, TurnId, AUDIO_TARGET_RATE,
};
use boris_inference::{
    duration_to_samples, vad_initial_timeout_samples, vad_silence_samples, SpeechToText, Vad,
    WakeWord, VAD_PROCESSING_INTERVAL, VAD_WINDOW_SIZE, WAKEWORD_PROCESSING_INTERVAL,
    WAKEWORD_THRESHOLD, WAKEWORD_WINDOW_SIZE,
};

// ── Commands ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SttCommand {
    /// Load the STT model early (e.g. on wake) so `Transcribe` is faster.
    LoadModel,
    /// Transcribe `audio` for `turn` and emit [`Event::SpeechToTextResult`].
    /// Unloads the model after each successful/failed transcription.
    Transcribe { turn: TurnId, audio: AudioBuffer },
}

// ── STT Worker ────────────────────────────────────────────────────────────────

/// Runs [`SpeechToText`] on a background thread.
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

// ── Endpoint sensor (VAD) ─────────────────────────────────────────────────────

/// Watches the audio bus while listening and emits [`Event::SpeechEnded`] when
/// silence lasts long enough in **audio time** (sample counts, not wall clock).
///
/// Frames are still sized for WebRTC VAD ([`VAD_WINDOW_SIZE`]), but
/// [`Vad::predict`] runs only every [`VAD_PROCESSING_INTERVAL`] of audio.
/// Enable/disable via [`Lifecycle`]; Session owns when that happens.
pub struct EndpointSensor {
    _handle: JoinHandle<()>,
}

impl EndpointSensor {
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
            // WebRTC VAD frames stay at VAD_WINDOW_SIZE (10 ms); only *score* every
            // VAD_PROCESSING_INTERVAL of audio time (same idea as WakeSensor).
            let score_every = duration_to_samples(VAD_PROCESSING_INTERVAL, AUDIO_TARGET_RATE);
            let mut samples_since_score: usize = 0;

            let silence_after_speech = vad_silence_samples();
            let silence_before_speech = vad_initial_timeout_samples();

            loop {
                while let Ok(cmd) = control_rx.try_recv() {
                    match cmd {
                        Lifecycle::Start => {
                            is_listening = true;
                            has_spoken = false;
                            samples_since_speech = 0;
                            samples_since_score = 0;
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
                    samples_since_score = samples_since_score.saturating_add(chunk.len());

                    // Still drain 10 ms frames so the buffer does not grow, but only
                    // call the model once per VAD_PROCESSING_INTERVAL of audio.
                    if samples_since_score < score_every {
                        continue;
                    }
                    samples_since_score = 0;

                    match detector.predict(&chunk) {
                        Ok(true) => {
                            has_spoken = true;
                            samples_since_speech = 0;
                        }
                        Ok(false) => {
                            // Advance silence by the scoring hop, not the 10 ms frame.
                            samples_since_speech = samples_since_speech.saturating_add(score_every);
                            let limit = if has_spoken {
                                silence_after_speech
                            } else {
                                silence_before_speech
                            };

                            if samples_since_speech >= limit {
                                is_listening = false;
                                audio_buffer.clear();
                                samples_since_speech = 0;
                                samples_since_score = 0;
                                event_tx.send(Event::SpeechEnded).ok();
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "EndpointSensor: prediction failed");
                        }
                    }
                }
            }
        });
        Self { _handle: handle }
    }
}

// ── Wake sensor ───────────────────────────────────────────────────────────────

/// Scores the wakeword on a sliding window; emits [`Event::WakeWordDetected`]
/// when the score crosses the threshold.
///
/// Cadence and window sizes come from `boris_inference` constants. Session
/// decides whether wake hits are legal (Idle only).
pub struct WakeSensor {
    _handle: JoinHandle<()>,
}

impl WakeSensor {
    pub fn spawn(
        audio_rx: Receiver<ArcAudioBuffer>,
        control_rx: Receiver<Lifecycle>,
        event_tx: Sender<Event>,
        mut detector: impl WakeWord + 'static,
    ) -> Self {
        let handle = thread::spawn(move || {
            let mut is_listening = true;
            let mut audio_buffer = SlidingBuffer::new(WAKEWORD_WINDOW_SIZE);
            let score_every = duration_to_samples(WAKEWORD_PROCESSING_INTERVAL, AUDIO_TARGET_RATE);
            let mut samples_since_score: usize = 0;

            loop {
                while let Ok(cmd) = control_rx.try_recv() {
                    match cmd {
                        Lifecycle::Start => is_listening = true,
                        Lifecycle::Stop => is_listening = false,
                    }
                }

                let Ok(audio) = audio_rx.recv() else { break };
                audio_buffer.push(&audio);
                samples_since_score = samples_since_score.saturating_add(audio.len());

                if !is_listening {
                    continue;
                }

                if samples_since_score >= score_every && audio_buffer.ready() {
                    samples_since_score = 0;
                    let window = audio_buffer.read();

                    tracing::debug!("WW predict {:?}", Instant::now());
                    match detector.predict(&window) {
                        Ok(score) => {
                            if score >= WAKEWORD_THRESHOLD {
                                event_tx.send(Event::WakeWordDetected).ok();
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "
WakeSensor: prediction failed");
                        }
                    }
                }
            }
        });

        Self { _handle: handle }
    }
}
