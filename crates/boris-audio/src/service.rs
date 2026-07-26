use std::{
    collections::VecDeque,
    panic, println,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};

use cpal::{
    self,
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, DeviceId, FromSample, Sample, SampleFormat, Stream, StreamConfig,
};

use crate::resampler::{InputResampler, OutputResampler};

pub type AudioSample = f32;
pub type AudioBuffer = Vec<f32>;
pub type ArcAudioBuffer = Arc<AudioBuffer>;

type CrossBeamChannel<T> = (crossbeam_channel::Sender<T>, crossbeam_channel::Receiver<T>);

pub enum Direction {
    Input,
    Output,
}

#[derive(Debug)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub name: String,
    pub is_default: bool,
}

pub struct InputPipeline {
    _stream: Stream,
    _flag: Arc<AtomicBool>,
    _handle: Option<JoinHandle<()>>,
    device_id: DeviceId,
    _subscribers: Arc<Mutex<Vec<crossbeam_channel::Sender<ArcAudioBuffer>>>>,
}

impl InputPipeline {
    pub fn from_device(
        device: &Device,
        subscribers: Arc<Mutex<Vec<crossbeam_channel::Sender<ArcAudioBuffer>>>>,
    ) -> Self {
        // instance params
        let flag = Arc::new(AtomicBool::new(false));
        let (audio_tx, audio_rx) = crossbeam_channel::bounded::<AudioBuffer>(10);

        // device params
        let device_id = device.id().unwrap();
        let config = device.default_input_config().unwrap();
        let channels = config.channels();
        let sample_rate = config.sample_rate();

        let stream_config = StreamConfig {
            channels,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        let stream = match config.sample_format() {
            SampleFormat::F32 => Self::build_stream::<f32>(device, stream_config, audio_tx),
            SampleFormat::I16 => Self::build_stream::<i16>(device, stream_config, audio_tx),
            SampleFormat::U16 => Self::build_stream::<u16>(device, stream_config, audio_tx),
            _ => panic!("Unsupported sample format for InputStream"),
        };

        // play the stream;
        stream.play().unwrap();

        // Capture stays interleaved at device channel count; InputResampler
        // downmixes to mono then converts rate → AUDIO_TARGET_RATE.
        let mut resampler = InputResampler::new(channels as u32, sample_rate);
        let flag_clone = flag.clone();
        let subscribers_clone = subscribers.clone();

        // audio worker thread
        let handle = thread::spawn(move || {
            while let Ok(audio) = audio_rx.recv() {
                if flag_clone.load(Ordering::Relaxed) {
                    println!("droped");
                    break;
                }

                match resampler.process(&audio) {
                    Ok(resampled) => {
                        let arc_resampled = Arc::new(resampled);
                        let subs = subscribers_clone.lock().unwrap();
                        for sub in subs.iter() {
                            let _ = sub.try_send(arc_resampled.clone());
                        }
                    }
                    Err(e) => {
                        println!("resample failed: {e}");
                    }
                }
            }
        });

        Self {
            device_id,
            _flag: flag,
            _stream: stream,
            _handle: Some(handle),
            _subscribers: subscribers,
        }
    }

    /// Capture callback: convert device samples to interleaved f32 only.
    /// Channel downmix + rate conversion happen in `InputResampler`.
    fn build_stream<T>(
        device: &Device,
        config: StreamConfig,
        audio_tx: crossbeam_channel::Sender<AudioBuffer>,
    ) -> Stream
    where
        T: cpal::Sample + cpal::SizedSample + 'static,
        AudioSample: FromSample<T>,
    {
        let stream = device
            .build_input_stream(
                config,
                move |data: &[T], _| {
                    // Normalize every device format (i16/u16/f32/…) into f32 [-1, 1].
                    // Keep interleaved N-channel layout; InputResampler owns downmix.
                    let samples: Vec<AudioSample> = data
                        .iter()
                        .map(|sample| AudioSample::from_sample(*sample))
                        .collect();
                    audio_tx.try_send(samples).ok();
                },
                |err| tracing::error!("InputStream error: {err}"),
                None,
            )
            .unwrap();

        stream
    }
}

impl Drop for InputPipeline {
    fn drop(&mut self) {
        // set the shutdown_flag to true;
        self._flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self._handle.take() {
            //join the thread;
            handle.join().ok();
        }
    }
}

pub struct OutputPipeline {
    _stream: Stream,
    _flag: Arc<AtomicBool>,
    _handle: Option<JoinHandle<()>>,
    device_id: DeviceId,
    _state: Arc<Mutex<OutputStreamState>>,
}

impl OutputPipeline {
    pub fn from_device(
        device: &Device,
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
            SampleFormat::F32 => {
                Self::build_stream::<f32>(device, stream_config, state_clone, event_tx)
            }
            SampleFormat::I32 => {
                Self::build_stream::<i32>(device, stream_config, state_clone, event_tx)
            }
            SampleFormat::U32 => {
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
        config: StreamConfig,
        state: Arc<Mutex<OutputStreamState>>,
        event_tx: crossbeam_channel::Sender<OutputEvent>,
    ) -> Stream
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

pub struct AudioService {
    input_pipeline: InputPipeline,
    input_subscribers: Arc<Mutex<Vec<crossbeam_channel::Sender<ArcAudioBuffer>>>>,
    output_event_channel: CrossBeamChannel<OutputEvent>,
    output_command_channel: CrossBeamChannel<OutputCommand>,
    output_pipeline: OutputPipeline,
}

impl AudioService {
    // Pub Struct Methods
    pub fn list_devices(direction: Direction) -> Vec<DeviceInfo> {
        let host = cpal::default_host();
        let default_id = match direction {
            Direction::Input => host.default_input_device(),
            Direction::Output => host.default_output_device(),
        }
        .and_then(|device| device.id().ok());

        let devices = match direction {
            Direction::Input => host.input_devices(),
            Direction::Output => host.output_devices(),
        };

        devices
            .into_iter()
            .flatten()
            .filter_map(|device| {
                let id = device.id().ok()?;
                let description = device.description().ok()?;

                Some(DeviceInfo {
                    is_default: default_id.as_ref() == Some(&id),
                    name: description.name().to_string(),
                    id,
                })
            })
            .collect()
    }

    pub fn list_input_devices() -> Vec<DeviceInfo> {
        Self::list_devices(Direction::Input)
    }

    pub fn list_output_devices() -> Vec<DeviceInfo> {
        Self::list_devices(Direction::Output)
    }

    pub fn find_input_device_or_default(id: &DeviceId) -> Option<Device> {
        let host = cpal::default_host();
        if let Some(device) = host.device_by_id(id) {
            if device.supports_input() {
                return Some(device);
            }
        }
        host.default_input_device()
    }

    pub fn find_output_device_or_default(id: &DeviceId) -> Option<Device> {
        let host = cpal::default_host();
        if let Some(device) = host.device_by_id(id) {
            if device.supports_output() {
                return Some(device);
            }
        }
        host.default_output_device()
    }

    pub fn new() -> Self {
        let host = cpal::default_host();
        // setup input pipeline
        let input_device = host.default_input_device().unwrap();
        let input_subscribers = Arc::new(Mutex::new(vec![]));
        let input_pipeline = InputPipeline::from_device(&input_device, input_subscribers.clone());

        // setup output pipeline
        let output_device = host.default_output_device().unwrap();
        let output_event_channel = crossbeam_channel::bounded::<OutputEvent>(10);
        let output_command_channel = crossbeam_channel::bounded::<OutputCommand>(10);

        let output_event_tx = output_event_channel.0.clone();
        let output_command_rx = output_command_channel.1.clone();
        let output_pipeline =
            OutputPipeline::from_device(&output_device, output_command_rx, output_event_tx, 24_000);
        Self {
            input_pipeline,
            input_subscribers,
            output_pipeline,
            output_event_channel,
            output_command_channel,
        }
    }

    pub fn subscribe_input(&mut self) -> crossbeam_channel::Receiver<ArcAudioBuffer> {
        let (tx, rx) = crossbeam_channel::bounded::<ArcAudioBuffer>(10);
        {
            let mut subscribers = self.input_subscribers.lock().unwrap();
            subscribers.push(tx);
        }
        rx
    }

    pub fn switch_input(&mut self, id: &DeviceId) {
        if &self.input_pipeline.device_id != id {
            if let Some(device) = Self::find_input_device_or_default(id) {
                let input_pipeline =
                    InputPipeline::from_device(&device, self.input_subscribers.clone());
                self.input_pipeline = input_pipeline;
            }
        } else {
            println!("already using device {}", id);
        }
    }

    pub fn switch_output(&mut self, id: &DeviceId) {
        if &self.output_pipeline.device_id != id {
            if let Some(device) = Self::find_output_device_or_default(id) {
                let output_event_channel = crossbeam_channel::bounded::<OutputEvent>(10);
                let output_command_channel = crossbeam_channel::bounded::<OutputCommand>(10);

                let output_event_tx = output_event_channel.0.clone();
                let output_command_rx = output_command_channel.1.clone();

                let output_pipeline = OutputPipeline::from_device(
                    &device,
                    output_command_rx,
                    output_event_tx,
                    24_000,
                );
                self.output_command_channel = output_command_channel;
                self.output_event_channel = output_event_channel;
                self.output_pipeline = output_pipeline;
            }
        }
    }

    pub fn play(&mut self, audio: AudioBuffer) {
        self.output_command_channel
            .0
            .try_send(OutputCommand::Play(audio))
            .unwrap();
    }
}

impl Default for AudioService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {

    use std::println;

    use super::*;

    #[test]
    fn test_audio_service_input() {
        let mut service = AudioService::new();
        let rx = service.subscribe_input();

        if let Some(_audio) = rx.recv().ok() {
            println!("working ",)
        }
    }

    #[test]
    fn test_audio_service_input_switch() {
        let mut service = AudioService::new();
        let rx = service.subscribe_input();
        let devices = AudioService::list_input_devices();
        let device = devices.get(1).unwrap();
        println!("switching input to {}", device.name);
        service.switch_input(&device.id);
        if let Some(_audio) = rx.recv().ok() {
            println!("working ",)
        }
    }

    #[test]
    fn test_audio_service_output_switch() {
        let mut service = AudioService::new();

        let mut count = 0;

        let rx = service.subscribe_input();
        while let Some(audio) = rx.recv().ok() {
            count += 1;
            if count == 200 {
                let devices = AudioService::list_input_devices();
                let device = devices.get(1).unwrap();
                service.switch_input(&device.id);
                println!("switching input device")
            }
            if count == 400 {
                let devices = AudioService::list_input_devices();
                let device = devices.get(2).unwrap();
                service.switch_input(&device.id);
                println!("switching input device")
            }
            println!("playing audio, {:?}, {}", audio.len(), count);
            let buf: AudioBuffer = (*audio).clone();
            service.play(buf);
        }
    }
}
