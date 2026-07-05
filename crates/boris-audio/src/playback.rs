use std::collections::VecDeque;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use boris_core::{
    AudioBuffer,
    error::{Error, Result},
};
use cpal::{
    Stream, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

pub struct Playback {
    _stream: Stream,
}

impl Playback {
    /// Spawn an output stream that drains f32 PCM samples from `audio_rx`
    /// and plays them through the default output device.
    ///
    /// `sample_rate` — must match whatever rate the TTS model outputs
    /// (Kokoro = 24_000 Hz).
    pub fn new(audio_rx: Receiver<AudioBuffer>, sample_rate: u32) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| Error::AudioError("no output device found".to_string()))?;
        let dconfig = device
            .default_output_config()
            .map_err(|e| Error::AudioError(format!("failed to get default output config: {e}")))?;

        let target_channels = dconfig.channels();
        let target_sample_rate = dconfig.sample_rate();
        let stream_config = dconfig.config();

        // Shared ring-buffer of pending f32 samples (AudioBuffer = Vec<f32>).
        let pending: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
        let pending_fill = pending.clone();

        // A background thread refills the pending buffer from the channel,
        // resampling and converting channels as needed.
        std::thread::spawn(move || {
            while let Ok(samples) = audio_rx.recv() {
                let resampled = resample_linear(&samples, sample_rate, target_sample_rate);
                let channel_converted = convert_channels(&resampled, target_channels);
                pending_fill.lock().unwrap().extend(channel_converted);
            }
        });

        let stream = match dconfig.sample_format() {
            cpal::SampleFormat::F32 => {
                build_output_stream::<f32>(&device, stream_config, pending)?
            }
            cpal::SampleFormat::I16 => {
                build_output_stream::<i16>(&device, stream_config, pending)?
            }
            cpal::SampleFormat::U16 => {
                build_output_stream::<u16>(&device, stream_config, pending)?
            }
            _ => {
                return Err(Error::AudioError("unsupported sample format".to_string()));
            }
        };

        stream
            .play()
            .map_err(|e| Error::AudioError(format!("failed to start playback: {e}")))?;

        Ok(Self { _stream: stream })
    }
}

fn build_output_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    pending: Arc<Mutex<VecDeque<f32>>>,
) -> Result<Stream>
where
    T: cpal::Sample + cpal::SizedSample + cpal::FromSample<f32> + 'static,
{
    let stream = device
        .build_output_stream(
            config,
            move |output: &mut [T], _| {
                let mut buf = pending.lock().unwrap();
                for sample in output.iter_mut() {
                    let s_f32 = buf.pop_front().unwrap_or(0.0);
                    *sample = T::from_sample(s_f32);
                }
            },
            |err| tracing::error!("Playback stream error: {err}"),
            None,
        )
        .map_err(|e| Error::AudioError(format!("failed to build output stream: {e}")))?;

    Ok(stream)
}

fn resample_linear(input: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if src_rate == dst_rate {
        return input.to_vec();
    }

    let ratio = src_rate as f64 / dst_rate as f64;
    let dst_len = (input.len() as f64 / ratio).round() as usize;
    let mut output = Vec::with_capacity(dst_len);

    for i in 0..dst_len {
        let src_index = i as f64 * ratio;
        let index_low = src_index.floor() as usize;
        let index_high = (index_low + 1).min(input.len() - 1);
        let weight = src_index - index_low as f64;

        let sample = if index_low < input.len() {
            let low = input[index_low];
            let high = input[index_high];
            low + (high - low) * weight as f32
        } else {
            0.0
        };
        output.push(sample);
    }
    output
}

fn convert_channels(input: &[f32], target_channels: u16) -> Vec<f32> {
    if target_channels == 1 {
        return input.to_vec();
    }

    let mut output = Vec::with_capacity(input.len() * target_channels as usize);
    for &sample in input {
        for _ in 0..target_channels {
            output.push(sample);
        }
    }
    output
}

