//! High-level duplex audio service used by the voice engine.

use std::sync::{Arc, Mutex};

use boris_core::{ArcAudioBuffer, AudioBuffer, Error, Result};
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
/// Maximum time a lifecycle control command may take to reach and be applied
/// by the output worker. This is intentionally bounded so a dead worker cannot
/// strand the engine in Talking forever.
const OUTPUT_CONTROL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
/// Small bounded enqueue wait for streamed PCM. The engine produces at most a
/// sentence at a time; a short wait absorbs worker resampling jitter without
/// making Stop handling feel unresponsive.
const OUTPUT_APPEND_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);
/// Stop must remain responsive if a failed worker stops consuming commands.
const OUTPUT_STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

/// Full-duplex audio: mic capture fan-out + TTS playback.
///
/// # Field order
///
/// `output_command_channel` is declared before `output_pipeline` for readability
/// (channels grouped near the pipelines that use them), but this ordering is
/// **not** load-bearing for correctness anymore: `OutputPipeline`'s worker wakes
/// itself via a `recv_timeout` poll (see `output::OUTPUT_WORKER_SHUTDOWN_POLL`)
/// rather than relying on `output_command_channel`'s `Sender` being dropped
/// before `OutputPipeline::drop()` joins the worker. Historically it *was*
/// load-bearing — Rust drops struct fields in declaration order, and the old
/// worker blocked on `cmd_rx.recv()` until its `Sender` closed — so reordering
/// these fields would have silently reintroduced a shutdown hang.
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
    /// Returns `Err` when no default input/output device is available, or when
    /// opening a device stream fails (format unsupported, permission denied, etc.).
    pub fn with_source_rate(source_rate: u32) -> Result<Self> {
        let host = cpal::default_host();
        tracing::info!(source_rate, "AudioService::with_source_rate");

        let input_device = host.default_input_device().ok_or_else(|| {
            tracing::error!("no default input device from cpal host");
            Error::audio(
                "No default microphone found. Connect a mic or grant audio input permission.",
            )
        })?;
        let input_name = device_name(&input_device);
        tracing::info!(%input_name, "opening default input device");

        let input_subscribers: InputSubscribers = Arc::new(Mutex::new(Vec::new()));
        let input_pipeline = InputPipeline::from_device(&input_device, input_subscribers.clone())?;
        tracing::info!(%input_name, "input pipeline open");

        let output_device = host.default_output_device().ok_or_else(|| {
            tracing::error!("no default output device from cpal host");
            Error::audio(
                "No default speaker found. Connect a speaker/headphones or grant audio output permission.",
            )
        })?;
        let output_name = device_name(&output_device);
        tracing::info!(%output_name, source_rate, "opening default output device");

        let (output_event_channel, output_command_channel, output_pipeline) =
            open_output_pipeline(&output_device, source_rate)?;
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
    pub fn new() -> Result<Self> {
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
        match self.input_subscribers.lock() {
            Ok(mut g) => g.push(tx),
            Err(poisoned) => {
                tracing::error!("AudioService: input subscriber mutex poisoned — recovering");
                poisoned.into_inner().push(tx);
            }
        }
        rx
    }

    /// Switch capture to `id`. Does **not** fall back to default on failure.
    ///
    /// On stream open failure the previous input pipeline is left running.
    pub fn switch_input(&mut self, id: &DeviceId) -> Result<()> {
        if &self.input_pipeline.device_id == id {
            tracing::debug!(?id, "input already selected");
            return Ok(());
        }
        let device = Self::find_input_device(id).ok_or_else(|| {
            Error::audio(format!(
                "input device not found (id={id:?}) — unplugged or no longer available"
            ))
        })?;
        let name = device_name(&device);
        tracing::info!(%name, "opening input device");
        // Open new pipeline before replacing so a failure keeps the old stream.
        let new_pipeline = InputPipeline::from_device(&device, self.input_subscribers.clone())?;
        self.input_pipeline = new_pipeline;
        Ok(())
    }

    // ── output ───────────────────────────────────────────────────────────────

    /// Switch playback to `id`. Does **not** fall back to default.
    ///
    /// Returns `Ok(true)` when the pipeline was rebuilt (in-flight Play is dropped).
    /// Returns `Ok(false)` when `id` was already selected.
    ///
    /// On open failure the previous output pipeline is left running.
    pub fn switch_output(&mut self, id: &DeviceId) -> Result<bool> {
        if &self.output_pipeline.device_id == id {
            tracing::debug!(?id, "output already selected");
            return Ok(false);
        }
        let device = Self::find_output_device(id).ok_or_else(|| {
            Error::audio(format!(
                "output device not found (id={id:?}) — unplugged or no longer available"
            ))
        })?;
        let name = device_name(&device);
        tracing::info!(%name, "opening output device");

        let (output_event_channel, output_command_channel, output_pipeline) =
            open_output_pipeline(&device, self.source_rate)?;
        self.output_command_channel = output_command_channel;
        self.output_event_channel = output_event_channel;
        self.output_pipeline = output_pipeline;
        Ok(true)
    }

    /// Queue mono PCM at [`Self::source_rate`] for playback.
    ///
    /// Uses non-blocking `try_send`. Returns `Err` when the command queue is full
    /// (backpressure — caller may retry) or the output worker is gone.
    pub fn play(&self, audio: AudioBuffer) -> Result<()> {
        self.send_play_cmd(OutputCommand::Play(audio))
    }

    /// Append PCM to the current play job (streaming units). Starts a job if idle.
    /// Waits briefly for queue capacity so a sentence is not silently dropped.
    pub fn append(&self, audio: AudioBuffer) -> Result<()> {
        match self
            .output_command_channel
            .0
            .send_timeout(OutputCommand::Append(audio), OUTPUT_APPEND_TIMEOUT)
        {
            Ok(()) => Ok(()),
            Err(crossbeam_channel::SendTimeoutError::Timeout(_)) => Err(Error::audio(
                "output command queue stayed full while appending streamed audio",
            )),
            Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => Err(Error::audio(
                "output worker gone while appending streamed audio",
            )),
        }
    }

    /// Close a streaming play job so [`OutputEvent::Drained`] can fire.
    ///
    /// Unlike PCM enqueueing, this lifecycle transition is acknowledged by the
    /// output worker. Success therefore means `job_open` is definitely false;
    /// failures are bounded by a timeout and must be handled by stopping the job.
    pub fn finish_job(&self) -> Result<()> {
        let (ack_tx, ack_rx) = crossbeam_channel::bounded(1);
        match self
            .output_command_channel
            .0
            .send_timeout(OutputCommand::FinishJob(ack_tx), OUTPUT_CONTROL_TIMEOUT)
        {
            Ok(()) => {}
            Err(crossbeam_channel::SendTimeoutError::Timeout(_)) => {
                return Err(Error::audio(
                    "output worker did not accept FinishJob before timeout",
                ));
            }
            Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => {
                return Err(Error::audio("output worker gone while finishing play job"));
            }
        }

        match ack_rx.recv_timeout(OUTPUT_CONTROL_TIMEOUT) {
            Ok(()) => Ok(()),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => Err(Error::audio(
                "output worker did not acknowledge FinishJob before timeout",
            )),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Err(Error::audio(
                "output worker dropped FinishJob acknowledgement",
            )),
        }
    }

    /// Try to enqueue an acknowledged streaming-job close without blocking.
    ///
    /// The returned receiver resolves only after the output worker has applied
    /// the close. Event-loop hosts can retry a queue-full error while continuing
    /// to process Stop/device-switch commands, then poll the acknowledgement.
    pub fn request_finish_job(&self) -> Result<crossbeam_channel::Receiver<()>> {
        let (ack_tx, ack_rx) = crossbeam_channel::bounded(1);
        match self
            .output_command_channel
            .0
            .try_send(OutputCommand::FinishJob(ack_tx))
        {
            Ok(()) => Ok(ack_rx),
            Err(crossbeam_channel::TrySendError::Full(_)) => Err(Error::audio(
                "output command queue full while requesting FinishJob",
            )),
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => Err(Error::audio(
                "output worker gone while requesting FinishJob",
            )),
        }
    }

    fn send_play_cmd(&self, cmd: OutputCommand) -> Result<()> {
        match self.output_command_channel.0.try_send(cmd) {
            Ok(()) => Ok(()),
            Err(crossbeam_channel::TrySendError::Full(_)) => Err(Error::audio(
                "output command queue full — play dropped (backpressure)",
            )),
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                tracing::error!("AudioService::play — output worker gone");
                Err(Error::audio("output worker gone"))
            }
        }
    }

    /// Stop / clear current playback as soon as possible.
    ///
    /// Prefer `try_send`; briefly wait for capacity if the queue is full. The
    /// fallback is bounded so a failed output worker cannot hang shutdown.
    pub fn stop(&self) {
        if let Err(error) = enqueue_flush(&self.output_command_channel.0, OUTPUT_STOP_TIMEOUT) {
            tracing::error!(%error, "AudioService::stop failed");
        }
    }

    /// Clone the output event receiver (Started / Drained / Cleared).
    pub fn subscribe_output(&self) -> crossbeam_channel::Receiver<OutputEvent> {
        self.output_event_channel.1.clone()
    }
}

