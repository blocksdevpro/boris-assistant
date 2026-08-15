//! Speaker playback pipeline.
//!
//! Command thread resamples TTS PCM; the cpal callback pulls samples and emits
//! lifecycle events ([`OutputEvent`]) for the voice engine.

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
};

use boris_core::{AudioBuffer, AudioSample, Error, Result};
use cpal::traits::{DeviceTrait, StreamTrait};

use crate::resampler::OutputResampler;

/// Empty device callbacks required after the software queue empties before
/// [`OutputEvent::Drained`]. Covers OS/driver ring-buffer lag.
const DRAIN_EMPTY_CALLBACKS: u32 = 12;

/// Poll interval for the command worker's shutdown check.
///
/// The worker previously blocked on `cmd_rx.recv()` and relied on its matching
/// `Sender` (held elsewhere, e.g. [`crate::AudioService::output_command_channel`])
/// being dropped to unblock it — which in turn relied on undocumented struct
/// field drop order. Using a short `recv_timeout` instead lets the worker wake
/// on its own and check `shutdown` regardless of who still holds a `Sender`.
const OUTPUT_WORKER_SHUTDOWN_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// Commands from [`crate::AudioService`] to the output worker.
#[derive(Debug)]
pub enum OutputCommand {
    /// Replace the current job with one buffer and auto-finish (legacy one-shot).
    Play(AudioBuffer),
    /// Append PCM to the current job (or start one). Does not finish the job.
    Append(AudioBuffer),
    /// Mark the current streaming job complete so Drained can fire after empty,
    /// then acknowledge that the state transition was applied.
    FinishJob(crossbeam_channel::Sender<()>),
    /// Clear pending audio immediately.
    Flush,
}

/// Lifecycle signals for the voice engine / UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputEvent {
    /// The device callback accepted the first real sample for this play job.
    /// Audible output follows within the device/driver buffer latency.
    Started,
    /// Software + short device-buffer drain after a real Play job finished.
    Drained,
    /// Stopped by Flush; not a successful natural finish.
    Cleared,
}

struct OutputStreamState {
    pending: VecDeque<AudioSample>,
    empty_callbacks: u32,
    /// Job in flight: samples queued (or already streaming) for the current Play.
    active: bool,
    /// At least one real sample written to the device for this job.
    started: bool,
    /// When true, more [`OutputCommand::Append`]s may arrive — do not drain yet.
    job_open: bool,
}

impl OutputStreamState {
    fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            empty_callbacks: 0,
            active: false,
            started: false,
            job_open: false,
        }
    }

    fn clear_job(&mut self) {
        self.pending.clear();
        self.empty_callbacks = 0;
        self.active = false;
        self.started = false;
        self.job_open = false;
    }

    fn queue_play(&mut self, samples: Vec<AudioSample>) -> bool {
        self.pending.clear();
        self.pending.extend(samples);
        self.empty_callbacks = 0;
        self.started = false;
        self.job_open = false; // one-shot: drain after this buffer
        self.active = !self.pending.is_empty();
        self.active
    }

    fn queue_append(&mut self, samples: Vec<AudioSample>) -> bool {
        if samples.is_empty() {
            return self.active;
        }
        if !self.active {
            self.started = false;
            self.empty_callbacks = 0;
        }
        self.pending.extend(samples);
        self.job_open = true;
        self.active = true;
        true
    }

    fn finish_job(&mut self) {
        self.job_open = false;
    }
}

/// Live output stream + command worker for one playback device.
pub(crate) struct OutputPipeline {
    /// Held so the device callback lives for the pipeline lifetime.
    stream: Option<cpal::Stream>,
    shutdown: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    /// Currently open device id.
    pub device_id: cpal::DeviceId,
    _state: Arc<Mutex<OutputStreamState>>,
}

