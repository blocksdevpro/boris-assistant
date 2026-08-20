//! WeSpeaker-style 80-dim log-Mel fbank (Kaldi / torchaudio.compliance.kaldi).
//!
//! Test-time recipe used with CAM++: int16-scale PCM, 25 ms / 10 ms, povey
//! window, 80 bins, log, then mean subtract over time.

use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

use boris_core::AUDIO_TARGET_RATE;

const WIN: usize = 400; // 25 ms @ 16 kHz
const HOP: usize = 160; // 10 ms
const N_FFT: usize = 512;
const N_BINS: usize = N_FFT / 2 + 1;
const N_MELS: usize = 80;
const PREEMPH: f32 = 0.97;
const LOW_HZ: f32 = 20.0;
const HIGH_HZ: f32 = AUDIO_TARGET_RATE as f32 / 2.0;
const INT16: f32 = 32768.0;

/// `[T, 80]` row-major frames. `None` if the crop is shorter than one window.
pub fn log_mel_fbank(pcm: &[f32]) -> Option<Vec<f32>> {
    if pcm.len() < WIN {
        return None;
    }
    let n_frames = 1 + (pcm.len() - WIN) / HOP;
    if n_frames == 0 {
        return None;
    }

    let window = povey_window();
    let filters = mel_filters();
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N_FFT);

    let mut pre = vec![0.0f32; pcm.len()];
    pre[0] = pcm[0] * INT16;
    for i in 1..pcm.len() {
        pre[i] = pcm[i] * INT16 - PREEMPH * pcm[i - 1] * INT16;
    }

    let mut frames = vec![0.0f32; n_frames * N_MELS];
    let mut buf = vec![Complex::new(0.0, 0.0); N_FFT];
    let mut power = [0.0f32; N_BINS];

    for t in 0..n_frames {
        let start = t * HOP;
        buf.fill(Complex::new(0.0, 0.0));
        for k in 0..WIN {
            buf[k] = Complex::new(pre[start + k] * window[k], 0.0);
        }
        fft.process(&mut buf);
        for (k, p) in power.iter_mut().enumerate() {
            *p = buf[k].norm_sqr();
        }
        let dest = t * N_MELS;
        for m in 0..N_MELS {
            let mut e = 0.0f32;
            for k in 0..N_BINS {
                e += power[k] * filters[m][k];
            }
            frames[dest + m] = e.max(1.0).ln();
        }
    }

    // CMN over time, per coefficient.
    for m in 0..N_MELS {
        let mut sum = 0.0f32;
        for t in 0..n_frames {
            sum += frames[t * N_MELS + m];
        }
        let mean = sum / n_frames as f32;
        for t in 0..n_frames {
            frames[t * N_MELS + m] -= mean;
        }
    }

    Some(frames)
}

pub fn n_frames(pcm_len: usize) -> usize {
    if pcm_len < WIN {
        0
    } else {
        1 + (pcm_len - WIN) / HOP
    }
}

pub const MEL_BINS: usize = N_MELS;

fn povey_window() -> [f32; WIN] {
    let mut w = [0.0f32; WIN];
    let den = (WIN - 1) as f32;
    for (i, slot) in w.iter_mut().enumerate() {
        let hamming = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / den).cos();
        *slot = hamming.powf(0.85);
    }
    w
}

fn hz_to_mel(hz: f32) -> f32 {
    1127.0 * (1.0 + hz / 700.0).ln()
}

fn mel_to_hz(mel: f32) -> f32 {
    700.0 * ((mel / 1127.0).exp() - 1.0)
}

fn mel_filters() -> Vec<[f32; N_BINS]> {
    let mel_low = hz_to_mel(LOW_HZ);
    let mel_high = hz_to_mel(HIGH_HZ);
    let mut points = [0.0f32; N_MELS + 2];
    let step = (mel_high - mel_low) / (N_MELS + 1) as f32;
    for (i, p) in points.iter_mut().enumerate() {
        *p = mel_to_hz(mel_low + step * i as f32);
    }
    let bin_hz = AUDIO_TARGET_RATE as f32 / N_FFT as f32;
    let mut filters = vec![[0.0f32; N_BINS]; N_MELS];
    for m in 0..N_MELS {
        let left = points[m];
        let center = points[m + 1];
        let right = points[m + 2];
        for k in 0..N_BINS {
            let hz = k as f32 * bin_hz;
            filters[m][k] = if hz < left || hz > right {
                0.0
            } else if hz <= center {
                (hz - left) / (center - left).max(1e-6)
            } else {
                (right - hz) / (right - center).max(1e-6)
            };
        }
    }
    filters
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_clip_is_none() {
        assert!(log_mel_fbank(&[0.1; 100]).is_none());
    }

    #[test]
    fn one_second_has_expected_frames() {
        let pcm = vec![0.01f32; AUDIO_TARGET_RATE as usize];
        let feat = log_mel_fbank(&pcm).expect("fbank");
        let t = n_frames(pcm.len());
        assert_eq!(t, 98);
        assert_eq!(feat.len(), t * N_MELS);
        assert!(feat.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn cmn_zeros_mean_per_bin() {
        let pcm: Vec<f32> = (0..AUDIO_TARGET_RATE)
            .map(|i| ((i as f32) * 0.02).sin() * 0.1)
            .collect();
        let feat = log_mel_fbank(&pcm).unwrap();
        let t = feat.len() / N_MELS;
        for m in 0..N_MELS {
            let mut sum = 0.0f32;
            for i in 0..t {
                sum += feat[i * N_MELS + m];
            }
            assert!(sum.abs() < 1e-3, "bin {m} mean {sum} after CMN");
        }
    }
}
