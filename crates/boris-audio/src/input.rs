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

use boris_core::{ArcAudioBuffer, AudioBuffer, AudioSample};

use crate::resampler::InputResampler;

/// Bounded queue capacity for raw capture frames (~2–3s headroom at ~10ms callbacks).
const CAPTURE_QUEUE_CAPACITY: usize = 256;

/// Subscriber fan-out list shared with [`crate::AudioService`].
pub type InputSubscribers = Arc<Mutex<Vec<crossbeam_channel::Sender<ArcAudioBuffer>>>>;

/// Live input stream + resample worker for one capture device.
pub struct InputPipeline {
    /// Held so the device callback lives for the pipeline lifetime.
    _stream: cpal::Stream,
    shutdown: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    /// Currently open device id (for switch-no-op checks).
    pub device_id: cpal::DeviceId,
    _subscribers: InputSubscribers,
}

impl InputPipeline {
    /// Open `device`, start capture, and fan resampled mono frames to `subscribers`.
    pub fn from_device(device: &cpal::Device, subscribers: InputSubscribers) -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        let (audio_tx, audio_rx) =
            crossbeam_channel::bounded::<AudioBuffer>(CAPTURE_QUEUE_CAPACITY);

        let device_id = device.id().expect("input device id");
        let config = device
            .default_input_config()
            .expect("default_input_config — mic may be in use or denied");
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
                build_input_stream::<f32>(device, stream_config, audio_tx, capture_drops.clone())
            }
            cpal::SampleFormat::I16 => {
                build_input_stream::<i16>(device, stream_config, audio_tx, capture_drops.clone())
            }
            cpal::SampleFormat::U16 => {
                build_input_stream::<u16>(device, stream_config, audio_tx, capture_drops.clone())
            }
            other => {
                tracing::error!(?other, "unsupported input sample format");
                panic!("Unsupported sample format for InputStream: {other:?}");
            }
        };

        if let Err(e) = stream.play() {
            tracing::error!(error = %e, "input stream.play() failed");
            panic!("input stream.play() failed: {e}");
        }
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

        Self {
            device_id,
            shutdown,
            _stream: stream,
            worker: Some(worker),
            _subscribers: subscribers,
        }
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
                let subs = subscribers.lock().unwrap();
                for sub in subs.iter() {
                    if sub.try_send(arc.clone()).is_err() {
                        subscriber_drops = subscriber_drops.saturating_add(1);
                        if subscriber_drops == 1 || subscriber_drops.is_multiple_of(50) {
                            tracing::warn!(
                                subscriber_drops,
                                "InputPipeline: subscriber queue full — dropping resampled frame"
                            );
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "InputPipeline: resample failed");
            }
        }
    }
}

/// Capture callback: convert device samples to interleaved f32 only.
fn build_input_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    audio_tx: crossbeam_channel::Sender<AudioBuffer>,
    capture_drops: Arc<AtomicU64>,
) -> cpal::Stream
where
    T: cpal::Sample + cpal::SizedSample + 'static,
    AudioSample: cpal::FromSample<T>,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                let samples: Vec<AudioSample> = data
                    .iter()
                    .map(|sample| AudioSample::from_sample(*sample))
                    .collect();
                // Never block the real-time callback. Count drops; worker logs them.
                if audio_tx.try_send(samples).is_err() {
                    capture_drops.fetch_add(1, Ordering::Relaxed);
                }
            },
            |err| tracing::error!("InputStream error: {err}"),
            None,
        )
        .expect("build_input_stream")
}

impl Drop for InputPipeline {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // `stream` is dropped with Self; join the worker after signaling shutdown.
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}
