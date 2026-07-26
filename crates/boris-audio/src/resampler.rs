use boris_core::error::{Error, Result};
use boris_core::{AudioBuffer, AudioSample, AUDIO_TARGET_RATE};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler as RubatoResampler};

/// Core rate-conversion primitive. Works for both realtime streaming
/// (stable chunk size after warm-up) and one-shot buffers (chunk size
/// varies per call) by rebuilding the internal FFT resampler whenever
/// the input chunk size changes, instead of erroring.
pub struct Resampler {
    resampler: Option<Fft<AudioSample>>,
    channels: u32,
    input_rate: u32,
    output_rate: u32,
    chunk_frames: Option<usize>,
}

impl Resampler {
    pub fn new(channels: u32, input_rate: u32, output_rate: u32) -> Self {
        Self {
            resampler: None,
            channels,
            input_rate,
            output_rate,
            chunk_frames: None,
        }
    }

    /// Resample one chunk of interleaved audio at `self.channels` channels.
    pub fn resample(&mut self, input: &[AudioSample]) -> Result<AudioBuffer> {
        if input.is_empty() {
            return Ok(Vec::new());
        }

        // Identity path — no FFT needed.
        if self.input_rate == self.output_rate {
            return Ok(input.to_vec());
        }

        let channels = self.channels as usize;
        if channels == 0 {
            return Err(Error::AudioError("resampler channels must be > 0".into()));
        }
        if !input.len().is_multiple_of(channels) {
            return Err(Error::AudioError(format!(
                "input length {} is not a multiple of channel count {}",
                input.len(),
                channels
            )));
        }

        let input_frames = input.len() / channels;

        // Rebuild only when chunk size actually changes. Streaming callers
        // (mic, stable chunk size after warm-up) pay this once. One-shot
        // callers (TTS output, variable utterance lengths) pay it per call —
        // off the realtime capture hot path, so it's cheap where it happens.
        let needs_rebuild = self.resampler.is_none() || self.chunk_frames != Some(input_frames);
        if needs_rebuild {
            let input_rate = self.input_rate as usize;
            let output_rate = self.output_rate as usize;
            self.resampler = Some(
                Fft::<AudioSample>::new(
                    input_rate,
                    output_rate,
                    input_frames,
                    2,
                    channels,
                    FixedSync::Input,
                )
                .map_err(|e| Error::AudioError(format!("failed to create resampler: {e}")))?,
            );
            self.chunk_frames = Some(input_frames);
        }

        let resampler = self
            .resampler
            .as_mut()
            .expect("resampler just built or already present");

        let output_frames = resampler.output_frames_max();
        let output_capacity = output_frames * channels;
        // Must be length-initialized: InterleavedSlice::new_mut checks buf.len(),
        // not capacity. Vec::with_capacity alone leaves len=0 and always errors.
        let mut output_buffer: Vec<AudioSample> = vec![AudioSample::default(); output_capacity];

        let input_slice = InterleavedSlice::new(input, channels, input_frames).map_err(|e| {
            Error::AudioError(format!("failed to create input slice for resampling: {e}"))
        })?;
        let mut output_slice =
            InterleavedSlice::new_mut(&mut output_buffer, channels, output_frames).map_err(
                |e| Error::AudioError(format!("failed to create output slice for resampling: {e}")),
            )?;

        // Rubato writes only `produced` frames; the rest of the buffer stays zero.
        let (_consumed, produced) = resampler
            .process_into_buffer(&input_slice, &mut output_slice, None)
            .map_err(|e| Error::AudioError(format!("resampling failed: {e}")))?;

        output_buffer.truncate(produced * channels);
        Ok(output_buffer)
    }
}

/// Downmix interleaved N-channel audio to mono by averaging channels per frame.
/// No-op if `channels <= 1`.
pub fn downmix_to_mono(input: &[AudioSample], channels: u32) -> AudioBuffer {
    let channels = channels as usize;
    if channels <= 1 {
        return input.to_vec();
    }
    input
        .chunks_exact(channels)
        .map(|frame| frame.iter().copied().sum::<AudioSample>() / channels as AudioSample)
        .collect()
}

/// Duplicate a mono buffer across `target_channels`, interleaved.
/// No-op if `target_channels <= 1`.
pub fn convert_channels(input: &[AudioSample], target_channels: u16) -> AudioBuffer {
    if target_channels <= 1 {
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

/// Pipeline order: n_channels -> mono -> [`AUDIO_TARGET_RATE`].
/// Downmix happens before rate conversion so the FFT resampler only ever
/// processes a single channel of work, never N.
pub struct InputResampler {
    resampler: Resampler,
    src_channels: u32,
}

impl InputResampler {
    pub fn new(src_channels: u32, src_rate: u32) -> Self {
        Self {
            resampler: Resampler::new(1, src_rate, AUDIO_TARGET_RATE),
            src_channels,
        }
    }

    /// `raw` is interleaved at `src_channels` channels, `src_rate` Hz.
    /// Returns mono audio at `AUDIO_TARGET_RATE`.
    pub fn process(&mut self, raw: &[AudioSample]) -> Result<AudioBuffer> {
        let mono = downmix_to_mono(raw, self.src_channels);
        self.resampler.resample(&mono)
    }
}

/// Pipeline order: n_rate -> device_rate -> device_channels.
/// Rate conversion happens before channel duplication so resampling work
/// isn't repeated once per output channel.
pub struct OutputResampler {
    resampler: Resampler,
    device_channels: u16,
}

impl OutputResampler {
    pub fn new(src_rate: u32, device_rate: u32, device_channels: u16) -> Self {
        Self {
            resampler: Resampler::new(1, src_rate, device_rate),
            device_channels,
        }
    }

    /// `mono` is mono audio at `src_rate` Hz (e.g. TTS output at
    /// `AUDIO_TARGET_RATE`). Returns interleaved audio at `device_rate`,
    /// `device_channels` channels.
    pub fn process(&mut self, mono: &[AudioSample]) -> Result<AudioBuffer> {
        let resampled = self.resampler.resample(mono)?;
        Ok(convert_channels(&resampled, self.device_channels))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_stereo_averages_channels() {
        let stereo = vec![1.0, 3.0, 2.0, 4.0]; // frame0: L=1,R=3  frame1: L=2,R=4
        let mono = downmix_to_mono(&stereo, 2);
        assert_eq!(mono, vec![2.0, 3.0]);
    }

    #[test]
    fn downmix_mono_is_noop() {
        let mono = vec![1.0, 2.0, 3.0];
        assert_eq!(downmix_to_mono(&mono, 1), mono);
    }

    #[test]
    fn convert_channels_duplicates_mono_to_stereo() {
        let mono = vec![1.0, 2.0];
        assert_eq!(convert_channels(&mono, 2), vec![1.0, 1.0, 2.0, 2.0]);
    }

    #[test]
    fn resampler_rebuilds_on_chunk_size_change() {
        let mut r = Resampler::new(1, 48_000, AUDIO_TARGET_RATE);
        let chunk_a = vec![0.0f32; 480];
        let chunk_b = vec![0.0f32; 960]; // different length — one-shot / TTS-style use
        assert!(r.resample(&chunk_a).is_ok());
        assert!(r.resample(&chunk_b).is_ok()); // would have errored in the old version
    }
}
