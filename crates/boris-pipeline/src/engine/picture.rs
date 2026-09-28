//! Live status publisher for the UI (`StatusPicture` snapshots).
//!
//! Channel contract: `Picture` and the per-turn agent-event closure update one
//! current snapshot under a shared lock. Numbering and sending happen under
//! that lock, so the channel receives them in sequence order. The host still
//! filters stale snapshots before forwarding them to the UI.
//! The channel is unbounded deliberately: blocking the engine on a slow UI
//! would stall voice. `send` failure (host gone) is silent by design.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use boris_core::TurnId;

use crate::status::{
    ArtifactPeek, DeviceHealth, EngineState, InputPeek, Phase, StatusPicture, WakeEnrollPeek,
};

/// Max wire chars for the activity chip (overlay renders one line).
const MAX_ACTIVITY_CHARS: usize = 160;
/// Max wire chars for the reasoning tail (overlay renders a short tail).
const MAX_THINKING_CHARS: usize = 512;

pub(super) fn truncate_thinking(s: &str) -> String {
    let count = s.chars().count();
    if count <= MAX_THINKING_CHARS {
        return s.to_string();
    }
    let tail: String = s.chars().skip(count - (MAX_THINKING_CHARS - 1)).collect();
    format!("…{tail}")
}

fn truncate_wire(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

pub(super) fn send_locked(latest: &mut StatusPicture, status_tx: &Sender<StatusPicture>) {
    latest.seq += 1;
    let _ = status_tx.send(latest.clone());
}

/// Mutable engine-side status that publishes a full [`StatusPicture`] on change.
pub(super) struct Picture {
    pub engine: EngineState,
    pub phase: Phase,
    pub detail: Option<String>,
    pub heard: Option<String>,
    pub said: Option<String>,
    pub mic: DeviceHealth,
    pub speaker: DeviceHealth,
    pub turn: Option<TurnId>,
    pub activity: Option<String>,
    pub thinking: Option<String>,
    pub context_used: Option<u32>,
    pub context_limit: Option<u32>,
    pub context_estimated: bool,
    pub artifact: Option<ArtifactPeek>,
    pub wake_enroll: Option<WakeEnrollPeek>,
    pub input: Option<InputPeek>,
    pub status_tx: Sender<StatusPicture>,
    /// The agent-event closure updates this same snapshot while thinking.
    pub latest: Arc<Mutex<StatusPicture>>,
    /// When the current [`Phase`] began — used to log how long each status lasted.
    pub phase_started: Instant,
}

impl Picture {
    pub fn publish(&self) {
        let activity = self
            .activity
            .as_ref()
            .map(|a| truncate_wire(a, MAX_ACTIVITY_CHARS));
        let thinking = self.thinking.as_deref().map(truncate_thinking);
        let mut snapshot = StatusPicture {
            seq: 0,
            engine: self.engine,
            phase: self.phase,
            detail: self.detail.clone(),
            heard: self.heard.clone(),
            said: self.said.clone(),
            mic: self.mic.clone(),
            speaker: self.speaker.clone(),
            turn: self.turn.map(|t| t.to_string()),
            activity,
            thinking,
            context_used: self.context_used,
            context_limit: self.context_limit,
            context_estimated: self.context_estimated,
            artifact: self.artifact.clone(),
            wake_enroll: self.wake_enroll.clone(),
            input: self.input.clone(),
        };
        let mut latest = self
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if snapshot.phase == Phase::Thinking
            && latest.phase == Phase::Thinking
            && snapshot.turn == latest.turn
        {
            // Context and device updates must keep the live agent chip and
            // card until Picture explicitly replaces or clears them.
            if snapshot.activity.as_deref() == Some("thinking…") {
                snapshot.activity = latest.activity.clone();
            }
            if snapshot.thinking.is_none() {
                snapshot.thinking = latest.thinking.clone();
            }
            if snapshot.artifact.is_none() {
                snapshot.artifact = latest.artifact.clone();
            }
        }
        snapshot.seq = latest.seq;
        *latest = snapshot;
        send_locked(&mut latest, &self.status_tx);
    }

    /// Mark both devices dead (audio worker / event channel disconnected).
    /// Switches self-heal via `device_switch` (`ok: true` on success); a fresh
    /// `Start` re-marks them alive at the call site.
    pub fn mark_devices_dead(&mut self) {
        self.mic.ok = false;
        self.speaker.ok = false;
        self.publish();
    }

    pub fn mark_devices_alive(&mut self) {
        self.mic.ok = true;
        self.speaker.ok = true;
    }

    pub fn set_wake_enroll(&mut self, peek: Option<WakeEnrollPeek>) {
        if self.wake_enroll != peek {
            self.wake_enroll = peek;
            self.publish();
        }
    }

    pub fn set_phase(&mut self, phase: Phase) {
        if self.phase != phase {
            let ms = self.phase_started.elapsed().as_millis() as u64;
            tracing::info!(from = ?self.phase, to = ?phase, ms, "status phase");
            self.phase = phase;
            self.phase_started = Instant::now();
            // Hearing/Reading are same-turn pipeline stages after Thinking's
            // barge capture — keep the reasoning tail across those micro-flaps
            // so the overlay doesn't flicker. Every other transition starts a
            // fresh visual turn.
            if !matches!(phase, Phase::Thinking | Phase::Hearing | Phase::Reading) {
                self.thinking = None;
            }
            if phase != Phase::AwaitingInput {
                self.input = None;
            }
        }
        self.publish();
    }

    pub fn clear_activity(&mut self) {
        let activity = self.activity.take();
        let thinking = self.thinking.take();
        let mut latest = self
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let agent_activity = latest.activity.take();
        let agent_thinking = latest.thinking.take();
        let had_agent_fields = agent_activity.is_some() || agent_thinking.is_some();
        drop(latest);
        if activity.is_some() || thinking.is_some() || had_agent_fields {
            self.publish();
        }
    }

    /// Rough token estimate (chars/4) for the overlay context meter only.
    /// Zero stays zero — a fresh turn with no provider usage yet is `0`,
    /// not `1`.
    pub fn update_context_from_chars(&mut self, approx_chars: usize) {
        let used = approx_chars as u32 / 4;
        self.context_used = Some(used);
        self.context_estimated = true;
        self.publish();
    }

    pub fn update_context(&mut self, used: u32, limit: Option<u32>, estimated: bool) {
        self.context_used = Some(used);
        if limit.is_some() {
            self.context_limit = limit;
        }
        self.context_estimated = estimated;
        self.publish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn reasoning_wire_keeps_the_latest_unicode_tail() {
        let reasoning = format!("{}last thought", "α".repeat(900));
        let tail = truncate_thinking(&reasoning);
        assert_eq!(tail.chars().count(), MAX_THINKING_CHARS);
        assert!(tail.starts_with('…'));
        assert!(tail.ends_with("last thought"));
    }

    #[test]
    fn agent_and_engine_updates_share_order_and_current_fields() {
        let (status_tx, status_rx) = mpsc::channel();
        let off = StatusPicture::off();
        let mut picture = Picture {
            engine: EngineState::On,
            phase: Phase::Thinking,
            detail: None,
            heard: None,
            said: None,
            mic: off.mic,
            speaker: off.speaker,
            turn: None,
            activity: Some("thinking…".into()),
            thinking: None,
            context_used: None,
            context_limit: None,
            context_estimated: false,
            artifact: None,
            wake_enroll: None,
            input: None,
            status_tx,
            latest: Arc::new(Mutex::new(StatusPicture::off())),
            phase_started: Instant::now(),
        };
        picture.publish();
        {
            let mut latest = picture.latest.lock().unwrap();
            latest.activity = Some("tool · search".into());
            latest.thinking = Some("Checking the source".into());
            send_locked(&mut latest, &picture.status_tx);
        }
        picture.mic.ok = false;
        picture.publish();
        let snapshots: Vec<_> = status_rx.try_iter().collect();
        assert_eq!(
            snapshots.iter().map(|s| s.seq).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(snapshots[2].phase, Phase::Thinking);
        assert!(!snapshots[2].mic.ok);
        assert_eq!(snapshots[2].activity.as_deref(), Some("tool · search"));
        assert_eq!(
            snapshots[2].thinking.as_deref(),
            Some("Checking the source")
        );

        picture.clear_activity();
        let cleared = status_rx.try_recv().unwrap();
        assert_eq!(cleared.seq, 4);
        assert_eq!(cleared.activity, None);
        assert_eq!(cleared.thinking, None);
    }
}
