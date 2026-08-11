//! PCM format conversion for VAD / wake backends that expect int16.

use boris_core::AudioSample;

/// Convert mono `f32` samples in roughly `[-1.0, 1.0]` to little-endian PCM16.
///
/// # Scaling / clamp
///
/// Values outside `[-1.0, 1.0]` are clamped first. Scaling uses
/// `i16::MAX` (`32767`), so the full-scale range is **symmetric ±32767**:
/// - `+1.0` → `32767`
/// - `-1.0` → `-32767` (not `i16::MIN` / `-32768`)
///
/// This avoids the classic asymmetric full-scale map that would send `-1.0` to
/// `-32768` while `+1.0` only reaches `32767`. Empty input → empty output.
#[inline]
pub fn f32_to_pcm16_samples(audio: &[AudioSample]) -> Vec<i16> {
    let mut out = Vec::with_capacity(audio.len());
    f32_to_pcm16_samples_into(audio, &mut out);
    out
}

/// Write mono `f32` → PCM16 into `out`, reusing capacity (hot-path friendly).
///
/// Clears `out` then appends `audio.len()` samples. Same clamp/scale contract as
/// [`f32_to_pcm16_samples`] (symmetric ±32767).
#[inline]
pub fn f32_to_pcm16_samples_into(audio: &[AudioSample], out: &mut Vec<i16>) {
    out.clear();
    out.reserve(audio.len());
    for &s in audio {
        out.push((s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_and_scales_symmetric() {
        let pcm = f32_to_pcm16_samples(&[0.0, 1.0, -1.0, 2.0, -2.0]);
        assert_eq!(pcm[0], 0);
        assert_eq!(pcm[1], i16::MAX);
        assert_eq!(pcm[2], -i16::MAX); // symmetric ±32767, not i16::MIN
        assert_eq!(pcm[3], i16::MAX);
        assert_eq!(pcm[4], -i16::MAX);
    }

    #[test]
    fn empty() {
        assert!(f32_to_pcm16_samples(&[]).is_empty());
    }

    #[test]
    fn into_reuses_buffer() {
        let mut buf = Vec::with_capacity(8);
        f32_to_pcm16_samples_into(&[0.5, -0.5], &mut buf);
        assert_eq!(buf.len(), 2);
        let cap = buf.capacity();
        f32_to_pcm16_samples_into(&[1.0], &mut buf);
        assert_eq!(buf, vec![i16::MAX]);
        assert!(buf.capacity() >= cap);
    }
}
