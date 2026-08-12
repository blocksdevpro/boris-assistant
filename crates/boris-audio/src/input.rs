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

/// Capacity of the scratch-buffer free-list returned from the worker to the RT callback.
const SCRATCH_FREE_LIST_CAPACITY: usize = 8;

/// Number of scratch buffers pre-allocated into the free-list before the stream starts.
const SCRATCH_FREE_LIST_PRIME_COUNT: usize = 4;

/// Generous initial capacity for each pre-allocated scratch buffer (frames per callback).
/// Any actual callback size is handled after the first callback via clear+reuse.
const SCRATCH_BUFFER_INITIAL_CAPACITY: usize = 4096;

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
    pub(crate) fn from_device(
        device: &cpal::Device,
        subscribers: InputSubscribers,
    ) -> Result<Self> {
        let shutdown = Arc::new(AtomicBool::new(false));
        let (audio_tx, audio_rx) =
            crossbeam_channel::bounded::<AudioBuffer>(CAPTURE_QUEUE_CAPACITY);

        // Free-list of pre-allocated scratch buffers handed back by the worker so the
        // RT callback can pull a ready `Vec` instead of allocating. See the RT
        // allocation note on `build_input_stream` for the full design.
        let (scratch_return_tx, scratch_return_rx) =
            crossbeam_channel::bounded::<AudioBuffer>(SCRATCH_FREE_LIST_CAPACITY);
        for _ in 0..SCRATCH_FREE_LIST_PRIME_COUNT {
            let _ = scratch_return_tx.try_send(Vec::with_capacity(SCRATCH_BUFFER_INITIAL_CAPACITY));
        }

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
            cpal::SampleFormat::F32 => build_input_stream::<f32>(
                device,
                stream_config,
                audio_tx,
                capture_drops.clone(),
                scratch_return_rx,
            )?,
            cpal::SampleFormat::I16 => build_input_stream::<i16>(
                device,
                stream_config,
                audio_tx,
                capture_drops.clone(),
                scratch_return_rx,
            )?,
            cpal::SampleFormat::U16 => build_input_stream::<u16>(
                device,
                stream_config,
                audio_tx,
                capture_drops.clone(),
                scratch_return_rx,
            )?,
            cpal::SampleFormat::I32 => build_input_stream::<i32>(
                device,
                stream_config,
                audio_tx,
                capture_drops.clone(),
                scratch_return_rx,
            )?,
            cpal::SampleFormat::U32 => build_input_stream::<u32>(
                device,
                stream_config,
                audio_tx,
                capture_drops.clone(),
                scratch_return_rx,
            )?,
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
                scratch_return_tx,
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
    scratch_return_tx: crossbeam_channel::Sender<AudioBuffer>,
) {
    let mut last_reported_capture_drops = 0u64;
    let mut subscriber_drops: u64 = 0;

    while let Ok(mut audio) = audio_rx.recv() {
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

        // Hand the emptied buffer back to the RT callback's free-list so it
        // doesn't have to allocate a new `Vec` on the next successful send.
        // Non-blocking: if the free-list is full, just drop the buffer.
        audio.clear();
        let _ = scratch_return_tx.try_send(audio);
    }
}

