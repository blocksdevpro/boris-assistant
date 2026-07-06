use std::{
    sync::mpsc::{Receiver, Sender},
    thread::{self, JoinHandle},
    time::Instant,
};

use boris_audio::buffer::SlidingBuffer;
use boris_core::{
    AudioBuffer,
    event::Event,
    types::{ArcAudioBuffer, Lifecycle},
};
use boris_inference::{
    SpeechToText, VAD_INITIAL_TIMEOUT, VAD_SILENCE_WINDOW, VAD_WINDOW_SIZE, Vad,
    WAKEWORD_PROCESSING_INTERVAL, WAKEWORD_THRESHOLD, WAKEWORD_WINDOW_SIZE, WakeWord,
};

pub enum STTCommand {
    LoadModel,
    Transcribe(AudioBuffer),
}

pub struct STTWorker {
    _handle: JoinHandle<()>,
}

impl STTWorker {
    pub fn spawn(
        command_rx: Receiver<STTCommand>,
        event_tx: Sender<Event>,
        mut stt: impl SpeechToText + 'static,
    ) -> Self {
        let handle = thread::spawn(move || {
            while let Ok(cmd) = command_rx.recv() {
                match cmd {
                    STTCommand::LoadModel => {
                        tracing::debug!("Loading STT model into memory...");
                        stt.load().ok();
                        tracing::debug!("STT model loaded successfully.");
                    }
                    STTCommand::Transcribe(audio) => {
                        if let Ok(text) = stt.transcribe(&audio) {
                            event_tx.send(Event::SpeechToTextResult(text)).ok();
                        }
                        tracing::debug!("Unloading STT model from memory...");
                        stt.unload().ok();
                    }
                }
            }
        });
        Self { _handle: handle }
    }
}

pub struct VADWorker {
    _handle: JoinHandle<()>,
}

impl VADWorker {
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
                while let Ok(command) = control_rx.try_recv() {
                    match command {
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

                // If the channel is closed (returns Err), break the loop!
                let Ok(audio) = audio_rx.recv() else {
                    break;
                };
                if !is_listening {
                    continue;
                }
                audio_buffer.extend_from_slice(&audio);
                while audio_buffer.len() >= VAD_WINDOW_SIZE {
                    let chunk: Vec<f32> = audio_buffer.drain(..VAD_WINDOW_SIZE).collect();

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
                                event_tx.send(Event::SpeechEnded).ok();
                            }
                        }
                    }
                }
            }
        });
        Self { _handle: handle }
    }
}

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
        let handle = std::thread::spawn(move || {
            let mut is_listening = true;
            let mut last_processing_time = Instant::now();
            let mut audio_buffer = SlidingBuffer::new(WAKEWORD_WINDOW_SIZE);

            loop {
                while let Ok(command) = control_rx.try_recv() {
                    match command {
                        Lifecycle::Start => is_listening = true,
                        Lifecycle::Stop => is_listening = false,
                    }
                }
                //Break if the channel closes.
                let Ok(audio) = audio_rx.recv() else {
                    break;
                };
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

                    tracing::debug!(
                        "Wakeword score: {:.3} ({}ms)",
                        result,
                        last_processing_time.elapsed().as_millis()
                    );
                    if result >= WAKEWORD_THRESHOLD {
                        event_tx.send(Event::WakeWordDetected).unwrap();
                    }
                    last_processing_time = Instant::now();
                }
            }
        });

        Self { _handle: handle }
    }
}