fn enqueue_flush(
    tx: &crossbeam_channel::Sender<OutputCommand>,
    timeout: std::time::Duration,
) -> Result<()> {
    match tx.try_send(OutputCommand::Flush) {
        Ok(()) => Ok(()),
        Err(crossbeam_channel::TrySendError::Full(command)) => {
            match tx.send_timeout(command, timeout) {
                Ok(()) => Ok(()),
                Err(crossbeam_channel::SendTimeoutError::Timeout(_)) => Err(Error::audio(
                    "output command queue stayed full while stopping playback",
                )),
                Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => {
                    Err(Error::audio("output worker gone while stopping playback"))
                }
            }
        }
        Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
            Err(Error::audio("output worker gone while stopping playback"))
        }
    }
}

fn open_output_pipeline(
    device: &cpal::Device,
    source_rate: u32,
) -> Result<(EventChannel, CommandChannel, OutputPipeline)> {
    let output_event_channel = crossbeam_channel::bounded::<OutputEvent>(OUTPUT_EVENT_CAPACITY);
    let output_command_channel =
        crossbeam_channel::bounded::<OutputCommand>(OUTPUT_COMMAND_CAPACITY);

    let output_event_tx = output_event_channel.0.clone();
    let output_command_rx = output_command_channel.1.clone();
    let output_pipeline =
        OutputPipeline::from_device(device, output_command_rx, output_event_tx, source_rate)?;

    Ok((
        output_event_channel,
        output_command_channel,
        output_pipeline,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flush_enqueue_has_a_hard_capacity_deadline() {
        let (tx, _rx) = crossbeam_channel::bounded(1);
        tx.send(OutputCommand::Flush).unwrap();
        let started = std::time::Instant::now();
        let error = enqueue_flush(&tx, std::time::Duration::from_millis(30)).unwrap_err();
        assert!(error.to_string().contains("stayed full"));
        assert!(started.elapsed() < std::time::Duration::from_millis(250));
    }

    #[test]
    fn flush_enqueue_reports_disconnected_worker() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        drop(rx);
        let error = enqueue_flush(&tx, std::time::Duration::from_millis(30)).unwrap_err();
        assert!(error.to_string().contains("worker gone"));
    }
}
