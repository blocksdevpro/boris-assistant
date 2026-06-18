use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::thread::JoinHandle;

use boris_audio::buffer::RecordingBuffer;
use boris_core::event::Event;
use boris_core::types::{ArcAudioBuffer, Lifecycle};
use boris_core::{AudioBuffer, AudioSample, error::Result};

use boris_audio::{AUDIO_CHUNK_SIZE, AUDIO_TARGET_RATE, capture::Capture, resampler::Resampler};

pub struct AudioPipelineWorker {
    _handle: JoinHandle<()>,
    _capture: Capture,
}

impl AudioPipelineWorker {
    pub fn spawn(audio_tx: Sender<AudioBuffer>) -> Result<Self> {
        let (raw_audio_tx, raw_audio_rx) = crossbeam_channel::bounded::<AudioBuffer>(100);

        let capture = Capture::new(raw_audio_tx)?;
        let mut resampler = Resampler::new(1, capture.sample_rate, AUDIO_TARGET_RATE);

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

pub struct AudioDispatcherWorker {
    _handle: JoinHandle<()>,
}

impl AudioDispatcherWorker {
    pub fn spawn(audio_rx: Receiver<AudioBuffer>, audio_txs: Vec<Sender<ArcAudioBuffer>>) -> Self {
        let handle = std::thread::spawn(move || {
            while let Ok(audio) = audio_rx.recv() {
                let shared_audio: ArcAudioBuffer = Arc::from(audio);
                for tx in &audio_txs {
                    tx.send(shared_audio.clone()).ok();
                }
            }
        });
        Self { _handle: handle }
    }
}

pub struct AudioRecordingWorker {
    _handle: JoinHandle<()>,
}

impl AudioRecordingWorker {
    pub fn spawn(
        audio_rx: Receiver<ArcAudioBuffer>,
        control_rx: Receiver<Lifecycle>,
        event_tx: Sender<Event>,
    ) -> Self {
        let handle = thread::spawn(move || {
            let mut buffer = RecordingBuffer::new(AUDIO_TARGET_RATE as usize * 2);

            loop {
                while let Ok(command) = control_rx.try_recv() {
                    match command {
                        Lifecycle::Start => {
                            buffer.set_recording(true);
                        }
                        Lifecycle::Stop => {
                            buffer.set_recording(false);
                            let audio = buffer.take_audio();
                            event_tx.send(Event::RecordingFinished(audio)).ok();
                        }
                    };
                }
                if let Ok(audio) = audio_rx.recv() {
                    buffer.push(&audio);
                } else {
                    break;
                }
            }
        });

        Self { _handle: handle }
    }
}
