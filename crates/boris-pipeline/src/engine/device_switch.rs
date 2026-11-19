//! Mic / speaker device switches applied on the engine thread.

use boris_audio::output::OutputEvent;
use boris_audio::service::AudioService;

use crate::devices::{find_input, find_output};
use crate::status::DeviceHealth;

use super::device_watch::pin_from_request;
use super::picture::Picture;

/// Name of the current OS default for status labels.
pub(super) fn default_name(devices: Vec<boris_audio::service::DeviceInfo>) -> String {
    devices
        .into_iter()
        .find(|d| d.is_default)
        .map(|d| d.name)
        .unwrap_or_else(|| "System default".into())
}

pub(super) fn apply_input_switch(audio: &mut AudioService, picture: &mut Picture, device_id: &str) {
    if pin_from_request(device_id).is_none() {
        match audio.use_default_input() {
            Ok(()) => {
                let label = default_name(AudioService::list_input_devices());
                tracing::info!(name = %label, "input follows the system default");
                picture.mic = DeviceHealth { label, ok: true };
                picture.detail = None;
                picture.publish();
            }
            Err(e) => {
                tracing::warn!(error = %e, "default input unavailable");
                picture.mic.ok = false;
                picture.publish();
            }
        }
        return;
    }
    match find_input(device_id) {
        Some(info) => match audio.switch_input(&info.id) {
            Ok(()) => {
                tracing::info!(name = %info.name, "switched input device");
                picture.mic = DeviceHealth {
                    label: info.name,
                    ok: true,
                };
                picture.detail = None;
                picture.publish();
            }
            Err(e) => {
                tracing::error!(error = %e, %device_id, "input switch failed");
                picture.detail = Some(format!("mic switch failed: {e}"));
                picture.mic.ok = false;
                picture.publish();
            }
        },
        None => {
            // Picked mic is not plugged in. Keep the current stream; the
            // device watcher switches over when it appears.
            tracing::info!(%device_id, "picked input not connected — keeping current mic");
        }
    }
}

/// Switch speaker. Returns `true` when the output pipeline was rebuilt
/// (in-flight playback and its event stream are gone).
pub(super) fn apply_output_switch(
    audio: &mut AudioService,
    output_events: &mut crossbeam_channel::Receiver<OutputEvent>,
    picture: &mut Picture,
    device_id: &str,
) -> bool {
    if pin_from_request(device_id).is_none() {
        return match audio.use_default_output() {
            Ok(rebuilt) => {
                if rebuilt {
                    *output_events = audio.subscribe_output();
                }
                let label = default_name(AudioService::list_output_devices());
                tracing::info!(name = %label, rebuilt, "output follows the system default");
                picture.speaker = DeviceHealth { label, ok: true };
                picture.detail = None;
                picture.publish();
                rebuilt
            }
            Err(e) => {
                tracing::warn!(error = %e, "default output unavailable");
                picture.speaker.ok = false;
                picture.publish();
                false
            }
        };
    }
    match find_output(device_id) {
        Some(info) => match audio.switch_output(&info.id) {
            Ok(rebuilt) => {
                if rebuilt {
                    tracing::info!(name = %info.name, "switched output device");
                    // Pipeline rebuild drops in-flight Play + its event stream.
                    // Waiters must treat this as playback abort (`output_rebuilt`).
                    *output_events = audio.subscribe_output();
                } else {
                    tracing::debug!(name = %info.name, "output already selected");
                }
                picture.speaker = DeviceHealth {
                    label: info.name,
                    ok: true,
                };
                picture.detail = None;
                picture.publish();
                rebuilt
            }
            Err(e) => {
                tracing::error!(error = %e, %device_id, "output switch failed");
                picture.detail = Some(format!("speaker switch failed: {e}"));
                picture.speaker.ok = false;
                picture.publish();
                false
            }
        },
        None => {
            // Picked speaker is not plugged in. Keep the current stream; the
            // device watcher switches over when it appears.
            tracing::info!(%device_id, "picked output not connected — keeping current speaker");
            false
        }
    }
}