impl OutputPipeline {
    /// Open `device` and process commands from `cmd_rx`.
    ///
    /// `source_rate` is the rate of PCM on [`OutputCommand::Play`] (TTS native rate).
    pub(crate) fn from_device(
        device: &cpal::Device,
        cmd_rx: crossbeam_channel::Receiver<OutputCommand>,
        event_tx: crossbeam_channel::Sender<OutputEvent>,
        source_rate: u32,
    ) -> Result<Self> {
        let shutdown = Arc::new(AtomicBool::new(false));
        let device_id = device
            .id()
            .map_err(|e| Error::audio(format!("output device id: {e}")))?;
        let state = Arc::new(Mutex::new(OutputStreamState::new()));

        let config = device.default_output_config().map_err(|e| {
            Error::audio(format!(
                "default_output_config failed — speaker may be denied: {e}"
            ))
        })?;
        let stream_config = config.config();
        let sample_format = config.sample_format();
        tracing::info!(
            ?device_id,
            channels = stream_config.channels,
            sample_rate = ?stream_config.sample_rate,
            ?sample_format,
            source_rate,
            "OutputPipeline::from_device"
        );

        let event_drops = Arc::new(AtomicU64::new(0));
        let stream = match sample_format {
            cpal::SampleFormat::F32 => build_output_stream::<f32>(
                device,
                stream_config,
                state.clone(),
                event_tx.clone(),
                event_drops.clone(),
            )?,
            cpal::SampleFormat::I16 => build_output_stream::<i16>(
                device,
                stream_config,
                state.clone(),
                event_tx.clone(),
                event_drops.clone(),
            )?,
            cpal::SampleFormat::U16 => build_output_stream::<u16>(
                device,
                stream_config,
                state.clone(),
                event_tx.clone(),
                event_drops.clone(),
            )?,
            cpal::SampleFormat::I32 => build_output_stream::<i32>(
                device,
                stream_config,
                state.clone(),
                event_tx.clone(),
                event_drops.clone(),
            )?,
            cpal::SampleFormat::U32 => build_output_stream::<u32>(
                device,
                stream_config,
                state.clone(),
                event_tx.clone(),
                event_drops.clone(),
            )?,
            other => {
                return Err(Error::audio(format!(
                    "unsupported output sample format: {other:?}"
                )));
            }
        };

        stream
            .play()
            .map_err(|e| Error::audio(format!("output stream.play() failed: {e}")))?;
        tracing::info!("output stream playing");

        let shutdown_worker = shutdown.clone();
        let state_worker = state.clone();
        let mut resampler = OutputResampler::new(
            source_rate,
            stream_config.sample_rate,
            stream_config.channels,
        );

        let worker = thread::spawn(move || {
            run_output_worker(
                cmd_rx,
                event_tx,
                shutdown_worker,
                state_worker,
                &mut resampler,
                event_drops,
            );
        });

        Ok(Self {
            device_id,
            shutdown,
            stream: Some(stream),
            worker: Some(worker),
            _state: state,
        })
    }
}

fn lock_output_state(
    state: &Mutex<OutputStreamState>,
) -> std::sync::MutexGuard<'_, OutputStreamState> {
    match state.lock() {
        Ok(g) => g,
        Err(poisoned) => {
            tracing::error!("OutputPipeline: state mutex poisoned — recovering");
            poisoned.into_inner()
        }
    }
}

fn run_output_worker(
    cmd_rx: crossbeam_channel::Receiver<OutputCommand>,
    event_tx: crossbeam_channel::Sender<OutputEvent>,
    shutdown: Arc<AtomicBool>,
    state: Arc<Mutex<OutputStreamState>>,
    resampler: &mut OutputResampler,
    event_drops: Arc<AtomicU64>,
) {
    let mut last_event_drops = 0u64;

    loop {
        // Wake periodically to check `shutdown` instead of blocking indefinitely
        // on `recv()` — see [`OUTPUT_WORKER_SHUTDOWN_POLL`] for why this must not
        // depend on the matching `Sender` being dropped.
        let command = match cmd_rx.recv_timeout(OUTPUT_WORKER_SHUTDOWN_POLL) {
            Ok(command) => command,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        let drops = event_drops.load(Ordering::Relaxed);
        if drops > last_event_drops {
            tracing::warn!(
                new_drops = drops - last_event_drops,
                total_drops = drops,
                "OutputPipeline: output event queue full — events dropped in RT callback"
            );
            last_event_drops = drops;
        }

        match command {
            OutputCommand::Play(audio) => {
                // Resample outside the state lock so the stream callback is not
                // blocked for the full FFT oneshot duration.
                let in_samples = audio.len();
                let resampled = match resampler.process(&audio) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!(error = %e, "OutputPipeline: resample failed");
                        continue;
                    }
                };

                tracing::info!(
                    in_samples,
                    out_samples = resampled.len(),
                    "OutputPipeline: queued play buffer"
                );

                let mut guard = lock_output_state(&state);
                if !guard.queue_play(resampled) {
                    tracing::warn!("OutputPipeline: Play produced empty buffer");
                }
            }
            OutputCommand::Append(audio) => {
                let resampled = match resampler.process(&audio) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!(error = %e, "OutputPipeline: append resample failed");
                        continue;
                    }
                };
                lock_output_state(&state).queue_append(resampled);
            }
            OutputCommand::FinishJob(ack) => {
                lock_output_state(&state).finish_job();
                // Control path, never the RT callback. A bounded acknowledgement
                // lets the host distinguish "queued" from "job actually closed".
                let _ = ack.send(());
            }
            OutputCommand::Flush => {
                lock_output_state(&state).clear_job();
                try_emit_worker_event(&event_tx, &event_drops, OutputEvent::Cleared);
            }
        }
    }
}

