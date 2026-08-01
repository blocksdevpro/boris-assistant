use std::sync::{Arc, Mutex};

use boris_core::{types::ArcAudioBuffer, AudioBuffer};
use cpal::{
    self,
    traits::{DeviceTrait, HostTrait},
    Device, DeviceId,
};

use crate::{
    input::InputPipeline,
    output::{OutputCommand, OutputEvent, OutputPipeline},
};

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

pub struct AudioService {
    input_pipeline: InputPipeline,
    input_subscribers: Arc<Mutex<Vec<crossbeam_channel::Sender<ArcAudioBuffer>>>>,
    output_event_channel: CrossBeamChannel<OutputEvent>,
    output_command_channel: CrossBeamChannel<OutputCommand>,
    output_pipeline: OutputPipeline,
    /// Sample rate of PCM passed to [`Self::play`] (must match TTS, e.g. Supertone 44.1 kHz).
    source_rate: u32,
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

    pub fn find_input_device(id: &DeviceId) -> Option<Device> {
        let host = cpal::default_host();
        host.device_by_id(id)
            .filter(|device| device.supports_input())
    }

    pub fn find_output_device(id: &DeviceId) -> Option<Device> {
        let host = cpal::default_host();
        host.device_by_id(id)
            .filter(|device| device.supports_output())
    }

    pub fn find_input_device_or_default(id: &DeviceId) -> Option<Device> {
        Self::find_input_device(id).or_else(|| cpal::default_host().default_input_device())
    }

    pub fn find_output_device_or_default(id: &DeviceId) -> Option<Device> {
        Self::find_output_device(id).or_else(|| cpal::default_host().default_output_device())
    }

    /// Build with default devices. `source_rate` is the rate of buffers given to [`Self::play`].
    ///
    /// Use the TTS native rate (Supertone = 44_100, Kokoro = 24_000). Wrong rate = slow/fast audio.
    ///
    /// Returns `Err` when no default input/output device is available (headless, denied
    /// permission, or no hardware) so callers can surface a fault instead of panicking.
    pub fn with_source_rate(source_rate: u32) -> Result<Self, String> {
        let host = cpal::default_host();
        // setup input pipeline
        let input_device = host.default_input_device().ok_or_else(|| {
            "No default microphone found. Connect a mic or grant audio input permission."
                .to_string()
        })?;
        let input_subscribers = Arc::new(Mutex::new(vec![]));
        let input_pipeline = InputPipeline::from_device(&input_device, input_subscribers.clone());

        // setup output pipeline
        let output_device = host.default_output_device().ok_or_else(|| {
            "No default speaker found. Connect a speaker/headphones or grant audio output permission."
                .to_string()
        })?;
        let output_event_channel = crossbeam_channel::bounded::<OutputEvent>(10);
        let output_command_channel = crossbeam_channel::bounded::<OutputCommand>(10);

        let output_event_tx = output_event_channel.0.clone();
        let output_command_rx = output_command_channel.1.clone();
        let output_pipeline = OutputPipeline::from_device(
            &output_device,
            output_command_rx,
            output_event_tx,
            source_rate,
        );
        Ok(Self {
            input_pipeline,
            input_subscribers,
            output_pipeline,
            output_event_channel,
            output_command_channel,
            source_rate,
        })
    }

    /// Defaults to 44.1 kHz play source (Supertone). Prefer [`Self::with_source_rate`] when known.
    pub fn new() -> Result<Self, String> {
        Self::with_source_rate(44_100)
    }

    pub fn subscribe_input(
        &mut self,
        queue: Option<usize>,
    ) -> crossbeam_channel::Receiver<ArcAudioBuffer> {
        let queue = queue.unwrap_or(64);
        let (tx, rx) = crossbeam_channel::bounded::<ArcAudioBuffer>(queue);
        {
            let mut subscribers = self.input_subscribers.lock().unwrap();
            subscribers.push(tx);
        }
        rx
    }

