//! Device enumeration helpers (cpal).

use cpal::{
    traits::{DeviceTrait, HostTrait},
    Device, DeviceId,
};

/// Capture vs playback enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Microphones / line-in.
    Input,
    /// Speakers / headphones.
    Output,
}

/// One audio endpoint for UI lists.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Host-stable device id (use for switch APIs).
    pub id: DeviceId,
    /// Human-readable name from the OS.
    pub name: String,
    /// Whether this is the current host default for the direction.
    pub is_default: bool,
}

/// List devices for `direction` (name + default flag).
pub fn list_devices(direction: Direction) -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let default_id = match direction {
        Direction::Input => host.default_input_device(),
        Direction::Output => host.default_output_device(),
    }
    .and_then(|device| device.id().ok());

    let devices = match direction {
        Direction::Input => host.input_devices(),
        Direction::Output => host.output_devices(),
    };

    devices
        .into_iter()
        .flatten()
        .filter_map(|device| {
            let id = device.id().ok()?;
            let description = device.description().ok()?;
            Some(DeviceInfo {
                is_default: default_id.as_ref() == Some(&id),
                name: description.name().to_string(),
                id,
            })
        })
        .collect()
}

/// List input devices.
pub fn list_input_devices() -> Vec<DeviceInfo> {
    list_devices(Direction::Input)
}

/// List output devices.
pub fn list_output_devices() -> Vec<DeviceInfo> {
    list_devices(Direction::Output)
}

/// Resolve an input device by id (must support input).
pub fn find_input_device(id: &DeviceId) -> Option<Device> {
    cpal::default_host()
        .device_by_id(id)
        .filter(|device| device.supports_input())
}

/// Resolve an output device by id (must support output).
pub fn find_output_device(id: &DeviceId) -> Option<Device> {
    cpal::default_host()
        .device_by_id(id)
        .filter(|device| device.supports_output())
}

/// Input device by id, or host default if missing.
pub fn find_input_device_or_default(id: &DeviceId) -> Option<Device> {
    find_input_device(id).or_else(|| cpal::default_host().default_input_device())
}

/// Output device by id, or host default if missing.
pub fn find_output_device_or_default(id: &DeviceId) -> Option<Device> {
    find_output_device(id).or_else(|| cpal::default_host().default_output_device())
}

/// Best-effort human name for logging.
pub fn device_name(device: &Device) -> String {
    device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "<unknown>".into())
}