/// Emit from the non-RT command worker without ever blocking command progress.
/// Call only after releasing [`OutputStreamState`]'s mutex.
fn try_emit_worker_event(
    event_tx: &crossbeam_channel::Sender<OutputEvent>,
    event_drops: &AtomicU64,
    event: OutputEvent,
) {
    match event_tx.try_send(event) {
        Ok(()) => {}
        Err(crossbeam_channel::TrySendError::Full(_)) => {
            event_drops.fetch_add(1, Ordering::Relaxed);
        }
        Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
            tracing::error!(?event, "OutputPipeline: event channel disconnected");
        }
    }
}

fn fill_silence<T>(output: &mut [T])
where
    T: cpal::Sample + cpal::FromSample<f32>,
{
    for sample in output.iter_mut() {
        *sample = T::from_sample(0.0);
    }
}

fn build_output_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    state: Arc<Mutex<OutputStreamState>>,
    event_tx: crossbeam_channel::Sender<OutputEvent>,
    event_drops: Arc<AtomicU64>,
) -> Result<cpal::Stream>
where
    T: cpal::Sample + cpal::SizedSample + cpal::FromSample<f32> + 'static,
{
    device
        .build_output_stream(
            config,
            move |output: &mut [T], _| {
                // Prefer try_lock on the RT path: never block; fill silence if busy.
                let mut state = match state.try_lock() {
                    Ok(g) => g,
                    Err(std::sync::TryLockError::WouldBlock) => {
                        fill_silence(output);
                        return;
                    }
                    Err(std::sync::TryLockError::Poisoned(p)) => {
                        // Recover rather than panicking on the audio thread.
                        p.into_inner()
                    }
                };

                let mut got_real_sample = false;

                for sample in output.iter_mut() {
                    if let Some(s) = state.pending.pop_front() {
                        *sample = T::from_sample(s);
                        got_real_sample = true;
                    } else {
                        *sample = T::from_sample(0.0);
                    }
                }

                // Idle: never emit Drained.
                if !state.active {
                    return;
                }

                if got_real_sample {
                    let first_real_sample = !state.started;
                    state.started = true;
                    state.empty_callbacks = 0;
                    if first_real_sample && event_tx.try_send(OutputEvent::Started).is_err() {
                        event_drops.fetch_add(1, Ordering::Relaxed);
                    }
                    return;
                }

                if !state.pending.is_empty() {
                    state.empty_callbacks = 0;
                    return;
                }

                // Software queue empty. Only count toward drain after we have
                // actually written samples for this job (not pre-play / idle).
                // Streaming jobs stay open until FinishJob.
                if !state.started || state.job_open {
                    return;
                }

                state.empty_callbacks = state.empty_callbacks.saturating_add(1);
                if state.empty_callbacks >= DRAIN_EMPTY_CALLBACKS {
                    state.active = false;
                    state.started = false;
                    state.empty_callbacks = 0;
                    // Never block in the cpal callback.
                    if event_tx.try_send(OutputEvent::Drained).is_err() {
                        event_drops.fetch_add(1, Ordering::Relaxed);
                    }
                }
            },
            |err| tracing::error!("OutputStream error: {err}"),
            None,
        )
        .map_err(|e| Error::audio(format!("build_output_stream failed: {e}")))
}

