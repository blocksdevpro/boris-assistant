//! Desktop voice pipeline — one engine thread, sequential turns.
//!
//! Not a worker mesh and not a Session FSM. The engine runs a straight loop:
//!
//! ```text
//! Start → Armed → (wake) → hear → read → think → talk → Armed → …
//! ```
//!
//! Wake scoring, VAD capture, STT, agent, and TTS are **called inline** on that
//! thread (or briefly block it). Status is pushed for the UI. Hosts send
//! [`EngineCommand`] via [`EngineHandle`].

pub mod config;
pub mod devices;
pub mod engine;
pub mod hear;
pub mod paths;
pub mod prompt;
pub mod status;

pub use config::PipelineConfig;
pub use devices::DeviceDto;
pub use engine::{Engine, EngineCommand, EngineHandle};
pub use paths::{boris_home, models_dir, BORIS_HOME_ENV};
pub use prompt::BORIS_SYSTEM_PROMPT;
pub use status::{DeviceHealth, EngineState, Phase, StatusPicture};
