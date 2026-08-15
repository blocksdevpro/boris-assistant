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
    /// Waiting for a freeform user reply without another wake word.
    AwaitingReply,
    /// Waiting for yes/no after a dangerous tool confirmation prompt.
    AwaitingConfirm,
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

/// Compact pointer to the session's current visual card (no body).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactPeek {
    pub id: String,
    pub title: String,
    /// `markdown` or `code`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Filename under the session `artifacts/` dir.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusPicture {
    pub engine: EngineState,
    pub phase: Phase,
    /// Error / fault text only (not confirm prompts — those use `activity`).
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
    /// Compact progressive status (tool name, confirm summary) for the overlay.
    #[serde(default)]
    pub activity: Option<String>,
    /// Estimated context tokens used (chars/4 heuristic).
    #[serde(default)]
    pub context_used: Option<u32>,
    /// Soft context window for the meter (tokens).
    #[serde(default)]
    pub context_limit: Option<u32>,
    /// Overlay glance for the card presented this turn (or the Ready linger
    /// after it). Cleared when the next utterance starts. Body is fetched
    /// separately; the session catalog is the source of truth for Home.
    #[serde(default)]
    pub artifact: Option<ArtifactPeek>,
}

/// Default soft context window for the overlay meter (token estimate).
pub const DEFAULT_CONTEXT_LIMIT_TOKENS: u32 = 500_000;

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
            activity: None,
            context_used: None,
            context_limit: None,
            artifact: None,
        }
    }
}
