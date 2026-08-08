//! Speaker playback pipeline.
//!
//! Command thread resamples TTS PCM; the cpal callback pulls samples and emits
//! lifecycle events ([`OutputEvent`]) for the voice engine.

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

use boris_core::{AudioBuffer, AudioSample};
use cpal::traits::{DeviceTrait, StreamTrait};

use crate::resampler::OutputResampler;

/// Empty device callbacks required after the software queue empties before
/// [`OutputEvent::Drained`]. Covers OS/driver ring-buffer lag.
const DRAIN_EMPTY_CALLBACKS: u32 = 12;

/// Commands from [`crate::AudioService`] to the output worker.
pub enum OutputCommand {
    /// Queue mono PCM at the service source rate for playback.
    Play(AudioBuffer),
    /// Clear pending audio immediately.
    Flush,
}

/// Lifecycle signals for the voice engine / UI.
pub enum OutputEvent {
    /// Samples are in the device queue — audible audio is about to start.
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
}

impl OutputStreamState {
    fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            empty_callbacks: 0,
            active: false,
            started: false,
        }
    }

    fn clear_job(&mut self) {
        self.pending.clear();
        self.empty_callbacks = 0;
        self.active = false;
        self.started = false;
    }

    fn queue_play(&mut self, samples: Vec<AudioSample>) -> bool {
        self.pending.clear();
        self.pending.extend(samples);
        self.empty_callbacks = 0;
        self.started = false;
        self.active = !self.pending.is_empty();
        self.active
    }
}

/// Live output stream + command worker for one playback device.
pub struct OutputPipeline {
    /// Held so the device callback lives for the pipeline lifetime.
    _stream: cpal::Stream,
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
    pub fn from_device(
        device: &cpal::Device,
        cmd_rx: crossbeam_channel::Receiver<OutputCommand>,
        event_tx: crossbeam_channel::Sender<OutputEvent>,
        source_rate: u32,
    ) -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        let device_id = device.id().expect("output device id");
        let state = Arc::new(Mutex::new(OutputStreamState::new()));

        let config = device
            .default_output_config()
            .expect("default_output_config — speaker may be denied");
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

        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                build_output_stream::<f32>(device, stream_config.clone(), state.clone(), event_tx.clone())
            }
            cpal::SampleFormat::I32 => {
                build_output_stream::<i32>(device, stream_config.clone(), state.clone(), event_tx.clone())
            }
            cpal::SampleFormat::U32 => {
                build_output_stream::<u32>(device, stream_config.clone(), state.clone(), event_tx.clone())
            }
            other => {
                tracing::error!(?other, "unsupported output sample format");
                panic!("unsupported sample format: {other:?}");
            }
        };

        if let Err(e) = stream.play() {
            tracing::error!(error = %e, "output stream.play() failed");
            panic!("output stream.play() failed: {e}");
        }
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
            );
        });

        Self {
            device_id,
            shutdown,
            _stream: stream,
            worker: Some(worker),
            _state: state,
        }
    }
}

fn run_output_worker(
    cmd_rx: crossbeam_channel::Receiver<OutputCommand>,
    event_tx: crossbeam_channel::Sender<OutputEvent>,
    shutdown: Arc<AtomicBool>,
    state: Arc<Mutex<OutputStreamState>>,
    resampler: &mut OutputResampler,
) {
    while let Ok(command) = cmd_rx.recv() {
        if shutdown.load(Ordering::Relaxed) {
            break;
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

                let mut guard = state.lock().unwrap();
                if guard.queue_play(resampled) {
                    // Host can flip UI to "Speaking" only after this — not during TTS synth.
                    let _ = event_tx.send(OutputEvent::Started);
                } else {
                    tracing::warn!("OutputPipeline: Play produced empty buffer");
                }
            }
            OutputCommand::Flush => {
                state.lock().unwrap().clear_job();
                let _ = event_tx.send(OutputEvent::Cleared);
            }
        }
    }
}

fn build_output_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    state: Arc<Mutex<OutputStreamState>>,
    event_tx: crossbeam_channel::Sender<OutputEvent>,
) -> cpal::Stream
where
    T: cpal::Sample + cpal::SizedSample + cpal::FromSample<f32> + 'static,
{
    device
        .build_output_stream(
            config,
            move |output: &mut [T], _| {
                let mut state = state.lock().unwrap();
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
                    state.started = true;
                    state.empty_callbacks = 0;
                    return;
                }

                if !state.pending.is_empty() {
                    state.empty_callbacks = 0;
                    return;
                }

                // Software queue empty. Only count toward drain after we have
                // actually written samples for this job (not pre-play / idle).
                if !state.started {
                    return;
                }

                state.empty_callbacks = state.empty_callbacks.saturating_add(1);
                if state.empty_callbacks >= DRAIN_EMPTY_CALLBACKS {
                    state.active = false;
                    state.started = false;
                    state.empty_callbacks = 0;
                    let _ = event_tx.send(OutputEvent::Drained);
                }
            },
            |err| tracing::error!("OutputStream error: {err}"),
            None,
        )
        .expect("build_output_stream")
}

impl Drop for OutputPipeline {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}
