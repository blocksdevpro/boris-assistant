//! Host-command polling and speaker playback waits.
//!
//! Playback waits cooperate with device switches: rebuilding the output pipeline
//! aborts the wait so the UI never sticks in Talking on a dead event stream.

use std::sync::mpsc::{self, Receiver};

use boris_audio::output::OutputEvent;
use boris_audio::service::AudioService;

use super::device_switch::{apply_input_switch, apply_output_switch};
use super::picture::Picture;
use super::EngineCommand;

/// Result of draining host commands once.
#[derive(Debug, Clone, Copy)]
pub(super) struct PollOutcome {
    /// Engine still on.
    pub running: bool,
    /// Output pipeline was rebuilt — any in-flight Play is gone and its
    /// Started/Drained events will never arrive on the new channel.
    pub output_rebuilt: bool,
}

impl PollOutcome {
    pub fn still_running(self) -> bool {
        self.running
    }
}

/// How a playback wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PlaybackWait {
    /// Natural finish (Started then later Drained), or Started observed.
    Finished,
    /// Speaker switched / Flush — do not keep waiting for dead events.
    Aborted,
    /// Host stop / disconnect — go Off.
    Stopped,
}

/// Drain host commands; apply device switches immediately.
pub(super) fn poll_running(
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
    audio: &mut AudioService,
    output_events: &mut crossbeam_channel::Receiver<OutputEvent>,
    picture: &mut Picture,
) -> PollOutcome {
    let mut output_rebuilt = false;
    loop {
        match cmd_rx.try_recv() {
            Ok(EngineCommand::Stop) | Ok(EngineCommand::Shutdown) => {
                *running = false;
                return PollOutcome {
                    running: false,
                    output_rebuilt,
                };
            }
            Ok(EngineCommand::Start) => *running = true,
            Ok(EngineCommand::SwitchInput { device_id }) => {
                apply_input_switch(audio, picture, &device_id);
            }
            Ok(EngineCommand::SwitchOutput { device_id }) => {
                if apply_output_switch(audio, output_events, picture, &device_id) {
                    output_rebuilt = true;
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                return PollOutcome {
                    running: *running,
                    output_rebuilt,
                };
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                *running = false;
                return PollOutcome {
                    running: false,
                    output_rebuilt,
                };
            }
        }
    }
}

/// Wait until the output worker has resampled + queued samples (about to be audible).
pub(super) fn wait_playback_started(
    output_events: &mut crossbeam_channel::Receiver<OutputEvent>,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
    audio: &mut AudioService,
    picture: &mut Picture,
) -> PlaybackWait {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let poll = poll_running(cmd_rx, running, audio, output_events, picture);
        if !poll.running {
            audio.stop();
            return PlaybackWait::Stopped;
        }
        if poll.output_rebuilt {
            tracing::info!("speaker switched before playback Started — aborting play wait");
            return PlaybackWait::Aborted;
        }
        if std::time::Instant::now() > deadline {
            tracing::warn!("playback Started timeout — flipping UI anyway");
            return if *running {
                PlaybackWait::Finished
            } else {
                PlaybackWait::Stopped
            };
        }
        match output_events.recv_timeout(std::time::Duration::from_millis(20)) {
            Ok(OutputEvent::Started) => return PlaybackWait::Finished,
            // Short clips may drain before we observe Started if we missed it — treat as ok.
            Ok(OutputEvent::Drained) => return PlaybackWait::Finished,
            Ok(OutputEvent::Cleared) => return PlaybackWait::Aborted,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return PlaybackWait::Stopped;
            }
        }
    }
}

pub(super) fn wait_playback_or_stop(
    output_events: &mut crossbeam_channel::Receiver<OutputEvent>,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
    audio: &mut AudioService,
    picture: &mut Picture,
) {
    loop {
        let poll = poll_running(cmd_rx, running, audio, output_events, picture);
        if !poll.running {
            audio.stop();
            return;
        }
        if poll.output_rebuilt {
            // Old pipeline (and its Drained event) is gone with the device rebuild.
            tracing::info!("speaker switched mid-playback — ending Talking wait");
            return;
        }
        match output_events.recv_timeout(std::time::Duration::from_millis(40)) {
            Ok(OutputEvent::Started) => continue, // already speaking
            Ok(OutputEvent::Drained) => return,
            Ok(OutputEvent::Cleared) => return,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
        }
    }
}