/// Capture callback: convert device samples to interleaved f32 only.
///
/// # RT allocation note
///
/// The callback owns one scratch buffer at a time. On a successful `try_send`
/// the buffer is moved into the capture queue, so the callback needs a fresh
/// one for the *next* invocation. Instead of allocating there, it pulls a
/// pre-allocated buffer from `scratch_return_rx` — a bounded free-list that the
/// worker thread (`run_input_worker`) refills by clearing and returning each
/// `AudioBuffer` once it is done with it (see the free-list priming in
/// [`InputPipeline::from_device`]). `try_recv` is non-blocking, so the RT
/// thread never waits on the worker.
///
/// In steady state the free-list always has a spare buffer by the time it is
/// needed, so there is no allocation on the RT thread. The only allocation
/// paths are cold-start / rare-path fallbacks: the free-list starting empty,
/// or the worker falling behind so the list runs dry — both fall back to
/// `Vec::new()`, which is acceptable off the steady-state path.
fn build_input_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    audio_tx: crossbeam_channel::Sender<AudioBuffer>,
    capture_drops: Arc<AtomicU64>,
    scratch_return_rx: crossbeam_channel::Receiver<AudioBuffer>,
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
                        // Steady state: pull a ready buffer from the free-list so the
                        // next callback does not need to allocate. Rare-path fallback
                        // to a fresh (empty) `Vec` if the free-list is momentarily dry.
                        scratch = scratch_return_rx.try_recv().unwrap_or_default();
                    }
                    Err(crossbeam_channel::TrySendError::Full(returned))
                    | Err(crossbeam_channel::TrySendError::Disconnected(returned)) => {
                        capture_drops.fetch_add(1, Ordering::Relaxed);
                        // Reclaim capacity so the Full/Disconnected path does not re-allocate.
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

#[cfg(test)]
mod scratch_free_list_tests {
    //! Exercises the free-list hand-back protocol used by [`build_input_stream`]
    //! and [`run_input_worker`] without needing a real cpal device: a bounded
    //! channel primed with pre-allocated buffers, round-tripped the same way
    //! the RT callback (`try_recv`) and the worker (`try_send`) use it.
    use super::*;

    #[test]
    fn primed_buffer_is_reused_without_reallocation() {
        let (tx, rx) = crossbeam_channel::bounded::<AudioBuffer>(SCRATCH_FREE_LIST_CAPACITY);
        for _ in 0..SCRATCH_FREE_LIST_PRIME_COUNT {
            tx.try_send(Vec::with_capacity(SCRATCH_BUFFER_INITIAL_CAPACITY))
                .expect("priming a fresh bounded channel must not fail");
        }

        // "RT callback": pull a primed buffer instead of allocating.
        let mut scratch = rx.try_recv().expect("free-list primed with spare buffers");
        let primed_capacity = scratch.capacity();
        assert!(primed_capacity >= SCRATCH_BUFFER_INITIAL_CAPACITY);

        // Fill it as the callback would, well within the primed capacity.
        scratch.extend((0..128).map(|i| i as AudioSample));
        assert_eq!(scratch.len(), 128);

        // "Worker": done with the buffer, clears and hands it back.
        scratch.clear();
        tx.try_send(scratch)
            .expect("free-list has room for the returned buffer");

        // "RT callback" on the next invocation: pulls the same buffer back —
        // capacity must be preserved (no reallocation happened in the round trip).
        let scratch_again = rx.try_recv().expect("worker returned a buffer");
        assert_eq!(scratch_again.len(), 0);
        assert!(scratch_again.capacity() >= primed_capacity);
    }

    #[test]
    fn empty_free_list_falls_back_to_default_without_panicking() {
        let (_tx, rx) = crossbeam_channel::bounded::<AudioBuffer>(SCRATCH_FREE_LIST_CAPACITY);
        // Mirrors the RT callback's fallback: `try_recv().unwrap_or_default()`.
        let scratch: AudioBuffer = rx.try_recv().unwrap_or_default();
        assert!(scratch.is_empty());
    }

    #[test]
    fn worker_drops_buffer_when_free_list_is_full() {
        let (tx, rx) = crossbeam_channel::bounded::<AudioBuffer>(1);
        tx.try_send(Vec::new()).unwrap();
        // Free-list already full: worker's hand-back must not block or panic,
        // just silently drop the extra buffer (mirrors `let _ = tx.try_send(audio)`).
        let result = tx.try_send(Vec::new());
        assert!(matches!(
            result,
            Err(crossbeam_channel::TrySendError::Full(_))
        ));
        assert_eq!(rx.len(), 1);
    }
}
