use std::sync::mpsc::Sender;
use std::thread;
use std::thread::JoinHandle;

use boris_core::AudioSample;
use boris_core::{AudioBuffer, error::Result};

use crate::AUDIO_CHUNK_SIZE;
use crate::{AUDIO_TARGET_RATE, capture::Capture, resampler::AudioResampler};

pub struct Pipeline {
    _handle: JoinHandle<()>,
    _capture: Capture,
}

impl Pipeline {
    pub fn spawn(audio_tx: Sender<AudioBuffer>) -> Result<Self> {
        let (raw_audio_tx, raw_audio_rx) = crossbeam_channel::bounded::<AudioBuffer>(100);

        let capture = Capture::new(raw_audio_tx)?;
        let mut resampler = AudioResampler::new(1, capture.sample_rate, AUDIO_TARGET_RATE);

        let handle = thread::spawn(move || {
            let mut accumulator: Vec<AudioSample> = Vec::with_capacity(AUDIO_CHUNK_SIZE as usize);
            loop {
                let sample = raw_audio_rx
                    .recv()
                    .expect("[ERROR] failed to receive raw audio sample");
                accumulator.extend_from_slice(&sample);
                while accumulator.len() >= AUDIO_CHUNK_SIZE as usize {
                    let resample_chunk = accumulator[..AUDIO_CHUNK_SIZE as usize].to_vec();
                    accumulator.drain(..AUDIO_CHUNK_SIZE as usize);

                    let resampled = resampler
                        .resample(&resample_chunk)
                        .expect("[ERROR] failed to resample raw audio chunks.");
                    audio_tx
                        .send(resampled)
                        .expect("[ERROR] failed to send resampled audio sample");
                }
            }
        });

        Ok(Self {
            _handle: handle,
            _capture: capture,
        })
    }
}
