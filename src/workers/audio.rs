use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use boris_audio::{
    buffer::RecordingBuffer, capture::Capture, resampler::Resampler, AUDIO_CHUNK_SIZE,
    AUDIO_TARGET_RATE,
};
use boris_core::{
    error::Result, event::Event, types::ArcAudioBuffer, AudioBuffer, AudioSample, TurnId,
};

// ── Recorder control ──────────────────────────────────────────────────────────

/// Start capture for a specific turn so the resulting clip is tagged correctly.
#[derive(Debug)]
pub enum RecorderCtl {
    Start { turn: TurnId },
    Stop,
}

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
                        Ok(resampled) if resampled.is_empty() => {
                            // Rubato may produce 0 frames on some partial FFT steps.
                        }
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
///
/// Subscribers use **bounded** `SyncSender`s. On full channels we drop the frame
/// for that subscriber (drop-newest) so capture never blocks on a slow sensor.
pub struct AudioDispatcherWorker {
    _handle: JoinHandle<()>,
}

impl AudioDispatcherWorker {
    pub fn spawn(
        audio_rx: Receiver<AudioBuffer>,
        subscribers: Vec<SyncSender<ArcAudioBuffer>>,
    ) -> Self {
        let handle = thread::spawn(move || {
            while let Ok(audio) = audio_rx.recv() {
                let shared: ArcAudioBuffer = Arc::from(audio);
                for tx in &subscribers {
                    match tx.try_send(shared.clone()) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            tracing::warn!("AudioDispatcher: subscriber full — dropping frame");
                        }
                        Err(TrySendError::Disconnected(_)) => {}
                    }
                }
            }
        });
        Self { _handle: handle }
    }
}

// ── Utterance capture ─────────────────────────────────────────────────────────

/// Job that records one user utterance for a [`TurnId`].
///
/// Continuously keeps a short pre-roll while idle. [`RecorderCtl::Start`] freezes
/// that window and appends live audio; [`RecorderCtl::Stop`] emits
/// [`Event::RecordingResult`] tagged with the active turn.
pub struct UtteranceCapture {
    _handle: JoinHandle<()>,
}

impl UtteranceCapture {
    pub fn spawn(
        audio_rx: Receiver<ArcAudioBuffer>,
        control_rx: Receiver<RecorderCtl>,
        event_tx: Sender<Event>,
    ) -> Self {
        let handle = thread::spawn(move || {
            // 2-second pre-roll so speech that started just before Start is kept.
            let mut buffer = RecordingBuffer::new(AUDIO_TARGET_RATE as usize * 2);
            let mut active_turn: Option<TurnId> = None;

            loop {
                while let Ok(cmd) = control_rx.try_recv() {
                    match cmd {
                        RecorderCtl::Start { turn } => {
                            active_turn = Some(turn);
                            buffer.set_recording(true);
                        }
                        RecorderCtl::Stop => {
                            buffer.set_recording(false);
                            let audio = buffer.take_audio();
                            if let Some(turn) = active_turn.take() {
                                event_tx.send(Event::RecordingResult { turn, audio }).ok();
                            } else {
                                tracing::warn!(
                                    "UtteranceCapture: Stop with no active turn — dropping clip"
                                );
                            }
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
