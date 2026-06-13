use std::sync::mpsc::Receiver;

use boris_core::{
    AudioSampleBuffer,
    error::{BorisError, BorisResult},
};
use cpal::{
    Stream,
    traits::{DeviceTrait, HostTrait},
};

pub struct AudioPlayback {
    _stream: Option<Stream>,
}

impl AudioPlayback {
    pub fn new(audio_rx: Receiver<AudioSampleBuffer>) -> BorisResult<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| BorisError::AudioError("no output device found!".to_string()))?;
        let config = device
            .default_output_config()
            .map_err(|err| BorisError::AudioError(err.to_string()))?;

        let channels = config.channels() as usize;
        let sample_rate = config.sample_rate();

        Ok(Self { _stream: None })
    }
}
