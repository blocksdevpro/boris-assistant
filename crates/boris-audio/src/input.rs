//! Microphone capture pipeline.
//!
//! Real-time callback → bounded queue → worker thread (resample) → subscribers.
//! The RT callback must never block; drops are counted and logged from the worker.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
};

use cpal::{
    traits::{DeviceTrait, StreamTrait},
    Sample,
};

use boris_core::{ArcAudioBuffer, AudioBuffer, AudioSample, Error, Result};

use crate::resampler::InputResampler;

/// Bounded queue capacity for raw capture frames (~2–3s headroom at ~10ms callbacks).
const CAPTURE_QUEUE_CAPACITY: usize = 256;

/// Subscriber fan-out list shared with [`crate::AudioService`].
pub(crate) type InputSubscribers = Arc<Mutex<Vec<crossbeam_channel::Sender<ArcAudioBuffer>>>>;

/// Live input stream + resample worker for one capture device.
pub(crate) struct InputPipeline {
    /// Held so the device callback lives for the pipeline lifetime.
    /// `Option` so [`Drop`] can drop the stream (and its capture sender) before join.
    stream: Option<cpal::Stream>,
    shutdown: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    /// Currently open device id (for switch-no-op checks).
    pub device_id: cpal::DeviceId,
    _subscribers: InputSubscribers,
}

impl InputPipeline {
    /// Open `device`, start capture, and fan resampled mono frames to `subscribers`.
    pub(crate) fn from_device(device: &cpal::Device, subscribers: InputSubscribers) -> Result<Self> {
        let shutdown = Arc::new(AtomicBool::new(false));
        let (audio_tx, audio_rx) =
            crossbeam_channel::bounded::<AudioBuffer>(CAPTURE_QUEUE_CAPACITY);

        let device_id = device
            .id()
            .map_err(|e| Error::audio(format!("input device id: {e}")))?;
        let config = device.default_input_config().map_err(|e| {
            Error::audio(format!(
                "default_input_config failed — mic may be in use or denied: {e}"
            ))
        })?;
        let channels = config.channels();
        let sample_rate = config.sample_rate();
        let sample_format = config.sample_format();
        tracing::info!(
            ?device_id,
            channels,
            ?sample_rate,
            ?sample_format,
            "InputPipeline::from_device"
        );

        let stream_config = cpal::StreamConfig {
            channels,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        let capture_drops = Arc::new(AtomicU64::new(0));
        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                build_input_stream::<f32>(device, stream_config, audio_tx, capture_drops.clone())?
            }
            cpal::SampleFormat::I16 => {
                build_input_stream::<i16>(device, stream_config, audio_tx, capture_drops.clone())?
            }
            cpal::SampleFormat::U16 => {
                build_input_stream::<u16>(device, stream_config, audio_tx, capture_drops.clone())?
            }
            cpal::SampleFormat::I32 => {
                build_input_stream::<i32>(device, stream_config, audio_tx, capture_drops.clone())?
            }
            cpal::SampleFormat::U32 => {
                build_input_stream::<u32>(device, stream_config, audio_tx, capture_drops.clone())?
            }
            other => {
                return Err(Error::audio(format!(
                    "unsupported input sample format: {other:?}"
                )));
            }
        };

        stream
            .play()
            .map_err(|e| Error::audio(format!("input stream.play() failed: {e}")))?;
        tracing::info!("input stream playing");

        // Capture stays interleaved at device channel count; InputResampler
        // downmixes to mono then converts rate → AUDIO_TARGET_RATE.
        let mut resampler = InputResampler::new(channels as u32, sample_rate);
        let shutdown_worker = shutdown.clone();
        let subscribers_worker = subscribers.clone();

        let worker = thread::spawn(move || {
            run_input_worker(
                audio_rx,
                shutdown_worker,
                capture_drops,
                &mut resampler,
                subscribers_worker,
            );
        });

        Ok(Self {
            device_id,
            shutdown,
            stream: Some(stream),
            worker: Some(worker),
            _subscribers: subscribers,
        })
    }
}

