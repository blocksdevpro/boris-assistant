//! High-level duplex audio service used by the voice engine.

use std::sync::{Arc, Mutex};

use boris_core::{ArcAudioBuffer, AudioBuffer};
use cpal::{traits::HostTrait, DeviceId};

use crate::devices::{self, device_name};
use crate::input::{InputPipeline, InputSubscribers};
use crate::output::{OutputCommand, OutputEvent, OutputPipeline};

// Historical path: `boris_audio::service::{DeviceInfo, Direction}` (pipeline).
pub use crate::devices::{DeviceInfo, Direction};

type CommandChannel = (
    crossbeam_channel::Sender<OutputCommand>,
    crossbeam_channel::Receiver<OutputCommand>,
);
type EventChannel = (
    crossbeam_channel::Sender<OutputEvent>,
    crossbeam_channel::Receiver<OutputEvent>,
);

/// Default capacity for output lifecycle events.
const OUTPUT_EVENT_CAPACITY: usize = 32;
/// Default capacity for Play/Flush commands (Play carries large PCM).
const OUTPUT_COMMAND_CAPACITY: usize = 16;
/// Default fan-out queue size for input subscribers.
const DEFAULT_INPUT_SUBSCRIBER_QUEUE: usize = 64;
/// Default play source rate when unspecified (Supertone native).
const DEFAULT_SOURCE_RATE_HZ: u32 = 44_100;

/// Full-duplex audio: mic capture fan-out + TTS playback.
pub struct AudioService {
    input_pipeline: InputPipeline,
    input_subscribers: InputSubscribers,
    output_event_channel: EventChannel,
    output_command_channel: CommandChannel,
    output_pipeline: OutputPipeline,
    /// Sample rate of PCM passed to [`Self::play`] (must match TTS).
    source_rate: u32,
}

impl AudioService {
    // ── device discovery (delegates to [`crate::devices`]) ───────────────────

    /// List devices for capture or playback.
    pub fn list_devices(direction: Direction) -> Vec<DeviceInfo> {
        devices::list_devices(direction)
    }

    /// List input devices.
    pub fn list_input_devices() -> Vec<DeviceInfo> {
        devices::list_input_devices()
    }

    /// List output devices.
    pub fn list_output_devices() -> Vec<DeviceInfo> {
        devices::list_output_devices()
    }

    /// Resolve input device by id.
    pub fn find_input_device(id: &DeviceId) -> Option<cpal::Device> {
        devices::find_input_device(id)
    }

    /// Resolve output device by id.
    pub fn find_output_device(id: &DeviceId) -> Option<cpal::Device> {
        devices::find_output_device(id)
    }

    /// Input by id, or host default.
    pub fn find_input_device_or_default(id: &DeviceId) -> Option<cpal::Device> {
        devices::find_input_device_or_default(id)
    }

    /// Output by id, or host default.
    pub fn find_output_device_or_default(id: &DeviceId) -> Option<cpal::Device> {
        devices::find_output_device_or_default(id)
    }

    // ── construction ─────────────────────────────────────────────────────────

    /// Build with default devices. `source_rate` is the rate of buffers given to [`Self::play`].
    ///
    /// Use the TTS native rate (Supertone = 44_100, Kokoro = 24_000). Wrong rate = slow/fast audio.
    ///
    /// Returns `Err` when no default input/output device is available.
    pub fn with_source_rate(source_rate: u32) -> Result<Self, String> {
        let host = cpal::default_host();
        tracing::info!(source_rate, "AudioService::with_source_rate");

        let input_device = host.default_input_device().ok_or_else(|| {
            tracing::error!("no default input device from cpal host");
            "No default microphone found. Connect a mic or grant audio input permission."
                .to_string()
        })?;
        let input_name = device_name(&input_device);
        tracing::info!(%input_name, "opening default input device");

        let input_subscribers: InputSubscribers = Arc::new(Mutex::new(Vec::new()));
        let input_pipeline = InputPipeline::from_device(&input_device, input_subscribers.clone());
        tracing::info!(%input_name, "input pipeline open");

        let output_device = host.default_output_device().ok_or_else(|| {
            tracing::error!("no default output device from cpal host");
            "No default speaker found. Connect a speaker/headphones or grant audio output permission."
                .to_string()
        })?;
        let output_name = device_name(&output_device);
        tracing::info!(%output_name, source_rate, "opening default output device");

        let (output_event_channel, output_command_channel, output_pipeline) =
            open_output_pipeline(&output_device, source_rate);
        tracing::info!(%output_name, "output pipeline open");

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
        Self::with_source_rate(DEFAULT_SOURCE_RATE_HZ)
    }

