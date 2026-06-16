use boris_core::error::Result;
use boris_core::{AudioBuffer, AudioSample};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler as RubatoResampler};

pub struct AudioResampler {
    resampler: Option<Fft<AudioSample>>,
    channels: u32,   // output channels, this should be 1 as we are using mono audio.
    input_rate: u32, //
    output_rate: u32,
}

impl AudioResampler {
    pub fn new(channels: u32, input_rate: u32, output_rate: u32) -> Self {
        Self {
            resampler: None,
            channels,
            input_rate,
            output_rate,
        }
    }

    pub fn resample(&mut self, input: &[AudioSample]) -> Result<AudioBuffer> {
        let input_length = input.len();
        let resampler = self.resampler.get_or_insert_with(|| {
            Fft::<AudioSample>::new(
                self.input_rate as usize,
                self.output_rate as usize,
                input.len(),
                2,
                self.channels as usize,
                FixedSync::Input,
            )
            // should throw an BorisError
            .expect("[ERROR] failed to create resampler!")
        });

        let output_length = resampler.output_frames_max() * self.channels as usize;
        let mut output_buffer = vec![AudioSample::default(); output_length];

        let input_slice = InterleavedSlice::new(input, self.channels as usize, input_length)
            .expect("[ERROR] failed to create input slice for resampling!");
        let mut output_slice =
            InterleavedSlice::new_mut(&mut output_buffer, self.channels as usize, output_length)
                .expect("[ERROR] failed to create output slice for resampling!");

        resampler
            .process_into_buffer(&input_slice, &mut output_slice, None)
            .expect("[ERROR] resampling failed!");
        Ok(output_buffer)
    }
}
