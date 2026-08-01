//! UI-facing status — mirrors `desktop/src/bridge/types.ts`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum EngineState {
    Off,
    Starting,
    On,
    Fault,
}

/// Where the sequential turn loop is right now (display only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Phase {
    Off,
    Quiet,
    Armed,
    Hearing,
    Reading,
    Thinking,
    Talking,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceHealth {
    pub label: String,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusPicture {
    pub engine: EngineState,
    pub phase: Phase,
    /// Always present for the TS bridge (`null` when unset).
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub heard: Option<String>,
    #[serde(default)]
    pub said: Option<String>,
    pub mic: DeviceHealth,
    pub speaker: DeviceHealth,
    #[serde(default)]
    pub turn: Option<String>,
}

impl StatusPicture {
    pub fn off() -> Self {
        Self {
            engine: EngineState::Off,
            phase: Phase::Off,
            detail: None,
            heard: None,
            said: None,
            mic: DeviceHealth {
                label: "—".into(),
                ok: false,
            },
            speaker: DeviceHealth {
                label: "—".into(),
                ok: false,
            },
            turn: None,
        }
    }
}
