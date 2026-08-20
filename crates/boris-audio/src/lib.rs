//! Real-time audio I/O for Boris (cpal + rubato).
//!
//! # Layout
//!
//! | Module | Role |
//! |--------|------|
//! | [`service`] | [`AudioService`] — duplex mic + speaker used by the engine |
//! | [`input`] | Capture stream + resample worker |
//! | [`output`] | Playback stream + drain events |
//! | [`frontend`] | 16 kHz HPF + AGC + AEC after resample, before fan-out |
//! | [`resampler`] | FFT rate conversion; input/output wrappers |
//! | [`channels`] | Downmix / upmix helpers |
//! | [`buffer`] | Sliding + recording buffers for wake/hear |
//! | [`devices`] | Device enumeration |
//!
//! # Rates
//!
//! - Capture is resampled to [`AUDIO_TARGET_RATE`] (16 kHz mono), then run
//!   through the capture front-end, then fanned out to VAD/wake/STT.
//! - Playback accepts TTS-native mono PCM (`source_rate` on [`AudioService`]) and
//!   resamples to the device rate. A 16 kHz copy of that PCM is the AEC far-end.

pub mod buffer;
pub mod channels;
pub mod devices;
pub mod frontend;
pub mod input;
pub mod output;
pub mod resampler;
pub mod service;

// Preferred crate-root imports for hosts.
pub use boris_core::AUDIO_TARGET_RATE;
pub use devices::{DeviceInfo, Direction};
pub use frontend::{CaptureFrontEnd, FarEnd, FRAME_SAMPLES};
pub use output::{OutputCommand, OutputEvent};
pub use service::AudioService;

// Pipelines (`input::InputPipeline`, `output::OutputPipeline`) are crate-private;
// hosts go through [`AudioService`].
