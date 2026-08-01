//! JSON-friendly device listing for the desktop UI.

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
fn device_id_key(id: &impl std::fmt::Debug) -> String {
    format!("{id:?}")
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
