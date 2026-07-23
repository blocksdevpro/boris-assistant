pub type AudioBuffer = Vec<f32>;
pub type ArcAudioBuffer = Arc<AudioBuffer>;

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

use std::{
    sync::{Arc, Mutex},
    thread,
};

use cpal::{
    self,
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, DeviceId, FromSample, Sample, SampleFormat, Stream, StreamConfig,
};

use crate::resampler::Resampler;

type CrossBeamChannel<T> = (crossbeam_channel::Sender<T>, crossbeam_channel::Receiver<T>);

pub struct InputStream {
    _stream: Stream,
    pub device_id: DeviceId,
    pub sample_rate: u32,
    pub channels: u32,
}

impl InputStream {
    pub fn from_device(device: &Device, audio_tx: crossbeam_channel::Sender<AudioBuffer>) -> Self {
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
                build_input_stream::<f32>(device, stream_config, channels as usize, audio_tx)
            }
            SampleFormat::I16 => {
                build_input_stream::<i16>(device, stream_config, channels as usize, audio_tx)
            }
            SampleFormat::U16 => {
                build_input_stream::<u16>(device, stream_config, channels as usize, audio_tx)
            }
            _ => panic!("Unsupported sample format"),
        };

        stream.play().unwrap();

        Self {
            _stream: stream,
            channels: channels as u32,
            device_id: device.id().unwrap(),
            sample_rate,
        }
    }
}

pub struct OutputStream {
    _stream: Stream,
}

pub struct AudioService {
    input_channel: CrossBeamChannel<AudioBuffer>,
    input_stream: InputStream,
    input_subscribers: Arc<Mutex<Vec<crossbeam_channel::Sender<ArcAudioBuffer>>>>,
    input_handle: Option<thread::JoinHandle<()>>,
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
        let input_channel = crossbeam_channel::bounded::<AudioBuffer>(20);

        let input_stream = InputStream::from_device(&input_device, input_channel.0.clone());

        Self {
            input_channel,
            input_stream,
            input_handle: None,
            input_subscribers: Arc::new(Mutex::new(vec![])),
        }
    }

    pub fn spawn_input(&mut self) {
        let audio_rx = self.input_channel.1.clone();
        let mut input_resampler = Resampler::new(
            self.input_stream.channels,
            self.input_stream.sample_rate,
            16_000,
        );
        let input_subscribers = self.input_subscribers.clone();
        let handle = thread::spawn(move || {
            while let Ok(audio) = audio_rx.recv() {
                let resampled = input_resampler.resample(&audio).unwrap();
                let arc_resampled = Arc::new(resampled);
                {
                    let subscribers = input_subscribers.lock().unwrap();
                    for subscriber in subscribers.iter() {
                        subscriber.try_send(arc_resampled.clone()).ok();
                    }
                }
            }
        });
        self.input_handle = Some(handle);
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
        if &self.input_stream.device_id != id {
            if let Some(device) = Self::find_input_device_or_default(id) {
                let audio_tx = self.input_channel.0.clone();
                let input_stream = InputStream::from_device(&device, audio_tx);
                self.input_stream = input_stream;
                self.spawn_input();
            }
        } else {
            println!("already using device {}", id);
        }
    }
}

pub type AudioSample = f32;

fn build_input_stream<T>(
    device: &Device,
    config: StreamConfig,
    channels: usize,
    audio_tx: crossbeam_channel::Sender<AudioBuffer>,
) -> Stream
where
    T: Sample + cpal::SizedSample + 'static,
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

#[cfg(test)]
mod tests {

    use std::println;

    use super::*;

    #[test]
    fn test_audio_service_input() {
        let mut service = AudioService::new();
        service.spawn_input();
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
