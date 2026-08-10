//! Mic / speaker device switches applied on the engine thread.

use boris_audio::output::OutputEvent;
use boris_audio::service::AudioService;

use crate::devices::{find_input, find_output};
use crate::status::DeviceHealth;

use super::picture::Picture;

pub(super) fn apply_input_switch(audio: &mut AudioService, picture: &mut Picture, device_id: &str) {
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
            tracing::warn!(%device_id, "unknown input device");
            picture.detail = Some("unknown microphone id".into());
            picture.publish();
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
            tracing::warn!(%device_id, "unknown output device");
            picture.detail = Some("unknown speaker id".into());
            picture.publish();
            false
        }
    }
}
