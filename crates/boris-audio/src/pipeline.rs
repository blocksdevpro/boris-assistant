use std::sync::mpsc::Sender;
use std::thread;
use std::{sync::mpsc, thread::JoinHandle};

use boris_core::{AudioSampleBuffer, error::BorisResult};

use crate::{AUDIO_TARGET_RATE, capture::AudioCapture, resampler::AudioResampler};

pub struct AudioPipeline {
    _handle: JoinHandle<()>,
    _capture: AudioCapture,
}

impl AudioPipeline {
    pub fn spawn(audio_tx: Sender<AudioSampleBuffer>) -> BorisResult<Self> {
        let (raw_audio_tx, raw_audio_rx) = mpsc::channel::<AudioSampleBuffer>();

        let capture = AudioCapture::new(raw_audio_tx)?;
        let mut resampler = AudioResampler::new(1, capture.sample_rate, AUDIO_TARGET_RATE);

        let handle = thread::spawn(move || {
            loop {
                let sample = raw_audio_rx
                    .recv()
                    .expect("[ERROR] failed to receive raw audio sample");
                let resampled = resampler
                    .resample(&sample)
                    .expect("[ERROR] failed to resample raw audio chunks.");
                audio_tx
                    .send(resampled)
                    .expect("[ERROR] failed to send resampled audio sample");
            }
        });

        Ok(Self {
            _handle: handle,
            _capture: capture,
        })
    }
}
