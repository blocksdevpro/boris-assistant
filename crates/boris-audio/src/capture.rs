use boris_core::{
    error::{Error, Result},
    AudioBuffer, AudioSample,
};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    BufferSize, Device, FromSample, Sample, SampleFormat, Stream, StreamConfig,
};
use crossbeam_channel::Sender;

pub struct Capture {
    _stream: Stream,
    pub sample_rate: u32,
}

impl Capture {
    pub fn new(audio_tx: Sender<AudioBuffer>) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| Error::AudioError("no input device found!".to_string()))?;
        let config = device
            .default_input_config()
            .map_err(|err| Error::AudioError(err.to_string()))?;

        let channels = config.channels() as usize;
        let sample_rate = config.sample_rate();

        let stream_config = StreamConfig {
            channels: channels as u16,
            sample_rate,
            buffer_size: BufferSize::Default,
        };

        let stream = match config.sample_format() {
            SampleFormat::F32 => {
                build_input_stream::<f32>(&device, stream_config, channels, audio_tx)?
            }
            SampleFormat::I16 => {
                build_input_stream::<i16>(&device, stream_config, channels, audio_tx)?
            }
            SampleFormat::U16 => {
                build_input_stream::<u16>(&device, stream_config, channels, audio_tx)?
            }
            _ => {
                return Err(Error::AudioError(
                    "[Boris.AudioCapture] unsupported sample format".into(),
                ));
            }
        };

        stream.play().map_err(|err| {
            Error::AudioError(
                "[Boris.AudioCapture] failed to play the stream: ".to_string() + &err.to_string(),
            )
        })?;

        Ok(Self {
            _stream: stream,
            sample_rate,
        })
    }
}

fn build_input_stream<T>(
    device: &Device,
    config: StreamConfig,
    channels: usize,
    audio_tx: Sender<AudioBuffer>,
) -> Result<Stream>
where
    T: Sample + cpal::SizedSample + 'static,
    AudioSample: FromSample<T>,
{
    let stream = device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                // Normalize every device format (i16/u16/f32/…) into f32 [-1, 1].
                // Raw `Into<f32>` on integer samples yields ±32768-scale values and
                // saturates every downstream PCM path that clamps to [-1, 1].
                let mono_samples: Vec<AudioSample> = data
                    .chunks(channels)
                    .map(|frame| {
                        frame
                            .iter()
                            .map(|sample| AudioSample::from_sample(*sample))
                            .sum::<AudioSample>()
                            / channels as AudioSample
                    })
                    .collect();
                audio_tx.try_send(mono_samples).ok();
            },
            |err| tracing::error!("Audio capture failed: {err}"),
            None,
        )
        .map_err(|err| Error::AudioError("[Boris.AudioCapture] ".to_string() + &err.to_string()))?;

    Ok(stream)
}