fn run_input_worker(
    audio_rx: crossbeam_channel::Receiver<AudioBuffer>,
    shutdown: Arc<AtomicBool>,
    capture_drops: Arc<AtomicU64>,
    resampler: &mut InputResampler,
    subscribers: InputSubscribers,
) {
    let mut last_reported_capture_drops = 0u64;
    let mut subscriber_drops: u64 = 0;

    while let Ok(audio) = audio_rx.recv() {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        let drops = capture_drops.load(Ordering::Relaxed);
        if drops > last_reported_capture_drops {
            tracing::warn!(
                new_drops = drops - last_reported_capture_drops,
                total_drops = drops,
                "InputPipeline: capture queue full — mic frames dropped"
            );
            last_reported_capture_drops = drops;
        }

        match resampler.process(&audio) {
            Ok(resampled) if resampled.is_empty() => {
                // Stream resampler only emits when a full FFT block is ready.
            }
            Ok(resampled) => {
                let arc: ArcAudioBuffer = Arc::from(resampled);
                let mut subs = match subscribers.lock() {
                    Ok(g) => g,
                    Err(poisoned) => {
                        tracing::error!("InputPipeline: subscriber mutex poisoned — recovering");
                        poisoned.into_inner()
                    }
                };
                // Prune disconnected senders; count full-queue drops.
                subs.retain(|sub| match sub.try_send(arc.clone()) {
                    Ok(()) => true,
                    Err(crossbeam_channel::TrySendError::Full(_)) => {
                        subscriber_drops = subscriber_drops.saturating_add(1);
                        if subscriber_drops == 1 || subscriber_drops.is_multiple_of(50) {
                            tracing::warn!(
                                subscriber_drops,
                                "InputPipeline: subscriber queue full — dropping resampled frame"
                            );
                        }
                        true
                    }
                    Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                        tracing::debug!("InputPipeline: pruning disconnected subscriber");
                        false
                    }
                });
            }
            Err(e) => {
                tracing::error!(error = %e, "InputPipeline: resample failed");
            }
        }
    }
}

/// Capture callback: convert device samples to interleaved f32 only.
///
/// # RT allocation note
///
/// Reuses a scratch buffer when `try_send` fails (Full/Disconnected reclaim).
/// On a successful send the buffer is moved into the queue, so the next callback
/// may allocate once to rebuild capacity. A lock-free free-list would remove that
/// residual alloc; not done here to keep the design simple.
fn build_input_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    audio_tx: crossbeam_channel::Sender<AudioBuffer>,
    capture_drops: Arc<AtomicU64>,
) -> Result<cpal::Stream>
where
    T: cpal::Sample + cpal::SizedSample + 'static,
    AudioSample: cpal::FromSample<T>,
{
    let mut scratch: AudioBuffer = Vec::new();

    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                scratch.clear();
                if scratch.capacity() < data.len() {
                    scratch.reserve(data.len());
                }
                scratch.extend(data.iter().map(|sample| AudioSample::from_sample(*sample)));

                // Never block the real-time callback. Count drops; worker logs them.
                match audio_tx.try_send(std::mem::take(&mut scratch)) {
                    Ok(()) => {
                        // Residual: next callback may allocate a new scratch buffer.
                    }
                    Err(crossbeam_channel::TrySendError::Full(returned))
                    | Err(crossbeam_channel::TrySendError::Disconnected(returned)) => {
                        capture_drops.fetch_add(1, Ordering::Relaxed);
                        // Reclaim capacity so Full path does not re-allocate.
                        scratch = returned;
                    }
                }
            },
            |err| tracing::error!("InputStream error: {err}"),
            None,
        )
        .map_err(|e| Error::audio(format!("build_input_stream failed: {e}")))
}

impl Drop for InputPipeline {
    fn drop(&mut self) {
        // 1) Signal worker to exit after current frame.
        self.shutdown.store(true, Ordering::Relaxed);
        // 2) Drop stream first so the capture sender closes and `recv` unblocks.
        drop(self.stream.take());
        // 3) Join worker only after the channel can close.
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}
