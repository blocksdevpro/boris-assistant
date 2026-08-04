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

/// Empty device callbacks required after software queue empties before [`OutputEvent::Drained`].
/// Covers OS/driver ring-buffer lag after our pending deque is empty.
const DRAIN_EMPTY_CALLBACKS: u32 = 12;

pub enum OutputCommand {
    Play(AudioBuffer),
    Flush,
}

pub enum OutputEvent {
    /// Samples are in the device queue — audible audio is about to start (or just started).
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
    /// Prevents Drained while still resampling / before audio starts.
    started: bool,
}

pub struct OutputPipeline {
    _stream: cpal::Stream,
    _flag: Arc<AtomicBool>,
    _handle: Option<thread::JoinHandle<()>>,
    pub device_id: cpal::DeviceId,
    _state: Arc<Mutex<OutputStreamState>>,
}

impl OutputPipeline {
    pub fn from_device(
        device: &cpal::Device,
        cmd_rx: crossbeam_channel::Receiver<OutputCommand>,
        event_tx: crossbeam_channel::Sender<OutputEvent>,
        source_rate: u32,
    ) -> Self {
        let flag = Arc::new(AtomicBool::new(false));
        let device_id = device.id().expect("output device id");
        let state = Arc::new(Mutex::new(OutputStreamState {
            pending: VecDeque::new(),
            empty_callbacks: 0,
            active: false,
            started: false,
        }));

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

        let state_clone = state.clone();
        let event_tx_clone = event_tx.clone();
        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                Self::build_stream::<f32>(device, stream_config, state_clone, event_tx_clone)
            }
            cpal::SampleFormat::I32 => {
                Self::build_stream::<i32>(device, stream_config, state_clone, event_tx_clone)
            }
            cpal::SampleFormat::U32 => {
                Self::build_stream::<u32>(device, stream_config, state_clone, event_tx_clone)
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

        let flag_clone = flag.clone();
        let state_clone = state.clone();
        let mut resampler = OutputResampler::new(
            source_rate,
            stream_config.sample_rate,
            stream_config.channels,
        );
        let handle = thread::spawn(move || {
            while let Ok(command) = cmd_rx.recv() {
                if flag_clone.load(Ordering::Relaxed) {
                    break;
                }
                match command {
                    OutputCommand::Play(audio) => {
                        // Resample *outside* the state lock so the stream can keep
                        // outputting silence without holding the mutex for long.
                        // `active` stays false until samples are queued → no early Drained.
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

                        let mut state = state_clone.lock().unwrap();
                        state.pending.clear();
                        state.pending.extend(resampled);
                        state.empty_callbacks = 0;
                        state.started = false;
                        // Only arm drain detection once audio is actually ready.
                        state.active = !state.pending.is_empty();
                        if !state.active {
                            tracing::warn!("OutputPipeline: Play produced empty buffer");
                        } else {
                            // Host can flip UI to "Speaking" only after this — not during TTS synth.
                            let _ = event_tx.send(OutputEvent::Started);
                        }
                    }
                    OutputCommand::Flush => {
                        {
                            let mut state = state_clone.lock().unwrap();
                            state.pending.clear();
                            state.empty_callbacks = 0;
                            state.active = false;
                            state.started = false;
                        }
                        let _ = event_tx.send(OutputEvent::Cleared);
                    }
                }
            }
        });

        Self {
            device_id,
            _flag: flag,
            _state: state,
            _stream: stream,
            _handle: Some(handle),
        }
    }

    fn build_stream<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        state: Arc<Mutex<OutputStreamState>>,
        event_tx: crossbeam_channel::Sender<OutputEvent>,
    ) -> cpal::Stream
    where
        T: cpal::Sample + cpal::SizedSample + cpal::FromSample<f32> + 'static,
    {
        let stream = device
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
            .unwrap();
        stream
    }
}

impl Drop for OutputPipeline {
    fn drop(&mut self) {
        self._flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self._handle.take() {
            let _ = handle.join();
        }
    }
}
