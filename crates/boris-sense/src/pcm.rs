use boris_core::AudioSample;

/// Converts a normalized `&[f32]` audio sample (-1.0..1.0) into PCM16 `Vec<i16>`.
///
/// Values outside the range are clamped.
#[inline]
pub fn f32_to_pcm16_samples(audio: &[AudioSample]) -> Vec<i16> {
    audio
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .collect()
}
