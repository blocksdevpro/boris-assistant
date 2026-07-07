use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread::{self, JoinHandle};

use boris_audio::{AUDIO_CHUNK_SIZE, AUDIO_TARGET_RATE, buffer::RecordingBuffer, capture::Capture, resampler::Resampler};
use boris_core::{AudioBuffer, AudioSample, error::Result, event::Event, types::{ArcAudioBuffer, Lifecycle}};

// ── Audio Pipeline Worker ─────────────────────────────────────────────────────

/// Captures raw audio from the microphone, resamples it to [`AUDIO_TARGET_RATE`],
/// and emits fixed-size chunks on `audio_tx`.
pub struct AudioPipelineWorker {
    _handle: JoinHandle<()>,
    _capture: Capture,
}

impl AudioPipelineWorker {
    pub fn spawn(audio_tx: Sender<AudioBuffer>) -> Result<Self> {
        let (raw_tx, raw_rx) = crossbeam_channel::bounded::<AudioBuffer>(100);

        let capture = Capture::new(raw_tx)?;
        let mut resampler = Resampler::new(1, capture.sample_rate, AUDIO_TARGET_RATE);

        let handle = thread::spawn(move || {
            let mut accumulator: Vec<AudioSample> = Vec::with_capacity(AUDIO_CHUNK_SIZE as usize);

            loop {
                let chunk = match raw_rx.recv() {
                    Ok(c) => c,
                    Err(_) => {
                        tracing::warn!("AudioPipelineWorker: raw audio channel closed");
                        break;
                    }
                };

                accumulator.extend_from_slice(&chunk);

                while accumulator.len() >= AUDIO_CHUNK_SIZE as usize {
                    let to_resample: Vec<AudioSample> =
                        accumulator.drain(..AUDIO_CHUNK_SIZE as usize).collect();

                    match resampler.resample(&to_resample) {
                        Ok(resampled) => {
                            if audio_tx.send(resampled).is_err() {
                                tracing::warn!("AudioPipelineWorker: downstream channel closed");
                                return;
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "AudioPipelineWorker: resample failed");
                        }
                    }
                }
            }
        });

        Ok(Self {
            _handle: handle,
            _capture: capture,
        })
    }
}

// ── Audio Dispatcher Worker ───────────────────────────────────────────────────

/// Fans a single [`AudioBuffer`] stream out to multiple subscribers by wrapping
/// each chunk in an [`Arc`] and cloning the pointer (zero-copy).
pub struct AudioDispatcherWorker {
    _handle: JoinHandle<()>,
}

impl AudioDispatcherWorker {
    pub fn spawn(audio_rx: Receiver<AudioBuffer>, subscribers: Vec<Sender<ArcAudioBuffer>>) -> Self {
        let handle = thread::spawn(move || {
            while let Ok(audio) = audio_rx.recv() {
                let shared: ArcAudioBuffer = Arc::from(audio);
                for tx in &subscribers {
                    // Best-effort delivery — a slow subscriber won't block others.
                    tx.send(shared.clone()).ok();
                }
            }
        });
        Self { _handle: handle }
    }
}

// ── Audio Recording Worker ────────────────────────────────────────────────────

/// Accumulates incoming audio into a pre-roll buffer.
///
/// When started (via [`Lifecycle::Start`]) it enters recording mode. When
/// stopped (via [`Lifecycle::Stop`]) it drains the buffer and emits
/// [`Event::RecordingResult`].
pub struct AudioRecordingWorker {
    _handle: JoinHandle<()>,
}

impl AudioRecordingWorker {
    pub fn spawn(
        audio_rx: Receiver<ArcAudioBuffer>,
        control_rx: Receiver<Lifecycle>,
        event_tx: Sender<Event>,
    ) -> Self {
        let handle = thread::spawn(move || {
            // 2-second pre-roll buffer so we capture audio that arrived just
            // before the VAD triggered the stop signal.
            let mut buffer = RecordingBuffer::new(AUDIO_TARGET_RATE as usize * 2);

            loop {
                // Drain all pending control signals before processing audio.
                while let Ok(cmd) = control_rx.try_recv() {
                    match cmd {
                        Lifecycle::Start => buffer.set_recording(true),
                        Lifecycle::Stop => {
                            buffer.set_recording(false);
                            let audio = buffer.take_audio();
                            event_tx.send(Event::RecordingResult(audio)).ok();
                        }
                    }
                }

                match audio_rx.recv() {
                    Ok(audio) => buffer.push(&audio),
                    Err(_) => break,
                }
            }
        });

        Self { _handle: handle }
    }
}
