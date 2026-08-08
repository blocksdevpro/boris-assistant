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
//!
//! # Crate map (where to change what)
//!
//! | Concern | Module |
//! |---------|--------|
//! | Wake / STT / agent turn / talk loop | [`engine`] (+ submodules) |
//! | Mic capture / VAD / wake wait helpers | [`hear`] |
//! | UI status DTO | [`status`] |
//! | `~/.boris` layout, preflight | [`paths`] |
//! | User prefs + secrets | [`settings`] |
//! | Model HTTP install | [`download`] |
//! | Host spawn config | [`config`] |
//! | Device enumeration DTO | [`devices`] |
//! | Startup / model-load diagnostics | [`diagnostics`] |
//! | System prompt text | [`prompt`] |

pub mod config;
pub mod devices;
pub mod diagnostics;
pub mod download;
pub mod engine;
pub mod hear;
pub mod paths;
pub mod prompt;
pub mod settings;
pub mod status;

pub use config::PipelineConfig;
pub use devices::DeviceDto;
pub use diagnostics::{log_environment, log_model_load_failure};
pub use download::{
    install_models, models_status, DownloadFileStatus, DownloadProgress, ModelComponent,
    ModelsInstallReport, ModelsStatus, BORIS_MODEL_BASE_URL_ENV,
};
pub use engine::{Engine, EngineCommand, EngineHandle};
pub use paths::{
    auth_path, boris_home, config_path, ensure_logs_dir, ensure_sessions_dir, logs_dir,
    memory_dir, migrate_home_if_needed, models_dir, notes_path, preflight, profile_path,
    sessions_dir, sessions_root, supertone_onnx_is_multilingual, supertone_version_problem,
    workspace_dir, PreflightReport, BORIS_HOME_ENV, DESKTOP_WORKSPACE,
};
pub use prompt::BORIS_SYSTEM_PROMPT;
pub use settings::{load_settings, save_settings, secrets_path, settings_path, AppSettings};
pub use status::{DeviceHealth, EngineState, Phase, StatusPicture};
