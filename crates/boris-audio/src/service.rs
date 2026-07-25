use std::{
    collections::VecDeque,
    panic,
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

use crate::resampler::Resampler;

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
    _handle: Option<JoinHandle<()>>,
    _flag: Arc<AtomicBool>,
    channel: CrossBeamChannel<AudioBuffer>,
    device_id: DeviceId,
    subscribers: Arc<Mutex<Vec<crossbeam_channel::Sender<ArcAudioBuffer>>>>,
}

impl InputPipeline {
    pub fn from_device(
        device: &Device,
        subscribers: Arc<Mutex<Vec<crossbeam_channel::Sender<ArcAudioBuffer>>>>,
    ) -> Self {
        // instance params
        let flag = Arc::new(AtomicBool::new(false));
        let channel = crossbeam_channel::bounded::<AudioBuffer>(10);
        let (audio_tx, audio_rx) = (channel.0.clone(), channel.1.clone());

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
            SampleFormat::F32 => {
                Self::build_stream::<f32>(device, stream_config, channels as usize, audio_tx)
            }
            SampleFormat::I16 => {
                Self::build_stream::<i16>(device, stream_config, channels as usize, audio_tx)
            }
            SampleFormat::U16 => {
                Self::build_stream::<u16>(device, stream_config, channels as usize, audio_tx)
            }
            _ => panic!("Unsupported sample format for InputStream"),
        };

        stream.play().unwrap();

        let mut resampler = Resampler::new(1, sample_rate, 16_000);
        let flag_clone = flag.clone();
        let subscribers_clone = subscribers.clone();

        let handle = thread::spawn(move || {
            while let Ok(audio) = audio_rx.recv() {
                if flag_clone.load(Ordering::Relaxed) {
                    println!("droped");
                    break;
                }
                let resampled = resampler.resample(&audio).unwrap();
                let arc_resampled = Arc::new(resampled);

                {
                    let subs = subscribers_clone.lock().unwrap();
                    for sub in subs.iter() {
                        sub.try_send(arc_resampled.clone()).unwrap();
                    }
                };
            }
        });

        Self {
            _stream: stream,
            _handle: Some(handle),
            _flag: flag,
            channel: channel,
            device_id: device_id,
            subscribers: subscribers,
        }
    }

    fn build_stream<T>(
        device: &Device,
        config: StreamConfig,
        channels: usize,
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
                    // Raw `Into<f32>` on integer samples yields ±32768-scale values and
                    // saturates every downstream PCM path that clamps to [-1, 1].
                    let mono_samples: Vec<AudioSample> = data
                        .chunks(channels)
                        .map(|frame| {
                            frame
                                .iter()
                                .map(|sample| AudioSample::from_sample(*sample))
                                .sum::<AudioSample>()
                                / channels as AudioSample
                        })
                        .collect();
                    audio_tx.try_send(mono_samples).ok();
                },
                |err| tracing::error!("Audio capture failed: {err}"),
                None,
            )
            .unwrap();

        stream
    }
}

impl Drop for InputPipeline {
    fn drop(&mut self) {
        self._flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self._handle.take() {
            handle.join().ok();
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

pub struct OutputStream {
    _stream: Stream,
    pub channels: u32,
    pub sample_rate: u32,
    pub device_id: DeviceId,
    state: Arc<Mutex<OutputStreamState>>,
}

impl OutputStream {
    pub fn from_device(
        device: &Device,
        command_rx: crossbeam_channel::Receiver<OutputCommand>,
        event_tx: crossbeam_channel::Sender<OutputEvent>,
    ) -> Self {
        let config = device.default_output_config().unwrap();
        let channels = config.channels();
        let sample_rate = config.sample_rate();

        let state = Arc::new(Mutex::new(OutputStreamState {
            pending: VecDeque::new(),
            empty_callbacks: 0,
        }));

        let state_clone = Arc::clone(&state);
        let stream = match config.sample_format() {
            SampleFormat::F32 => {
                Self::build_stream::<f32>(device, config.config(), state_clone, event_tx)
            }
            SampleFormat::I16 => {
                Self::build_stream::<i16>(device, config.config(), state_clone, event_tx)
            }
            SampleFormat::U16 => {
                Self::build_stream::<u16>(device, config.config(), state_clone, event_tx)
            }
            _ => panic!("Unsupported sample format for OutputStream"),
        };

        stream.play().unwrap();

        Self {
            _stream: stream,
            channels: channels as u32,
            sample_rate,
            device_id: device.id().unwrap(),
            state,
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
                |err| tracing::error!("PlaybackSink stream error: {err}"),
                None,
            )
            .unwrap();
        stream
    }
}

pub struct AudioService {
    input_pipeline: InputPipeline,
    input_subscribers: Arc<Mutex<Vec<crossbeam_channel::Sender<ArcAudioBuffer>>>>,
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
        let input_device = host.default_input_device().unwrap();
        let input_subscribers = Arc::new(Mutex::new(vec![]));
        let input_pipeline = InputPipeline::from_device(&input_device, input_subscribers.clone());

        Self {
            input_pipeline,
            input_subscribers,
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
}
