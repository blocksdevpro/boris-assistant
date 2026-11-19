//! Keeps the mic and speaker working across unplug / re-plug without a restart.
//!
//! A small helper thread looks once a second at the open streams
//! ([`AudioHealth`]) and the OS device list, then sends the engine the same
//! `SwitchInput` / `SwitchOutput` commands the settings picker uses. Every
//! engine phase already applies those promptly, so recovery needs no new
//! engine states.
//!
//! Policy:
//! - Device picked by the user and present → use it (reopen if its stream died,
//!   switch back to it when it is plugged in again).
//! - Nothing picked, or the picked device is gone → follow the OS default.
//!   cpal reroutes a default stream when Windows changes the default, so this
//!   only acts when that stream died (e.g. no default left) or when a pinned
//!   device disappeared.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use boris_audio::service::{AudioService, Direction};
use boris_audio::AudioHealth;

use super::EngineCommand;
use crate::devices::ids_match;

/// Device id that means "the OS default, and keep following it".
pub(crate) const DEFAULT_DEVICE: &str = "default";
/// How often the watcher looks at devices.
const TICK: Duration = Duration::from_secs(1);
/// Minimum gap before re-sending the same switch (engine may be mid-STT).
const RETRY_AFTER: Duration = Duration::from_secs(4);
/// Failed reopen attempts of a picked device before falling back to default.
const MAX_PIN_ATTEMPTS: u32 = 3;
/// How long a picked device that keeps failing is skipped.
const PIN_COOLDOWN: Duration = Duration::from_secs(30);

/// Devices the user explicitly picked (`None` = follow the OS default).
#[derive(Debug, Default, Clone)]
pub(crate) struct DevicePins {
    pub input: Option<String>,
    pub output: Option<String>,
}

pub(crate) type SharedPins = Arc<Mutex<DevicePins>>;

/// Map a host switch request to a pin. Empty / `"default"` means follow default.
pub(crate) fn pin_from_request(device_id: &str) -> Option<String> {
    let t = device_id.trim();
    if t.is_empty() || t.eq_ignore_ascii_case(DEFAULT_DEVICE) {
        None
    } else {
        Some(t.to_string())
    }
}

/// Where a stream should be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    Device(String),
    Default,
}

impl Target {
    /// The engine command that moves `direction`'s stream here.
    fn into_command(self, direction: Direction) -> EngineCommand {
        let device_id = match self {
            Self::Device(id) => id,
            Self::Default => DEFAULT_DEVICE.into(),
        };
        match direction {
            Direction::Input => EngineCommand::SwitchInput { device_id },
            Direction::Output => EngineCommand::SwitchOutput { device_id },
        }
    }
}

/// One direction's state as seen this tick.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Observed<'a> {
    /// Picked device id when it is currently connected (`None` when nothing
    /// is picked or the picked device is missing).
    pub pinned_present: Option<&'a str>,
    /// Device id the open stream is on.
    pub current: &'a str,
    pub follows_default: bool,
    pub failed: bool,
    pub default_available: bool,
}

/// Where the stream should move, if anywhere. Pure policy.
pub(crate) fn plan(o: &Observed<'_>) -> Option<Target> {
    if let Some(pinned) = o.pinned_present {
        if o.failed || o.follows_default || o.current != pinned {
            return Some(Target::Device(pinned.to_string()));
        }
        return None;
    }
    if (o.failed || !o.follows_default) && o.default_available {
        return Some(Target::Default);
    }
    None
}

/// Retry pacing and "picked device keeps failing" fallback for one direction.
#[derive(Debug, Default)]
pub(crate) struct Lane {
    last: Option<(Target, Instant)>,
    attempts: u32,
    pin_broken_until: Option<Instant>,
}

impl Lane {
    pub(crate) fn decide(&mut self, observed: Observed<'_>, now: Instant) -> Option<Target> {
        let mut o = observed;
        if let Some(until) = self.pin_broken_until {
            if now < until {
                o.pinned_present = None;
            } else {
                self.pin_broken_until = None;
            }
        }
        let Some(target) = plan(&o) else {
            self.last = None;
            self.attempts = 0;
            return None;
        };
        if let Some((last, at)) = &self.last {
            if *last == target && now.duration_since(*at) < RETRY_AFTER {
                return None;
            }
        }
        let repeat = self.last.as_ref().is_some_and(|(last, _)| *last == target);
        self.attempts = if repeat { self.attempts + 1 } else { 1 };
        if matches!(target, Target::Device(_)) && self.attempts > MAX_PIN_ATTEMPTS {
            tracing::warn!(
                ?target,
                "picked device keeps failing — using the default for now"
            );
            self.pin_broken_until = Some(now + PIN_COOLDOWN);
            self.last = None;
            self.attempts = 0;
            return self.decide(observed, now);
        }
        self.last = Some((target.clone(), now));
        Some(target)
    }
}

/// Helper thread handle. Dropping it stops and joins the thread.
pub(crate) struct DeviceWatch {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl DeviceWatch {
    /// Spawn the watcher. The engine thread owns the returned guard, so the
    /// watcher stops (and is joined) whenever the engine loop exits, including
    /// by panic. It also exits on its own once the engine stops taking
    /// commands.
    pub(crate) fn spawn(
        health: AudioHealth,
        pins: SharedPins,
        cmd_tx: Sender<EngineCommand>,
    ) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let join = thread::Builder::new()
            .name("boris-device-watch".into())
            .spawn(move || run(health, pins, cmd_tx, stop_thread))
            .map_err(|e| tracing::warn!(error = %e, "device watcher not started"))
            .ok()?;
        Some(Self {
            stop,
            join: Some(join),
        })
    }
}

