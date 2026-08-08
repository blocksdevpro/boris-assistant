use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
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
        // Capture callbacks fire every ~10ms. A tiny queue (e.g. 10 ≈ 100ms)
        // silently drops frames under even light worker stalls, which shows up
        // as missing words/sentences in STT. Keep ~2–3s of headroom.
        let (audio_tx, audio_rx) = crossbeam_channel::bounded::<AudioBuffer>(256);

        // device params
        let device_id = device.id().expect("input device id");
        let config = device
            .default_input_config()
            .expect("default_input_config — mic may be in use or denied");
        let channels = config.channels();
        let sample_rate = config.sample_rate();
        let sample_format = config.sample_format();
        tracing::info!(
            ?device_id,
            channels,
            ?sample_rate,
            ?sample_format,
            "InputPipeline::from_device"
        );

        let stream_config = cpal::StreamConfig {
            channels,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        // Shared with the RT callback (count only) and the worker (reports).
        let capture_drops = Arc::new(AtomicU64::new(0));
        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                Self::build_stream::<f32>(device, stream_config, audio_tx, capture_drops.clone())
            }
            cpal::SampleFormat::I16 => {
                Self::build_stream::<i16>(device, stream_config, audio_tx, capture_drops.clone())
            }
            cpal::SampleFormat::U16 => {
                Self::build_stream::<u16>(device, stream_config, audio_tx, capture_drops.clone())
            }
            other => {
                tracing::error!(?other, "unsupported input sample format");
                panic!("Unsupported sample format for InputStream: {other:?}");
            }
        };

        // play the stream;
        if let Err(e) = stream.play() {
            tracing::error!(error = %e, "input stream.play() failed");
            panic!("input stream.play() failed: {e}");
        }
        tracing::info!("input stream playing");

        // Capture stays interleaved at device channel count; InputResampler
        // downmixes to mono then converts rate → AUDIO_TARGET_RATE.
        let mut resampler = InputResampler::new(channels as u32, sample_rate);
        let flag_clone = flag.clone();
        let subscribers_clone = subscribers.clone();
        let capture_drops_worker = capture_drops;

        // audio worker thread
        let handle = thread::spawn(move || {
            let mut last_reported_capture_drops = 0u64;
            let mut subscriber_drops: u64 = 0;
            while let Ok(audio) = audio_rx.recv() {
                if flag_clone.load(Ordering::Relaxed) {
                    break;
                }

                // Report capture-side drops from the worker (not the RT callback).
                let drops = capture_drops_worker.load(Ordering::Relaxed);
                if drops > last_reported_capture_drops {
                    tracing::warn!(
                        new_drops = drops - last_reported_capture_drops,
                        total_drops = drops,
                        "InputPipeline: capture queue full — mic frames dropped"
                    );
                    last_reported_capture_drops = drops;
                }

                match resampler.process(&audio) {
                    Ok(resampled) if resampled.is_empty() => {
                        // Stream resampler only emits when a full FFT block is ready.
                    }
                    Ok(resampled) => {
                        // Arc::from(Vec) → Arc<[T]> so it matches ArcAudioBuffer.
                        let arc_resampled: ArcAudioBuffer = Arc::from(resampled);
                        let subs = subscribers_clone.lock().unwrap();
                        for sub in subs.iter() {
                            if sub.try_send(arc_resampled.clone()).is_err() {
                                subscriber_drops = subscriber_drops.saturating_add(1);
                                // Log occasionally — full spam during agent think is normal.
                                if subscriber_drops == 1 || subscriber_drops.is_multiple_of(50) {
                                    tracing::warn!(
                                        subscriber_drops,
                                        "InputPipeline: subscriber queue full — dropping resampled frame"
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "InputPipeline: resample failed");
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
        capture_drops: Arc<AtomicU64>,
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
                    // Never block the real-time callback. Count drops; worker logs them.
                    if audio_tx.try_send(samples).is_err() {
                        capture_drops.fetch_add(1, Ordering::Relaxed);
                    }
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