impl Drop for OutputPipeline {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // Drop stream so the device callback stops running.
        //
        // The worker's join below no longer depends on this happening before —
        // or on the matching `OutputCommand` `Sender` (owned outside this struct,
        // e.g. by `AudioService`) being dropped at all. `run_output_worker` wakes
        // on its own via `recv_timeout` (see `OUTPUT_WORKER_SHUTDOWN_POLL`) and
        // exits once it observes `shutdown`, so this join can no longer hang
        // waiting on someone else's channel handle.
        drop(self.stream.take());
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_job_does_not_finish_until_explicit_close() {
        let mut state = OutputStreamState::new();
        assert!(state.queue_append(vec![0.1, 0.2]));
        assert!(state.active);
        assert!(state.job_open);

        state.finish_job();
        assert!(state.active, "closing must not discard queued samples");
        assert!(!state.job_open);
        assert_eq!(state.pending.len(), 2);
    }

    #[test]
    fn flush_clears_an_open_streaming_job() {
        let mut state = OutputStreamState::new();
        state.queue_append(vec![0.1]);
        state.clear_job();
        assert!(!state.active);
        assert!(!state.job_open);
        assert!(state.pending.is_empty());
    }

    #[test]
    fn finish_command_acknowledges_after_job_is_closed() {
        let (cmd_tx, cmd_rx) = crossbeam_channel::bounded(4);
        let (event_tx, _event_rx) = crossbeam_channel::bounded(4);
        let shutdown = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(OutputStreamState::new()));
        let worker_shutdown = shutdown.clone();
        let worker_state = state.clone();
        let worker = thread::spawn(move || {
            let mut resampler = OutputResampler::new(16_000, 16_000, 1);
            run_output_worker(
                cmd_rx,
                event_tx,
                worker_shutdown,
                worker_state,
                &mut resampler,
                Arc::new(AtomicU64::new(0)),
            );
        });

        cmd_tx.send(OutputCommand::Append(vec![0.2; 32])).unwrap();
        let (ack_tx, ack_rx) = crossbeam_channel::bounded(1);
        cmd_tx.send(OutputCommand::FinishJob(ack_tx)).unwrap();
        ack_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("FinishJob acknowledgement");

        let guard = lock_output_state(&state);
        assert!(!guard.job_open, "ack must mean the job is already closed");
        assert!(guard.active);
        drop(guard);

        shutdown.store(true, Ordering::Relaxed);
        drop(cmd_tx);
        worker.join().unwrap();
    }

    #[test]
    fn saturated_event_queue_does_not_block_finish_control() {
        let (cmd_tx, cmd_rx) = crossbeam_channel::bounded(4);
        let (event_tx, event_rx) = crossbeam_channel::bounded(1);
        // Simulate a host that momentarily stopped draining lifecycle events.
        event_tx.send(OutputEvent::Cleared).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(OutputStreamState::new()));
        let drops = Arc::new(AtomicU64::new(0));
        let worker_shutdown = shutdown.clone();
        let worker_state = state.clone();
        let worker_drops = drops.clone();
        let worker = thread::spawn(move || {
            let mut resampler = OutputResampler::new(16_000, 16_000, 1);
            run_output_worker(
                cmd_rx,
                event_tx,
                worker_shutdown,
                worker_state,
                &mut resampler,
                worker_drops,
            );
        });

        cmd_tx.send(OutputCommand::Append(vec![0.2; 32])).unwrap();
        cmd_tx.send(OutputCommand::Flush).unwrap();
        let (ack_tx, ack_rx) = crossbeam_channel::bounded(1);
        cmd_tx.send(OutputCommand::FinishJob(ack_tx)).unwrap();
        let applied_without_event_drain = ack_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .is_ok();

        // Always make cleanup safe even if this behavior regresses.
        let _ = event_rx.try_recv();
        shutdown.store(true, Ordering::Relaxed);
        drop(cmd_tx);
        worker.join().unwrap();

        assert!(
            applied_without_event_drain,
            "a full event queue must not strand FinishJob"
        );
        assert!(drops.load(Ordering::Relaxed) >= 1);
    }
}
