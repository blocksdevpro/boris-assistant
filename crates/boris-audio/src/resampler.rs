//! Sample-rate conversion (rubato FFT) plus input/output pipeline wrappers.
//!
//! Channel layout helpers live in [`crate::channels`].

use boris_core::{AudioBuffer, AudioSample, Error, Result, AUDIO_TARGET_RATE};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler as RubatoResampler};

use crate::channels::{self, convert_channels, downmix_to_mono};

/// Fixed processing block for the FFT resampler (frames, not samples).
///
/// Never pass a multi-second TTS utterance as rubato `chunk_size` — that builds
/// a huge FFT and floors away large amounts of audio on a single process call.
const FFT_CHUNK_FRAMES: usize = 1024;

/// Core rate-conversion primitive shared by capture (streaming) and TTS (one-shot).
///
/// - [`Self::resample_stream`]: keep FFT state across mic callbacks; no pad/flush.
/// - [`Self::resample_oneshot`]: full buffer conversion with reset + delay flush.
pub struct Resampler {
    resampler: Option<Fft<AudioSample>>,
    channels: u32,
    input_rate: u32,
    output_rate: u32,
    /// Interleaved samples waiting for a full FFT input block (stream path only).
    pending: Vec<AudioSample>,
}

impl Resampler {
    /// Create a resampler for interleaved audio with `channels` channels.
    pub fn new(channels: u32, input_rate: u32, output_rate: u32) -> Self {
        Self {
            resampler: None,
            channels,
            input_rate,
            output_rate,
            pending: Vec::new(),
        }
    }

    /// Input sample rate (Hz).
    pub fn input_rate(&self) -> u32 {
        self.input_rate
    }

    /// Output sample rate (Hz).
    pub fn output_rate(&self) -> u32 {
        self.output_rate
    }

    fn same_rate(&self) -> bool {
        self.input_rate == self.output_rate
    }

    fn ensure_resampler(&mut self) -> Result<()> {
        if self.resampler.is_some() {
            return Ok(());
        }
        let channels = self.channels as usize;
        // sub_chunks ≈ 4 → ~256-frame FFT blocks for CHUNK=1024.
        self.resampler = Some(
            Fft::<AudioSample>::new(
                self.input_rate as usize,
                self.output_rate as usize,
                FFT_CHUNK_FRAMES,
                4,
                channels,
                FixedSync::Input,
            )
            .map_err(|e| Error::audio(format!("failed to create resampler: {e}")))?,
        );
        Ok(())
    }

    fn validate_input(&self, input: &[AudioSample]) -> Result<usize> {
        let channels = self.channels as usize;
        if channels == 0 {
            return Err(Error::audio("resampler channels must be > 0"));
        }
        if !input.len().is_multiple_of(channels) {
            return Err(Error::audio(format!(
                "input length {} is not a multiple of channel count {}",
                input.len(),
                channels
            )));
        }
        Ok(channels)
    }

    fn process_block(
        resampler: &mut Fft<AudioSample>,
        channels: usize,
        in_buf: &[AudioSample],
        in_frames: usize,
    ) -> Result<AudioBuffer> {
        let out_cap = resampler
            .output_frames_next()
            .max(resampler.output_frames_max());
        let mut out_buf = vec![0.0f32; out_cap * channels];

        let in_slice = InterleavedSlice::new(in_buf, channels, in_frames).map_err(|e| {
            Error::audio(format!("failed to create input slice for resampling: {e}"))
        })?;
        let mut out_slice =
            InterleavedSlice::new_mut(&mut out_buf, channels, out_cap).map_err(|e| {
                Error::audio(format!("failed to create output slice for resampling: {e}"))
            })?;

        let (_consumed, produced) = resampler
            .process_into_buffer(&in_slice, &mut out_slice, None)
            .map_err(|e| Error::audio(format!("resampling failed: {e}")))?;

        out_buf.truncate(produced * channels);
        Ok(out_buf)
    }

    /// Streaming path for mic capture.
    ///
    /// Accumulates samples until a full FFT input block is ready. Does **not**
    /// reset state or pad/flush with silence — that would corrupt continuous audio.
    pub fn resample_stream(&mut self, input: &[AudioSample]) -> Result<AudioBuffer> {
        if input.is_empty() {
            return Ok(Vec::new());
        }
        if self.same_rate() {
            return Ok(input.to_vec());
        }

        let channels = self.validate_input(input)?;
        self.ensure_resampler()?;
        self.pending.extend_from_slice(input);

        let mut output = Vec::new();
        loop {
            let need = self
                .resampler
                .as_ref()
                .expect("resampler present")
                .input_frames_next();
            let need_samples = need * channels;
            if self.pending.len() < need_samples {
                break;
            }

            let chunk: Vec<AudioSample> = self.pending.drain(..need_samples).collect();
            let resampler = self.resampler.as_mut().expect("resampler present");
            let block = Self::process_block(resampler, channels, &chunk, need)?;
            output.extend_from_slice(&block);
        }

        Ok(output)
    }

