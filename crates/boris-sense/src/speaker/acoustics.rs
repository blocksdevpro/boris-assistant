//! Spectral features that tell a close-talk mouth from a laptop speaker.
//!
//! Not an anti-spoof network. Speaker-played Translate/TTS is band-limited
//! relative to the enrolled live mic. Policy (the z cutoff) lives in the
//! pipeline; this module only measures.

use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use serde::{Deserialize, Serialize};

use boris_core::AUDIO_TARGET_RATE;

const FFT: usize = 512;
const HOP: usize = 256;

/// Suggested reject floor for [`AcousticModel::playback_z`]. Pipeline may use
/// a different value; this is the lab-tuned default.
pub const PLAYBACK_Z_REJECT: f32 = 3.6;
/// Suggested two-sided mismatch floor vs enrolled takes.
pub const MATCH_Z_REJECT: f32 = 3.2;

/// Per-clip close-talk / playback cues. Stored in the enroll profile.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct AcousticFeat {
    /// Spectral centroid in Hz.
    pub centroid_hz: f32,
    /// Energy 4–8 kHz / energy 0.3–4 kHz.
    pub hf_ratio: f32,
    pub flatness: f32,
    pub crest: f32,
    pub zcr: f32,
}

impl AcousticFeat {
    fn as_vec(self) -> [f32; 5] {
        [
            self.centroid_hz,
            self.hf_ratio,
            self.flatness,
            self.crest,
            self.zcr,
        ]
    }
}

/// Wide floors so leaning back / talking quieter is not a 5σ outlier.
const MIN_STD: [f32; 5] = [350.0, 0.12, 0.06, 0.50, 0.025];

/// Fitted from two or more enroll takes of live speech on this mic.
#[derive(Clone, Debug)]
pub struct AcousticModel {
    mean: [f32; 5],
    std: [f32; 5],
}

impl AcousticModel {
    /// `None` unless there are at least two takes.
    pub fn from_takes(takes: &[AcousticFeat]) -> Option<Self> {
        if takes.len() < 2 {
            return None;
        }
        let n = takes.len() as f32;
        let mut mean = [0.0f32; 5];
        for t in takes {
            for (m, x) in mean.iter_mut().zip(t.as_vec()) {
                *m += x / n;
            }
        }
        let mut var = [0.0f32; 5];
        for t in takes {
            for (i, x) in t.as_vec().into_iter().enumerate() {
                let d = x - mean[i];
                var[i] += d * d / n;
            }
        }
        let mut std = [0.0f32; 5];
        for i in 0..5 {
            std[i] = var[i].sqrt().max(MIN_STD[i]);
        }
        Some(Self { mean, std })
    }

    /// How much *darker / more band-limited* than enroll.
    ///
    /// One-sided on centroid + HF only. Brighter or closer than enroll is 0 —
    /// that is leaning in, not Translate through a speaker.
    pub fn playback_z(&self, feat: AcousticFeat) -> f32 {
        let v = feat.as_vec();
        let darker = ((self.mean[0] - v[0]) / self.std[0]).max(0.0);
        let duller = ((self.mean[1] - v[1]) / self.std[1]).max(0.0);
        (darker * darker + duller * duller).sqrt()
    }

    /// Two-sided distance on centroid + HF. Random room noise and other
    /// talk sit far from enrolled “Boris” takes; leaning in does not.
    pub fn mismatch_z(&self, feat: AcousticFeat) -> f32 {
        let v = feat.as_vec();
        let dc = (v[0] - self.mean[0]) / self.std[0];
        let dh = (v[1] - self.mean[1]) / self.std[1];
        (dc * dc + dh * dh).sqrt()
    }
}

