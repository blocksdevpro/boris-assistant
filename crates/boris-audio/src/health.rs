//! Shared view of the live capture / playback streams for device watchers.
//!
//! cpal reports a lost device (USB unplugged, Bluetooth gone, driver reset)
//! only through the stream's error callback, after which the stream stops
//! delivering callbacks. [`AudioHealth`] records that per stream so a watcher
//! thread can reopen or fall back without owning the [`crate::AudioService`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::devices::Direction;

/// Set by a stream's error callback when the stream is lost.
pub(crate) type FailFlag = Arc<AtomicBool>;

/// Fresh flag for a stream about to open. It becomes current only through
/// [`AudioHealth::set`] once the open succeeds, so a failed open never
/// replaces a working stream's state.
pub(crate) fn new_fail_flag() -> FailFlag {
    Arc::new(AtomicBool::new(false))
}

/// Point-in-time view of one open stream, read under a single lock.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamHealth {
    /// Device id of the open stream (`DeviceId` display form); empty before
    /// the first open.
    pub device_id: String,
    /// Opened on the OS default endpoint (cpal reroutes it when the Windows
    /// default changes) rather than on a specific device.
    pub follows_default: bool,
    /// The stream's error callback reported loss; it has likely stopped.
    pub failed: bool,
}

impl StreamHealth {
    /// Stream is running and following the OS default.
    pub fn is_live_default(&self) -> bool {
        self.follows_default && !self.failed
    }
}

/// One open stream: which device, and the flag its error callback sets.
#[derive(Debug, Default)]
struct StreamSlot {
    device_id: String,
    failed: FailFlag,
    follows_default: bool,
}

#[derive(Debug, Default)]
struct Inner {
    input: Mutex<StreamSlot>,
    output: Mutex<StreamSlot>,
}

/// Cheap-to-clone health handle shared by the audio service and watchers.
#[derive(Debug, Clone, Default)]
pub struct AudioHealth(Arc<Inner>);

impl AudioHealth {
    /// Snapshot of the capture stream.
    pub fn input(&self) -> StreamHealth {
        self.stream(Direction::Input)
    }

    /// Snapshot of the playback stream.
    pub fn output(&self) -> StreamHealth {
        self.stream(Direction::Output)
    }

    /// Snapshot of the stream for `direction`. All fields come from one lock,
    /// so a concurrent device switch is never observed half-applied.
    pub fn stream(&self, direction: Direction) -> StreamHealth {
        let slot = self.slot(direction);
        StreamHealth {
            device_id: slot.device_id.clone(),
            follows_default: slot.follows_default,
            failed: slot.failed.load(Ordering::Relaxed),
        }
    }

    /// Record a freshly opened stream (replacing the previous one's state).
    pub(crate) fn set(
        &self,
        direction: Direction,
        device_id: &cpal::DeviceId,
        failed: FailFlag,
        follows_default: bool,
    ) {
        *self.slot(direction) = StreamSlot {
            device_id: device_id.to_string(),
            failed,
            follows_default,
        };
    }

    fn slot(&self, direction: Direction) -> MutexGuard<'_, StreamSlot> {
        let slot = match direction {
            Direction::Input => &self.0.input,
            Direction::Output => &self.0.output,
        };
        // Slots hold plain data; a panic mid-assignment cannot break them.
        slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Whether a stream error means the stream stopped and must be reopened.
///
/// `DeviceChanged` is cpal rerouting a default-device stream (it keeps
/// running); xruns and refused real-time priority are glitches, not loss.
pub(crate) fn stream_error_is_fatal(kind: cpal::ErrorKind) -> bool {
    !matches!(
        kind,
        cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied
    )
}

/// Error callback body shared by capture and playback streams. Not on the RT
/// sample path; cpal calls it from its stream thread.
pub(crate) fn on_stream_error(stream: &'static str, failed: &AtomicBool, err: &cpal::Error) {
    if !stream_error_is_fatal(err.kind()) {
        tracing::info!(stream, error = %err, "audio stream notice");
        return;
    }
    if !failed.swap(true, Ordering::Relaxed) {
        tracing::warn!(stream, error = %err, "audio stream failed — device lost or reset");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_stream_cannot_mark_the_new_one_failed() {
        let health = AudioHealth::default();
        let old = new_fail_flag();
        let id: cpal::DeviceId = "wasapi:old".parse().unwrap();
        health.set(Direction::Input, &id, old.clone(), false);
        let id2: cpal::DeviceId = "wasapi:new".parse().unwrap();
        health.set(Direction::Input, &id2, new_fail_flag(), true);
        old.store(true, Ordering::Relaxed); // late error from the old device
        let input = health.input();
        assert!(!input.failed);
        assert!(input.device_id.contains("new"));
        assert!(input.is_live_default());
        // Directions are independent.
        assert_eq!(health.output(), StreamHealth::default());
    }

    #[test]
    fn a_failed_stream_is_not_live() {
        let health = AudioHealth::default();
        let flag = new_fail_flag();
        let id: cpal::DeviceId = "wasapi:spk".parse().unwrap();
        health.set(Direction::Output, &id, flag.clone(), true);
        assert!(health.output().is_live_default());
        flag.store(true, Ordering::Relaxed);
        assert!(health.output().failed);
        assert!(!health.output().is_live_default());
    }

    #[test]
    fn reroutes_and_glitches_are_not_device_loss() {
        assert!(!stream_error_is_fatal(cpal::ErrorKind::DeviceChanged));
        assert!(!stream_error_is_fatal(cpal::ErrorKind::Xrun));
        assert!(stream_error_is_fatal(cpal::ErrorKind::DeviceNotAvailable));
        assert!(stream_error_is_fatal(cpal::ErrorKind::StreamInvalidated));
    }
}
