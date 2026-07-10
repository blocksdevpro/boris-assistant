use boris_core::error::{Error, Result};
use boris_core::{AudioBuffer, AudioSample};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler as RubatoResampler};

/// Streaming resampler from device rate → pipeline rate ([`boris_core::AUDIO_TARGET_RATE`]).
///
/// Uses rubato's fixed-input FFT resampler. **Critical:** only the frames actually
/// produced by each `process_into_buffer` call are returned — the max-sized
/// output buffer may contain trailing zeros that must not be treated as audio.
pub struct Resampler {
    resampler: Option<Fft<AudioSample>>,
    /// Output channel count (pipeline is mono → 1).
    channels: u32,
    input_rate: u32,
    output_rate: u32,
    /// Chunk size the FFT was built for (fixed-input side).
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

    /// Resample one fixed-size mono (or interleaved multi-channel) chunk.
    ///
    /// When `input_rate == output_rate`, returns a copy of `input` (no FFT).
    /// Otherwise requires every call to use the **same** frame count as the first
    /// call (the size the FFT was constructed with).
    pub fn resample(&mut self, input: &[AudioSample]) -> Result<AudioBuffer> {
        if input.is_empty() {
            return Ok(Vec::new());
        }

        // Identity path — common when the device is already 16 kHz.
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

        if let Some(expected) = self.chunk_frames {
            if input_frames != expected {
                return Err(Error::AudioError(format!(
                    "resampler expects {expected} frames per call, got {input_frames}"
                )));
            }
        }

        let input_rate = self.input_rate as usize;
        let output_rate = self.output_rate as usize;

        let resampler = self.resampler.get_or_insert_with(|| {
            Fft::<AudioSample>::new(
                input_rate,
                output_rate,
                input_frames,
                2,
                channels,
                FixedSync::Input,
            )
            .expect("[ERROR] failed to create resampler!")
        });
        self.chunk_frames.get_or_insert(input_frames);

        let output_capacity = resampler.output_frames_max() * channels;
        let mut output_buffer = vec![AudioSample::default(); output_capacity];

        let input_slice = InterleavedSlice::new(input, channels, input_frames).map_err(|e| {
            Error::AudioError(format!("failed to create input slice for resampling: {e}"))
        })?;
        let mut output_slice =
            InterleavedSlice::new_mut(&mut output_buffer, channels, resampler.output_frames_max())
                .map_err(|e| {
                    Error::AudioError(format!(
                        "failed to create output slice for resampling: {e}"
                    ))
                })?;

        // Rubato writes only `produced` frames; the rest of the buffer stays zero.
        let (_consumed, produced) = resampler
            .process_into_buffer(&input_slice, &mut output_slice, None)
            .map_err(|e| Error::AudioError(format!("resampling failed: {e}")))?;

        output_buffer.truncate(produced * channels);
        Ok(output_buffer)
    }
}
