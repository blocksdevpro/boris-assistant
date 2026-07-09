use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use boris_core::event::Event;
use boris_core::TurnId;
use boris_core::{
    error::{Error, Result},
    AudioBuffer,
};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Stream, StreamConfig,
};

pub struct PlayJob {
    pub turn: TurnId,
    pub pcm: AudioBuffer,
}

struct PlaybackState {
    /// Device-rate interleaved (or mono) samples waiting to play.
    pending: VecDeque<f32>,
    /// Turn currently draining out the speakers. None = idle / silence.
    active_turn: Option<TurnId>,
    /// How many consecutive output callbacks saw an empty queue
    /// while we still had an active_turn (underrun / drained).
    empty_callbacks: u32,
}

/// After this many empty callbacks, declare finished.
/// Tune: 3–8 is typical. Depends on callback size (~few ms each).
const DRAIN_EMPTY_CALLBACKS: u32 = 5;

pub struct Playback {
    _stream: Stream,
}

impl Playback {
    /// Spawn an output stream that drains f32 PCM samples from `audio_rx`
    /// and plays them through the default output device.
    ///
    /// `sample_rate` — must match whatever rate the TTS model outputs
    /// (Kokoro = 24_000 Hz).
    pub fn new(
        audio_rx: Receiver<PlayJob>,
        source_sample_rate: u32,
        event_tx: Sender<Event>,
    ) -> Result<Self> {
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

        let playback_state = Arc::new(Mutex::new(PlaybackState {
            pending: VecDeque::new(),
            active_turn: None,
            empty_callbacks: 0,
        }));

        let state_clone = playback_state.clone();

        // A background thread refills the pending buffer from the channel,
        // resampling and converting channels as needed.
        std::thread::spawn(move || {
            while let Ok(job) = audio_rx.recv() {
                let resampled = resample_linear(&job.pcm, source_sample_rate, target_sample_rate);
                let channel_converted = convert_channels(&resampled, target_channels);

                let mut state = state_clone.lock().unwrap();

                // If a previous turn was still playing, you have a product choice:
                //   (a) append (queue utterances) — advanced
                //   (b) clear and replace — simple, OK for Boris today
                // Phase 2: (b) is fine and matches “one turn at a time”.

                state.pending.clear();
                state.pending.extend(channel_converted);
                state.active_turn = Some(job.turn);
                state.empty_callbacks = 0;
            }
        });

        let stream = match dconfig.sample_format() {
            cpal::SampleFormat::F32 => build_output_stream::<f32>(
                &device,
                stream_config,
                playback_state.clone(),
                event_tx.clone(),
            )?,
            cpal::SampleFormat::I16 => build_output_stream::<i16>(
                &device,
                stream_config,
                playback_state.clone(),
                event_tx.clone(),
            )?,
            cpal::SampleFormat::U16 => build_output_stream::<u16>(
                &device,
                stream_config,
                playback_state.clone(),
                event_tx.clone(),
            )?,
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
    state: Arc<Mutex<PlaybackState>>,
    event_tx: Sender<Event>,
) -> Result<Stream>
where
    T: cpal::Sample + cpal::SizedSample + cpal::FromSample<f32> + 'static,
{
    let stream = device
        .build_output_stream(
            config,
            move |output: &mut [T], _| {
                let mut state = state.lock().unwrap();
                let mut got_real_sample = false;

                for sample in output.iter_mut() {
                    if let Some(s) = state.pending.pop_front() {
                        *sample = T::from_sample(s);
                        got_real_sample = true;
                    } else {
                        *sample = T::from_sample(0.0);
                    }
                }

                if state.active_turn.is_some() {
                    if got_real_sample || !state.pending.is_empty() {
                        state.empty_callbacks = 0;
                    } else {
                        state.empty_callbacks = state.empty_callbacks.saturating_add(1);
                        if state.empty_callbacks >= DRAIN_EMPTY_CALLBACKS {
                            if let Some(turn) = state.active_turn.take() {
                                state.empty_callbacks = 0;
                                let _ = event_tx.send(Event::PlaybackFinished { turn }).ok();
                            }
                        }
                    }
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
