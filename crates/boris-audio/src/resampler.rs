use boris_core::error::{Error, Result};
use boris_core::{AudioBuffer, AudioSample, AUDIO_TARGET_RATE};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler as RubatoResampler};

/// Fixed processing block for the FFT resampler (frames, not samples).
///
/// Important: never pass a whole multi-second TTS utterance as the rubato
/// `chunk_size`. That builds a huge FFT and, with one process call, floors away
/// large amounts of audio (e.g. ~7s in → ~3.5s out). Always stream fixed chunks.
const FFT_CHUNK_FRAMES: usize = 1024;

/// Core rate-conversion primitive.
///
/// Uses a **fixed** FFT chunk size and walks the input in blocks so one-shot
/// buffers (TTS) and streaming chunks (mic) both convert fully.
pub struct Resampler {
    resampler: Option<Fft<AudioSample>>,
    channels: u32,
    input_rate: u32,
    output_rate: u32,
}

impl Resampler {
    pub fn new(channels: u32, input_rate: u32, output_rate: u32) -> Self {
        Self {
            resampler: None,
            channels,
            input_rate,
            output_rate,
        }
    }

    fn ensure_resampler(&mut self) -> Result<()> {
        if self.resampler.is_some() {
            return Ok(());
        }
        let channels = self.channels as usize;
        // sub_chunks ≈ 4 → ~256-frame FFT blocks for CHUNK=1024 (good delay/quality).
        self.resampler = Some(
            Fft::<AudioSample>::new(
                self.input_rate as usize,
                self.output_rate as usize,
                FFT_CHUNK_FRAMES,
                4,
                channels,
                FixedSync::Input,
            )
            .map_err(|e| Error::AudioError(format!("failed to create resampler: {e}")))?,
        );
        Ok(())
    }

    /// Resample interleaved audio at `self.channels` channels.
    ///
    /// Accepts any length (including multi-second TTS). Internally walks fixed
    /// FFT chunks and flushes the resampler delay so the tail is not dropped.
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

        self.ensure_resampler()?;
        let resampler = self
            .resampler
            .as_mut()
            .expect("resampler just built or already present");

        // Reset internal delay state so consecutive one-shot TTS calls don't
        // leak samples into each other.
        resampler.reset();

        let input_frames = input.len() / channels;
        let mut output: Vec<AudioSample> = Vec::with_capacity(
            ((input_frames as u64 * self.output_rate as u64) / self.input_rate as u64) as usize
                * channels
                + FFT_CHUNK_FRAMES * channels,
        );

        let mut frame_pos = 0usize;
        while frame_pos < input_frames {
            let need = resampler.input_frames_next();
            let available = (input_frames - frame_pos).min(need);

            // Always feed exactly `need` frames; pad the tail with silence.
            let mut in_buf = vec![0.0f32; need * channels];
            let copy_samples = available * channels;
            let src = frame_pos * channels;
            in_buf[..copy_samples].copy_from_slice(&input[src..src + copy_samples]);

            let out_cap = resampler.output_frames_next().max(resampler.output_frames_max());
            let mut out_buf = vec![0.0f32; out_cap * channels];

            let in_slice = InterleavedSlice::new(&in_buf, channels, need).map_err(|e| {
                Error::AudioError(format!("failed to create input slice for resampling: {e}"))
            })?;
            let mut out_slice = InterleavedSlice::new_mut(&mut out_buf, channels, out_cap)
                .map_err(|e| {
                    Error::AudioError(format!(
                        "failed to create output slice for resampling: {e}"
                    ))
                })?;

            let (_consumed, produced) = resampler
                .process_into_buffer(&in_slice, &mut out_slice, None)
                .map_err(|e| Error::AudioError(format!("resampling failed: {e}")))?;

            output.extend_from_slice(&out_buf[..produced * channels]);
            frame_pos += available;

            // After a padded (last) block, stop reading input and flush below.
            if available < need {
                break;
            }
        }

        // Flush FFT delay with silent input until we cover output_delay.
        let delay = resampler.output_delay();
        let mut flushed = 0usize;
        while flushed < delay {
            let need = resampler.input_frames_next();
            let in_buf = vec![0.0f32; need * channels];
            let out_cap = resampler.output_frames_next().max(resampler.output_frames_max());
            let mut out_buf = vec![0.0f32; out_cap * channels];

            let in_slice = InterleavedSlice::new(&in_buf, channels, need).map_err(|e| {
                Error::AudioError(format!("failed to create flush input slice: {e}"))
            })?;
            let mut out_slice = InterleavedSlice::new_mut(&mut out_buf, channels, out_cap)
                .map_err(|e| {
                    Error::AudioError(format!("failed to create flush output slice: {e}"))
                })?;

            let (_consumed, produced) = resampler
                .process_into_buffer(&in_slice, &mut out_slice, None)
                .map_err(|e| Error::AudioError(format!("resampler flush failed: {e}")))?;

            output.extend_from_slice(&out_buf[..produced * channels]);
            flushed = flushed.saturating_add(produced);
            if produced == 0 {
                break;
            }
        }

        Ok(output)
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
    fn resampler_accepts_varying_chunk_sizes() {
        let mut r = Resampler::new(1, 48_000, AUDIO_TARGET_RATE);
        let chunk_a = vec![0.0f32; 480];
        let chunk_b = vec![0.0f32; 960];
        assert!(r.resample(&chunk_a).is_ok());
        assert!(r.resample(&chunk_b).is_ok());
    }

    /// One-shot TTS-sized buffers must not be truncated by the FFT chunk floor.
    #[test]
    fn resampler_preserves_long_buffer_duration() {
        let mut r = Resampler::new(1, 44_100, 48_000);
        // ~7.036s of mono @ 44.1 kHz (matches a typical Supertone utterance).
        let input = vec![0.1f32; 310_327];
        let out = r.resample(&input).expect("resample");
        let expected = (input.len() as f64 * 48_000.0 / 44_100.0).round();
        let ratio = out.len() as f64 / expected;
        // Allow a little FFT delay / block alignment slack, but not half the audio.
        assert!(
            ratio > 0.95 && ratio < 1.05,
            "duration ratio {ratio:.3} (out={}, expected≈{expected})",
            out.len()
        );
    }
}
