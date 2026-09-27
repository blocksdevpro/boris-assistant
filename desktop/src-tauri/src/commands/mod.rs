//! Tauri `invoke` handlers grouped by the command's job.
//!
//! Command and event names are part of the frontend IPC contract. Keep them
//! aligned with `desktop/src/bridge/ipc.ts`. Handlers adapt requests to host
//! state and pipeline helpers; voice policy belongs in `boris_pipeline`.

pub(crate) mod artifacts;
pub(crate) mod diagnostics;
pub(crate) mod engine;
pub(crate) mod models;
pub(crate) mod settings;
pub(crate) mod status;

/// Status snapshot push. Payload: `boris_pipeline::StatusPicture`.
pub const EVENT_STATUS: &str = "status";

/// Model download progress push. Payload: `boris_pipeline::DownloadProgress`.
pub const EVENT_MODELS_PROGRESS: &str = "models-progress";
