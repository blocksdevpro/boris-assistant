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

const DRAIN_EMPTY_CALLBACKS: u32 = 5;

pub enum OutputCommand {
    Play(AudioBuffer),
    Flush,
}

pub enum OutputEvent {
    Drained,
    Cleared,
}

pub struct OutputStreamState {
    pending: VecDeque<AudioSample>,
    empty_callbacks: u32,
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
        // instance params
        let flag = Arc::new(AtomicBool::new(false));
        let device_id = device.id().unwrap();
        let state = Arc::new(Mutex::new(OutputStreamState {
            pending: VecDeque::new(),
            empty_callbacks: 0,
        }));

        // device params
        let config = device.default_output_config().unwrap();
        let channels = config.channels();
        let sample_rate = config.sample_rate();
        let stream_config = config.config();

        // build the stream
        let state_clone = state.clone();
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                Self::build_stream::<f32>(device, stream_config, state_clone, event_tx)
            }
            cpal::SampleFormat::I32 => {
                Self::build_stream::<i32>(device, stream_config, state_clone, event_tx)
            }
            cpal::SampleFormat::U32 => {
                Self::build_stream::<u32>(device, stream_config, state_clone, event_tx)
            }
            _ => panic!("unsupported sample format"),
        };

        stream.play().unwrap();

        // spawn thread
        let flag_clone = flag.clone();
        let state_clone = state.clone();
        let mut resampler = OutputResampler::new(source_rate, sample_rate, channels);
        let handle = thread::spawn(move || {
            while let Ok(command) = cmd_rx.recv() {
                if flag_clone.load(Ordering::Relaxed) {
                    println!("OutputPipeline droped");
                    break;
                }
                match command {
                    OutputCommand::Play(audio) => {
                        let resampled = resampler.process(&audio).unwrap();
                        {
                            let mut state = state_clone.lock().unwrap();
                            state.pending.clear();
                            state.pending.extend(resampled);
                            state.empty_callbacks = 0
                        }
                    }
                    OutputCommand::Flush => {
                        let mut state = state_clone.lock().unwrap();
                        state.pending.clear();
                        state.empty_callbacks = 0;
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

                    if got_real_sample || !state.pending.is_empty() {
                        state.empty_callbacks = 0;
                    } else {
                        state.empty_callbacks = state.empty_callbacks.saturating_add(1);
                        if state.empty_callbacks >= DRAIN_EMPTY_CALLBACKS {
                            state.empty_callbacks = 0;
                            let _ = event_tx.send(OutputEvent::Drained).ok();
                        }
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
        // Stop the output stream and wait for it to finish
        self._flag.store(false, Ordering::Relaxed);
        if let Some(handle) = self._handle.take() {
            let _ = handle.join();
        }
    }
}