    /// One-shot path for TTS / offline buffers.
    ///
    /// Resets FFT state, converts the full buffer, pads the last block, and
    /// flushes delay so the tail is not dropped.
    pub fn resample_oneshot(&mut self, input: &[AudioSample]) -> Result<AudioBuffer> {
        if input.is_empty() {
            return Ok(Vec::new());
        }
        if self.same_rate() {
            return Ok(input.to_vec());
        }

        let channels = self.validate_input(input)?;
        self.ensure_resampler()?;

        // Drop any stream leftovers and clear FFT delay between independent jobs.
        self.pending.clear();
        self.resampler.as_mut().expect("resampler present").reset();

        let input_frames = input.len() / channels;
        let mut output: Vec<AudioSample> = Vec::with_capacity(
            ((input_frames as u64 * self.output_rate as u64) / self.input_rate as u64) as usize
                * channels
                + FFT_CHUNK_FRAMES * channels,
        );

        let mut frame_pos = 0usize;
        while frame_pos < input_frames {
            let need = self
                .resampler
                .as_ref()
                .expect("resampler present")
                .input_frames_next();
            let available = (input_frames - frame_pos).min(need);

            let mut in_buf = vec![0.0f32; need * channels];
            let copy_samples = available * channels;
            let src = frame_pos * channels;
            in_buf[..copy_samples].copy_from_slice(&input[src..src + copy_samples]);

            let resampler = self.resampler.as_mut().expect("resampler present");
            let block = Self::process_block(resampler, channels, &in_buf, need)?;
            output.extend_from_slice(&block);
            frame_pos += available;

            if available < need {
                break;
            }
        }

        // Flush FFT delay with silent input.
        let delay = self
            .resampler
            .as_ref()
            .expect("resampler present")
            .output_delay();
        let mut flushed = 0usize;
        while flushed < delay {
            let need = self
                .resampler
                .as_ref()
                .expect("resampler present")
                .input_frames_next();
            let in_buf = vec![0.0f32; need * channels];
            let resampler = self.resampler.as_mut().expect("resampler present");
            let block = Self::process_block(resampler, channels, &in_buf, need)?;
            let produced = block.len() / channels;
            output.extend_from_slice(&block);
            flushed = flushed.saturating_add(produced);
            if produced == 0 {
                break;
            }
        }

        Ok(output)
    }

    /// Alias for [`Self::resample_stream`] (mic callers).
    pub fn resample(&mut self, input: &[AudioSample]) -> Result<AudioBuffer> {
        self.resample_stream(input)
    }
}

// Re-export channel helpers so older `resampler::downmix_to_mono` paths work.
#[doc(inline)]
pub use channels::upmix_mono;

/// Capture path: N-channel device rate → mono [`AUDIO_TARGET_RATE`].
///
/// Downmix happens **before** rate conversion so the FFT only processes mono.
pub struct InputResampler {
    resampler: Resampler,
    src_channels: u32,
}

impl InputResampler {
    /// `src_channels` / `src_rate` are the device capture layout.
    pub fn new(src_channels: u32, src_rate: u32) -> Self {
        Self {
            resampler: Resampler::new(1, src_rate, AUDIO_TARGET_RATE),
            src_channels,
        }
    }

    /// `raw` is interleaved at device channels / rate.
    /// Returns mono at [`AUDIO_TARGET_RATE`] (may be empty until a full FFT block).
    pub fn process(&mut self, raw: &[AudioSample]) -> Result<AudioBuffer> {
        let mono = downmix_to_mono(raw, self.src_channels);
        self.resampler.resample_stream(&mono)
    }
}

/// Playback path: mono TTS rate → device rate + channel upmix.
///
/// Rate conversion runs on mono first; then samples are duplicated per channel.
pub struct OutputResampler {
    resampler: Resampler,
    device_channels: u16,
}

impl OutputResampler {
    /// `src_rate` is the rate of PCM passed to [`crate::AudioService::play`].
    pub fn new(src_rate: u32, device_rate: u32, device_channels: u16) -> Self {
        Self {
            resampler: Resampler::new(1, src_rate, device_rate),
            device_channels,
        }
    }

    /// `mono` is mono PCM at the configured source rate (TTS one-shot).
    pub fn process(&mut self, mono: &[AudioSample]) -> Result<AudioBuffer> {
        let resampled = self.resampler.resample_oneshot(mono)?;
        Ok(convert_channels(&resampled, self.device_channels))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_accumulates_small_mic_chunks() {
        let mut r = Resampler::new(1, 48_000, AUDIO_TARGET_RATE);
        let mut total_out = 0usize;
        for _ in 0..20 {
            let chunk = vec![0.1f32; 480];
            let out = r.resample_stream(&chunk).expect("stream");
            total_out += out.len();
        }
        assert!(total_out > 0, "streaming resampler produced no output");
    }

    #[test]
    fn oneshot_preserves_long_buffer_duration() {
        let mut r = Resampler::new(1, 44_100, 48_000);
        let input = vec![0.1f32; 310_327];
        let out = r.resample_oneshot(&input).expect("oneshot");
        let expected = (input.len() as f64 * 48_000.0 / 44_100.0).round();
        let ratio = out.len() as f64 / expected;
        assert!(
            ratio > 0.95 && ratio < 1.05,
            "duration ratio {ratio:.3} (out={}, expected≈{expected})",
            out.len()
        );
    }

    #[test]
    fn stream_does_not_reset_between_chunks() {
        let mut r = Resampler::new(1, 48_000, AUDIO_TARGET_RATE);
        let first = r.resample_stream(&vec![0.2f32; 100]).unwrap();
        let mut produced = first.len();
        let mut fed = 100usize;
        for _ in 0..50 {
            let chunk = vec![0.2f32; 512];
            fed += chunk.len();
            produced += r.resample_stream(&chunk).unwrap().len();
        }
        let expected_min = (fed as f64 * AUDIO_TARGET_RATE as f64 / 48_000.0 * 0.5) as usize;
        assert!(
            produced > expected_min,
            "stream under-produced: produced={produced} fed={fed} expected_min={expected_min}"
        );
    }

    #[test]
    fn same_rate_is_copy() {
        let mut r = Resampler::new(1, 16_000, 16_000);
        let in_buf = vec![0.5f32; 64];
        assert_eq!(r.resample_stream(&in_buf).unwrap(), in_buf);
        assert_eq!(r.resample_oneshot(&in_buf).unwrap(), in_buf);
    }
}
