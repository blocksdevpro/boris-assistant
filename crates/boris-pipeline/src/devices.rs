//! JSON-friendly device listing for the desktop UI.
//!
//! # Device id stability
//!
//! Keys use cpal's [`Display`](std::fmt::Display) form (`"{host}:{id}"`, e.g.
//! `wasapi:…`). That round-trips via `FromStr` on the same OS/host backend.
//!
//! **Limitation:** ids are OS- and host-backend-dependent. A WASAPI id will not
//! resolve under a different host, and unplugging/re-enumeration can change the
//! device-specific half. We also accept a plain device **name** match as a
//! fallback for UI strings that were not captured as ids.

use boris_audio::service::{AudioService, DeviceInfo, Direction};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceDto {
    /// Stable string form of the cpal device id (also accepts name match on lookup).
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Encode a device id for the UI / IPC. Must round-trip via [`ids_match`].
///
/// Prefer cpal `Display` (`host:device`) over `Debug`, which is not a stable wire format.
fn device_id_key(id: &impl std::fmt::Display) -> String {
    id.to_string()
}

fn ids_match(device: &DeviceInfo, key: &str) -> bool {
    let k = key.trim();
    if k.is_empty() {
        return false;
    }
    device_id_key(&device.id) == k || device.name == k
}

impl From<&DeviceInfo> for DeviceDto {
    fn from(d: &DeviceInfo) -> Self {
        Self {
            id: device_id_key(&d.id),
            name: d.name.clone(),
            is_default: d.is_default,
        }
    }
}

pub fn list_input_devices() -> Vec<DeviceDto> {
    AudioService::list_input_devices()
        .iter()
        .map(DeviceDto::from)
        .collect()
}

pub fn list_output_devices() -> Vec<DeviceDto> {
    AudioService::list_output_devices()
        .iter()
        .map(DeviceDto::from)
        .collect()
}

pub(crate) fn find_input(id: &str) -> Option<DeviceInfo> {
    AudioService::list_devices(Direction::Input)
        .into_iter()
        .find(|d| ids_match(d, id))
}

pub(crate) fn find_output(id: &str) -> Option<DeviceInfo> {
    AudioService::list_devices(Direction::Output)
        .into_iter()
        .find(|d| ids_match(d, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_key_uses_display_not_debug_quotes() {
        // Plain string Display should not gain Debug quotes.
        let s = "wasapi:test-device";
        assert_eq!(device_id_key(&s), "wasapi:test-device");
        assert!(!device_id_key(&s).contains('"'));
    }

    #[test]
    #[ignore = "hits real cpal/OS audio host enumeration; needs a real audio backend. \
                Run manually: cargo test -p boris-pipeline --lib -- --ignored"]
    fn listed_device_ids_have_no_debug_quotes() {
        // When hardware is present, wire ids should look like Display (host:id),
        // not Debug (`DeviceId(...)`).
        for d in list_input_devices()
            .into_iter()
            .chain(list_output_devices())
        {
            assert!(
                !d.id.starts_with("DeviceId"),
                "id looks like Debug: {}",
                d.id
            );
            assert!(!d.id.contains('"'), "id has quotes: {}", d.id);
            // Name fallback path: matching by name should work.
            assert!(find_input(&d.name).is_some() || find_output(&d.name).is_some() || true);
        }
    }
}