    /// Switch capture to `id`. Does **not** silently fall back to the default
    /// device — callers get `Err` if the id is missing or not an input.
    pub fn switch_input(&mut self, id: &DeviceId) -> Result<(), String> {
        if &self.input_pipeline.device_id == id {
            tracing::debug!(?id, "input already selected");
            return Ok(());
        }
        let device = Self::find_input_device(id).ok_or_else(|| {
            format!("input device not found (id={id:?}) — unplugged or no longer available")
        })?;
        let name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| format!("{id:?}"));
        tracing::info!(%name, "opening input device");
        // Drop old pipeline (stops stream) then open the new one; subscribers stay.
        self.input_pipeline = InputPipeline::from_device(&device, self.input_subscribers.clone());
        Ok(())
    }

    /// Switch playback to `id`. Does **not** silently fall back to default.
    pub fn switch_output(&mut self, id: &DeviceId) -> Result<(), String> {
        if &self.output_pipeline.device_id == id {
            tracing::debug!(?id, "output already selected");
            return Ok(());
        }
        let device = Self::find_output_device(id).ok_or_else(|| {
            format!("output device not found (id={id:?}) — unplugged or no longer available")
        })?;
        let name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| format!("{id:?}"));
        tracing::info!(%name, "opening output device");

        let output_event_channel = crossbeam_channel::bounded::<OutputEvent>(10);
        let output_command_channel = crossbeam_channel::bounded::<OutputCommand>(10);

        let output_event_tx = output_event_channel.0.clone();
        let output_command_rx = output_command_channel.1.clone();

        let output_pipeline = OutputPipeline::from_device(
            &device,
            output_command_rx,
            output_event_tx,
            self.source_rate,
        );
        self.output_command_channel = output_command_channel;
        self.output_event_channel = output_event_channel;
        self.output_pipeline = output_pipeline;
        Ok(())
    }

    pub fn play(&self, audio: AudioBuffer) {
        self.output_command_channel
            .0
            .try_send(OutputCommand::Play(audio))
            .unwrap();
    }

    pub fn stop(&self) {
        self.output_command_channel
            .0
            .try_send(OutputCommand::Flush)
            .unwrap();
    }

    pub fn subscribe_output(&self) -> crossbeam_channel::Receiver<OutputEvent> {
        self.output_event_channel.1.clone()
    }
}

#[cfg(test)]
mod tests {

    use std::println;

    use super::*;

    #[test]
    fn test_audio_service_input() {
        let mut service = AudioService::new().expect("default audio devices required for test");
        let rx = service.subscribe_input(None);

        if let Some(_audio) = rx.recv().ok() {
            println!("working ",)
        }
    }

    #[test]
    fn test_audio_service_input_switch() {
        let mut service = AudioService::new().expect("default audio devices required for test");
        let rx = service.subscribe_input(None);
        let devices = AudioService::list_input_devices();
        let device = devices.get(1).unwrap();
        println!("switching input to {}", device.name);
        service
            .switch_input(&device.id)
            .expect("switch to second input");
        if let Some(_audio) = rx.recv().ok() {
            println!("working ",)
        }
    }

    #[test]
    fn test_audio_service_output_switch() {
        let mut service = AudioService::with_source_rate(16_000)
            .expect("default audio devices required for test");

        let mut count = 0;

        let rx = service.subscribe_input(None);
        while let Some(audio) = rx.recv().ok() {
            count += 1;
            if count == 200 {
                let devices = AudioService::list_input_devices();
                let device = devices.get(1).unwrap();
                service
                    .switch_input(&device.id)
                    .expect("switch input mid-stream");
                println!("switching input device")
            }
            if count == 400 {
                let devices = AudioService::list_input_devices();
                let device = devices.get(2).unwrap();
                service
                    .switch_input(&device.id)
                    .expect("switch input mid-stream");
                println!("switching input device")
            }
            println!("playing audio, {:?}, {}", audio.len(), count);
            // ArcAudioBuffer is Arc<[f32]>; play still wants owned AudioBuffer (Vec).
            let buf: AudioBuffer = audio.to_vec();
            service.play(buf);
        }
    }
}