impl Drop for DeviceWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            join.thread().unpark();
            let _ = join.join();
        }
    }
}

fn run(
    health: AudioHealth,
    pins: SharedPins,
    cmd_tx: Sender<EngineCommand>,
    stop: Arc<AtomicBool>,
) {
    let mut input = Lane::default();
    let mut output = Lane::default();
    loop {
        thread::park_timeout(TICK);
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let picked = pins.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let now = Instant::now();
        let lanes = [
            (Direction::Input, picked.input.as_deref(), &mut input),
            (Direction::Output, picked.output.as_deref(), &mut output),
        ];
        for (direction, pick, lane) in lanes {
            let Some(target) = tick(direction, &health, pick, lane, now) else {
                continue;
            };
            tracing::info!(?target, ?direction, "device watch: switching");
            if cmd_tx.send(target.into_command(direction)).is_err() {
                return; // engine thread gone
            }
        }
    }
}

fn tick(
    direction: Direction,
    health: &AudioHealth,
    picked: Option<&str>,
    lane: &mut Lane,
    now: Instant,
) -> Option<Target> {
    let devices = AudioService::list_devices(direction);
    let default_available = devices.iter().any(|d| d.is_default);
    // Picks may be stored as an id or (older configs) a device name.
    let pinned_present = picked.and_then(|p| {
        devices
            .iter()
            .find(|d| ids_match(d, p))
            .map(|d| d.id.to_string())
    });
    let stream = health.stream(direction);
    lane.decide(
        Observed {
            pinned_present: pinned_present.as_deref(),
            current: &stream.device_id,
            follows_default: stream.follows_default,
            failed: stream.failed,
            default_available,
        },
        now,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs<'a>(
        pinned_present: Option<&'a str>,
        current: &'a str,
        follows_default: bool,
        failed: bool,
    ) -> Observed<'a> {
        Observed {
            pinned_present,
            current,
            follows_default,
            failed,
            default_available: true,
        }
    }

    #[test]
    fn healthy_streams_are_left_alone() {
        assert_eq!(plan(&obs(None, "laptop", true, false)), None);
        assert_eq!(plan(&obs(Some("usb"), "usb", false, false)), None);
    }

    #[test]
    fn dead_default_stream_is_reopened_on_the_default() {
        assert_eq!(
            plan(&obs(None, "laptop", true, true)),
            Some(Target::Default)
        );
    }

    #[test]
    fn unplugged_pick_falls_back_to_the_default() {
        // Picked USB mic vanished: not present, its stream failed.
        assert_eq!(plan(&obs(None, "usb", false, true)), Some(Target::Default));
    }

    #[test]
    fn replugged_pick_is_taken_back() {
        assert_eq!(
            plan(&obs(Some("usb"), "laptop", true, false)),
            Some(Target::Device("usb".into()))
        );
    }

    #[test]
    fn dead_pick_still_present_is_reopened() {
        assert_eq!(
            plan(&obs(Some("usb"), "usb", false, true)),
            Some(Target::Device("usb".into()))
        );
    }

    #[test]
    fn nothing_to_fall_back_to_means_no_action() {
        let mut o = obs(None, "usb", false, true);
        o.default_available = false;
        assert_eq!(plan(&o), None);
    }

    #[test]
    fn targets_become_the_matching_switch_command() {
        assert!(matches!(
            Target::Default.into_command(Direction::Input),
            EngineCommand::SwitchInput { device_id } if device_id == DEFAULT_DEVICE
        ));
        assert!(matches!(
            Target::Device("usb".into()).into_command(Direction::Output),
            EngineCommand::SwitchOutput { device_id } if device_id == "usb"
        ));
    }

    #[test]
    fn requests_map_default_and_blank_to_follow_default() {
        assert_eq!(pin_from_request("default"), None);
        assert_eq!(pin_from_request("  "), None);
        assert_eq!(pin_from_request("wasapi:abc"), Some("wasapi:abc".into()));
    }

    #[test]
    fn lane_paces_retries_of_the_same_switch() {
        let mut lane = Lane::default();
        let t0 = Instant::now();
        let dead = obs(None, "laptop", true, true);
        assert_eq!(lane.decide(dead, t0), Some(Target::Default));
        assert_eq!(lane.decide(dead, t0 + Duration::from_secs(1)), None);
        assert_eq!(
            lane.decide(dead, t0 + RETRY_AFTER + Duration::from_millis(1)),
            Some(Target::Default)
        );
    }

    #[test]
    fn lane_falls_back_when_a_pick_keeps_failing() {
        let mut lane = Lane::default();
        let mut t = Instant::now();
        let broken_pick = obs(Some("usb"), "usb", false, true);
        for _ in 0..MAX_PIN_ATTEMPTS {
            assert_eq!(
                lane.decide(broken_pick, t),
                Some(Target::Device("usb".into()))
            );
            t += RETRY_AFTER + Duration::from_millis(1);
        }
        assert_eq!(lane.decide(broken_pick, t), Some(Target::Default));
        // Cooldown over: try the pick again.
        t += PIN_COOLDOWN + Duration::from_millis(1);
        assert_eq!(
            lane.decide(broken_pick, t),
            Some(Target::Device("usb".into()))
        );
    }

    #[test]
    fn lane_resets_once_healthy() {
        let mut lane = Lane::default();
        let t = Instant::now();
        assert!(lane.decide(obs(None, "a", true, true), t).is_some());
        assert_eq!(lane.decide(obs(None, "a", true, false), t), None);
        // A new failure right after recovery is acted on immediately.
        assert_eq!(
            lane.decide(obs(None, "a", true, true), t),
            Some(Target::Default)
        );
    }
}
