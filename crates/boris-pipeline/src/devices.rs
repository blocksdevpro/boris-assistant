//! JSON-friendly device listing for the desktop UI.

use boris_audio::service::{AudioService, DeviceInfo, Direction};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceDto {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

impl From<&DeviceInfo> for DeviceDto {
    fn from(d: &DeviceInfo) -> Self {
        Self {
            id: format!("{:?}", d.id),
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
        .find(|d| format!("{:?}", d.id) == id || d.name == id)
}

pub(crate) fn find_output(id: &str) -> Option<DeviceInfo> {
    AudioService::list_devices(Direction::Output)
        .into_iter()
        .find(|d| format!("{:?}", d.id) == id || d.name == id)
}