/// Feature vector for a 16 kHz mono crop. `None` if too short or silent.
pub fn compute_acoustic_feat(pcm: &[f32]) -> Option<AcousticFeat> {
    if pcm.len() < FFT {
        return None;
    }

    let mut rms = 0.0f32;
    let mut peak = 0.0f32;
    let mut zc = 0u32;
    for (i, &x) in pcm.iter().enumerate() {
        rms += x * x;
        peak = peak.max(x.abs());
        if i > 0 && (pcm[i - 1] >= 0.0) != (x >= 0.0) {
            zc += 1;
        }
    }
    rms = (rms / pcm.len() as f32).sqrt();
    if rms < 1e-6 {
        return None;
    }
    let crest = peak / rms;
    let zcr = zc as f32 / pcm.len() as f32;

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT);
    let mut spec = vec![0.0f32; FFT / 2];
    let mut frames = 0u32;
    let mut buf = vec![Complex::new(0.0, 0.0); FFT];
    let mut i = 0;
    while i + FFT <= pcm.len() {
        for k in 0..FFT {
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / (FFT - 1) as f32).cos();
            buf[k] = Complex::new(pcm[i + k] * w, 0.0);
        }
        fft.process(&mut buf);
        for k in 0..FFT / 2 {
            spec[k] += buf[k].norm();
        }
        frames += 1;
        i += HOP;
    }
    if frames == 0 {
        return None;
    }
    let inv = 1.0 / frames as f32;
    for s in &mut spec {
        *s *= inv;
    }

    let bin_hz = AUDIO_TARGET_RATE as f32 / FFT as f32;
    let mut num = 0.0f32;
    let mut den = 0.0f32;
    let mut hf = 0.0f32;
    let mut lf = 0.0f32;
    let mut log_sum = 0.0f32;
    let mut mag_sum = 0.0f32;
    let mut bins = 0u32;
    for (k, &mag) in spec.iter().enumerate().skip(1) {
        let f = k as f32 * bin_hz;
        let m = mag.max(1e-12);
        num += f * m;
        den += m;
        if (300.0..4_000.0).contains(&f) {
            lf += m;
        } else if (4_000.0..7_900.0).contains(&f) {
            hf += m;
        }
        log_sum += m.ln();
        mag_sum += m;
        bins += 1;
    }
    if den < 1e-12 || lf < 1e-12 || bins == 0 {
        return None;
    }
    Some(AcousticFeat {
        centroid_hz: num / den,
        hf_ratio: hf / lf,
        flatness: (log_sum / bins as f32).exp() / (mag_sum / bins as f32),
        crest,
        zcr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, secs: f32) -> Vec<f32> {
        let n = (secs * AUDIO_TARGET_RATE as f32) as usize;
        (0..n)
            .map(|i| {
                (2.0 * std::f32::consts::PI * freq * i as f32 / AUDIO_TARGET_RATE as f32).sin()
                    * 0.4
            })
            .collect()
    }

    #[test]
    fn tone_has_low_hf() {
        let f = compute_acoustic_feat(&tone(220.0, 1.0)).unwrap();
        assert!(f.hf_ratio < 0.3, "hf={}", f.hf_ratio);
        assert!(f.centroid_hz < 1500.0, "c={}", f.centroid_hz);
    }

    #[test]
    fn same_takes_low_playback_z() {
        let a = compute_acoustic_feat(&tone(180.0, 1.0)).unwrap();
        let b = compute_acoustic_feat(&tone(190.0, 1.0)).unwrap();
        let c = compute_acoustic_feat(&tone(200.0, 1.0)).unwrap();
        let m = AcousticModel::from_takes(&[a, b, c]).unwrap();
        assert!(m.playback_z(a) < 2.0, "z={}", m.playback_z(a));
    }

    #[test]
    fn darker_clip_raises_playback_z() {
        let bright: Vec<f32> = (0..16_000)
            .map(|i| {
                let t = i as f32 / AUDIO_TARGET_RATE as f32;
                (2.0 * std::f32::consts::PI * 400.0 * t).sin() * 0.3
                    + (2.0 * std::f32::consts::PI * 3500.0 * t).sin() * 0.2
            })
            .collect();
        let dark = tone(180.0, 1.0);
        let b1 = compute_acoustic_feat(&bright).unwrap();
        let b2 = compute_acoustic_feat(&bright).unwrap();
        let m = AcousticModel::from_takes(&[b1, b2]).unwrap();
        let z_same = m.playback_z(b1);
        let z_dark = m.playback_z(compute_acoustic_feat(&dark).unwrap());
        assert!(z_dark > z_same + 0.5, "same={z_same} dark={z_dark}");
        assert!(m.mismatch_z(compute_acoustic_feat(&dark).unwrap()) > 1.0);
    }
}
