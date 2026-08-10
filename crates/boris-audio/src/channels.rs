//! Channel layout helpers (downmix / upmix). Independent of sample rate.

use boris_core::{AudioBuffer, AudioSample};

/// Downmix interleaved N-channel audio to mono by averaging channels per frame.
///
/// No-op if `channels <= 1`.
pub fn downmix_to_mono(input: &[AudioSample], channels: u32) -> AudioBuffer {
    let channels = channels as usize;
    if channels <= 1 {
        return input.to_vec();
    }
    if channels == 0 || !input.len().is_multiple_of(channels) {
        // Degenerate input: return empty rather than panic in RT paths.
        tracing::warn!(
            input_len = input.len(),
            channels,
            "downmix_to_mono: length not multiple of channels; returning empty"
        );
        return Vec::new();
    }
    input
        .chunks_exact(channels)
        .map(|frame| frame.iter().copied().sum::<AudioSample>() / channels as AudioSample)
        .collect()
}

/// Duplicate a mono buffer across `target_channels`, interleaved.
///
/// No-op if `target_channels <= 1`.
pub fn upmix_mono(input: &[AudioSample], target_channels: u16) -> AudioBuffer {
    if target_channels <= 1 {
        return input.to_vec();
    }
    let n = target_channels as usize;
    let mut output = Vec::with_capacity(input.len() * n);
    for &sample in input {
        for _ in 0..n {
            output.push(sample);
        }
    }
    output
}

/// Backward-compatible name for [`upmix_mono`].
#[inline]
pub fn convert_channels(input: &[AudioSample], target_channels: u16) -> AudioBuffer {
    upmix_mono(input, target_channels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_stereo_averages_channels() {
        let stereo = vec![1.0, 3.0, 2.0, 4.0];
        assert_eq!(downmix_to_mono(&stereo, 2), vec![2.0, 3.0]);
    }

    #[test]
    fn downmix_mono_is_noop() {
        let mono = vec![1.0, 2.0, 3.0];
        assert_eq!(downmix_to_mono(&mono, 1), mono);
    }

    #[test]
    fn downmix_bad_length_returns_empty() {
        assert!(downmix_to_mono(&[1.0, 2.0, 3.0], 2).is_empty());
    }

    #[test]
    fn upmix_duplicates_mono_to_stereo() {
        let mono = vec![1.0, 2.0];
        assert_eq!(upmix_mono(&mono, 2), vec![1.0, 1.0, 2.0, 2.0]);
    }
}