    /// Configured play source rate (Hz).
    pub fn source_rate(&self) -> u32 {
        self.source_rate
    }

    // ── input ────────────────────────────────────────────────────────────────

    /// Subscribe to mono capture frames at [`boris_core::AUDIO_TARGET_RATE`].
    ///
    /// `queue` is the per-subscriber bound (default 64). Full queues drop frames.
    pub fn subscribe_input(
        &mut self,
        queue: Option<usize>,
    ) -> crossbeam_channel::Receiver<ArcAudioBuffer> {
        let queue = queue.unwrap_or(DEFAULT_INPUT_SUBSCRIBER_QUEUE);
        let (tx, rx) = crossbeam_channel::bounded::<ArcAudioBuffer>(queue);
        self.input_subscribers.lock().unwrap().push(tx);
        rx
    }

    /// Switch capture to `id`. Does **not** fall back to default on failure.
    pub fn switch_input(&mut self, id: &DeviceId) -> Result<(), String> {
        if &self.input_pipeline.device_id == id {
            tracing::debug!(?id, "input already selected");
            return Ok(());
        }
        let device = Self::find_input_device(id).ok_or_else(|| {
            format!("input device not found (id={id:?}) — unplugged or no longer available")
        })?;
        let name = device_name(&device);
        tracing::info!(%name, "opening input device");
        // Drop old pipeline (stops stream) then open the new one; subscribers stay.
        self.input_pipeline = InputPipeline::from_device(&device, self.input_subscribers.clone());
        Ok(())
    }

    // ── output ───────────────────────────────────────────────────────────────

    /// Switch playback to `id`. Does **not** fall back to default.
    ///
    /// Returns `Ok(true)` when the pipeline was rebuilt (in-flight Play is dropped).
    /// Returns `Ok(false)` when `id` was already selected.
    pub fn switch_output(&mut self, id: &DeviceId) -> Result<bool, String> {
        if &self.output_pipeline.device_id == id {
            tracing::debug!(?id, "output already selected");
            return Ok(false);
        }
        let device = Self::find_output_device(id).ok_or_else(|| {
            format!("output device not found (id={id:?}) — unplugged or no longer available")
        })?;
        let name = device_name(&device);
        tracing::info!(%name, "opening output device");

        let (output_event_channel, output_command_channel, output_pipeline) =
            open_output_pipeline(&device, self.source_rate);
        self.output_command_channel = output_command_channel;
        self.output_event_channel = output_event_channel;
        self.output_pipeline = output_pipeline;
        Ok(true)
    }

    /// Queue mono PCM at [`Self::source_rate`] for playback (blocking send).
    pub fn play(&self, audio: AudioBuffer) {
        if let Err(e) = self
            .output_command_channel
            .0
            .send(OutputCommand::Play(audio))
        {
            tracing::error!(error = %e, "AudioService::play — output worker gone");
        }
    }

    /// Stop / clear current playback as soon as possible.
    pub fn stop(&self) {
        if self
            .output_command_channel
            .0
            .try_send(OutputCommand::Flush)
            .is_err()
        {
            let _ = self.output_command_channel.0.send(OutputCommand::Flush);
        }
    }

    /// Clone the output event receiver (Started / Drained / Cleared).
    pub fn subscribe_output(&self) -> crossbeam_channel::Receiver<OutputEvent> {
        self.output_event_channel.1.clone()
    }
}

fn open_output_pipeline(
    device: &cpal::Device,
    source_rate: u32,
) -> (EventChannel, CommandChannel, OutputPipeline) {
    let output_event_channel = crossbeam_channel::bounded::<OutputEvent>(OUTPUT_EVENT_CAPACITY);
    let output_command_channel =
        crossbeam_channel::bounded::<OutputCommand>(OUTPUT_COMMAND_CAPACITY);

    let output_event_tx = output_event_channel.0.clone();
    let output_command_rx = output_command_channel.1.clone();
    let output_pipeline =
        OutputPipeline::from_device(device, output_command_rx, output_event_tx, source_rate);

    (
        output_event_channel,
        output_command_channel,
        output_pipeline,
    )
}
