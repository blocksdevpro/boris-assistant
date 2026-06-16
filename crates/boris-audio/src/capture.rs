use boris_core::{
    AudioBuffer, AudioSample,
    error::{BorisError, BorisResult},
};
use cpal::{
    BufferSize, Device, SampleFormat, Stream, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use crossbeam_channel::Sender;
pub struct AudioCapture {
    _stream: Stream,
    pub sample_rate: u32,
}

impl AudioCapture {
    pub fn new(audio_tx: Sender<AudioBuffer>) -> BorisResult<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| BorisError::AudioError("no input device found!".to_string()))?;
        let config = device
            .default_input_config()
            .map_err(|err| BorisError::AudioError(err.to_string()))?;

        let channels = config.channels() as usize;
        let sample_rate = config.sample_rate();

        let stream_config = StreamConfig {
            channels: channels as u16,
            sample_rate: sample_rate,
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
                return Err(BorisError::AudioError(
                    "[Boris.AudioCapture] unsupported sample format".into(),
                ));
            }
        };

        stream.play().map_err(|err| {
            BorisError::AudioError(
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
) -> BorisResult<Stream>
where
    T: cpal::Sample + cpal::SizedSample + Into<AudioSample> + 'static,
{
    let stream = device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                let mono_samples: Vec<AudioSample> = data
                    .chunks(channels)
                    .map(|frame| {
                        frame
                            .iter()
                            .map(|sample| (*sample).into())
                            .sum::<AudioSample>()
                            / channels as AudioSample
                    })
                    .collect();
                audio_tx.try_send(mono_samples).ok();
            },
            |err| eprintln!("[ERROR] [Boris.AudioCapture] capture error: {err}"),
            None,
        )
        .map_err(|err| {
            BorisError::AudioError("[Boris.AudioCapture] ".to_string() + &err.to_string())
        })?;

    Ok(stream)
}
