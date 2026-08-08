//! PCM format conversion for VAD / wake backends that expect int16.

use boris_core::AudioSample;

/// Convert mono `f32` samples in roughly `[-1.0, 1.0]` to little-endian PCM16.
///
/// Values outside the range are clamped. Empty input → empty output.
#[inline]
pub fn f32_to_pcm16_samples(audio: &[AudioSample]) -> Vec<i16> {
    audio
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_and_scales() {
        let pcm = f32_to_pcm16_samples(&[0.0, 1.0, -1.0, 2.0, -2.0]);
        assert_eq!(pcm[0], 0);
        assert_eq!(pcm[1], i16::MAX);
        assert_eq!(pcm[2], -i16::MAX); // clamp(-1)*MAX
        assert_eq!(pcm[3], i16::MAX);
        assert_eq!(pcm[4], -i16::MAX);
    }

    #[test]
    fn empty() {
        assert!(f32_to_pcm16_samples(&[]).is_empty());
    }
}
