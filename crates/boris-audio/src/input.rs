use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

use cpal::{
    traits::{DeviceTrait, StreamTrait},
    Sample,
};

use boris_core::{types::ArcAudioBuffer, AudioBuffer, AudioSample};

use crate::resampler::InputResampler;

pub struct InputPipeline {
    _stream: cpal::Stream,
    _flag: Arc<AtomicBool>,
    _handle: Option<thread::JoinHandle<()>>,
    pub device_id: cpal::DeviceId,
    _subscribers: Arc<Mutex<Vec<crossbeam_channel::Sender<ArcAudioBuffer>>>>,
}

impl InputPipeline {
    pub fn from_device(
        device: &cpal::Device,
        subscribers: Arc<Mutex<Vec<crossbeam_channel::Sender<ArcAudioBuffer>>>>,
    ) -> Self {
        // instance params
        let flag = Arc::new(AtomicBool::new(false));
        let (audio_tx, audio_rx) = crossbeam_channel::bounded::<AudioBuffer>(10);

        // device params
        let device_id = device.id().unwrap();
        let config = device.default_input_config().unwrap();
        let channels = config.channels();
        let sample_rate = config.sample_rate();

        let stream_config = cpal::StreamConfig {
            channels,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => Self::build_stream::<f32>(device, stream_config, audio_tx),
            cpal::SampleFormat::I16 => Self::build_stream::<i16>(device, stream_config, audio_tx),
            cpal::SampleFormat::U16 => Self::build_stream::<u16>(device, stream_config, audio_tx),
            _ => panic!("Unsupported sample format for InputStream"),
        };

        // play the stream;
        stream.play().unwrap();

        // Capture stays interleaved at device channel count; InputResampler
        // downmixes to mono then converts rate → AUDIO_TARGET_RATE.
        let mut resampler = InputResampler::new(channels as u32, sample_rate);
        let flag_clone = flag.clone();
        let subscribers_clone = subscribers.clone();

        // audio worker thread
        let handle = thread::spawn(move || {
            while let Ok(audio) = audio_rx.recv() {
                if flag_clone.load(Ordering::Relaxed) {
                    println!("droped");
                    break;
                }

                match resampler.process(&audio) {
                    Ok(resampled) => {
                        // Arc::from(Vec) → Arc<[T]> so it matches ArcAudioBuffer.
                        let arc_resampled: ArcAudioBuffer = Arc::from(resampled);
                        let subs = subscribers_clone.lock().unwrap();
                        for sub in subs.iter() {
                            let _ = sub.try_send(arc_resampled.clone());
                        }
                    }
                    Err(e) => {
                        println!("resample failed: {e}");
                    }
                }
            }
        });

        Self {
            device_id,
            _flag: flag,
            _stream: stream,
            _handle: Some(handle),
            _subscribers: subscribers,
        }
    }

    /// Capture callback: convert device samples to interleaved f32 only.
    /// Channel downmix + rate conversion happen in `InputResampler`.
    fn build_stream<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        audio_tx: crossbeam_channel::Sender<AudioBuffer>,
    ) -> cpal::Stream
    where
        T: cpal::Sample + cpal::SizedSample + 'static,
        AudioSample: cpal::FromSample<T>,
    {
        let stream = device
            .build_input_stream(
                config,
                move |data: &[T], _| {
                    // Normalize every device format (i16/u16/f32/…) into f32 [-1, 1].
                    // Keep interleaved N-channel layout; InputResampler owns downmix.
                    let samples: Vec<AudioSample> = data
                        .iter()
                        .map(|sample| AudioSample::from_sample(*sample))
                        .collect();
                    audio_tx.try_send(samples).ok();
                },
                |err| tracing::error!("InputStream error: {err}"),
                None,
            )
            .unwrap();

        stream
    }
}

impl Drop for InputPipeline {
    fn drop(&mut self) {
        // set the shutdown_flag to true;
        self._flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self._handle.take() {
            //join the thread;
            handle.join().ok();
        }
    }
}
