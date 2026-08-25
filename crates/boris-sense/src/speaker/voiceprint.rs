//! Mean embedding + cosine. Policy cutoffs live in the pipeline.

/// Cosine below this vs the enrolled mean is treated as a different voice.
/// Short wake crops score lower than full VoxCeleb trials; this is the
/// “do not reject the owner at laptop distance” floor, not an EER operating point.
pub const COSINE_REJECT: f32 = 0.20;
/// Enroll takes of the same person saying “Boris” should sit above this.
pub const ENROLL_COSINE_MIN: f32 = 0.28;

#[derive(Clone, Debug)]
pub struct Voiceprint {
    mean: Vec<f32>,
}

impl Voiceprint {
    /// `None` unless there are at least two finite, same-width embeddings.
    pub fn from_embeddings(embs: &[Vec<f32>]) -> Option<Self> {
        if embs.len() < 2 {
            return None;
        }
        let dim = embs[0].len();
        if dim == 0
            || embs
                .iter()
                .any(|e| e.len() != dim || e.iter().any(|x| !x.is_finite()))
        {
            return None;
        }
        let n = embs.len() as f32;
        let mut mean = vec![0.0f32; dim];
        for e in embs {
            for (m, x) in mean.iter_mut().zip(e.iter()) {
                *m += *x / n;
            }
        }
        l2_normalize(&mut mean);
        Some(Self { mean })
    }

    /// Cosine vs an L2-normalized probe. Unmatched dims → 0.
    pub fn cosine(&self, emb: &[f32]) -> f32 {
        if emb.len() != self.mean.len() {
            return 0.0;
        }
        self.mean.iter().zip(emb.iter()).map(|(a, b)| a * b).sum()
    }
}

pub fn l2_normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    for x in v {
        *x /= n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(i: usize, dim: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; dim];
        v[i] = 1.0;
        v
    }

    #[test]
    fn identical_embeddings_cosine_one() {
        let a = vec![0.3, 0.4, 0.0];
        let mut b = a.clone();
        l2_normalize(&mut b);
        let vp = Voiceprint::from_embeddings(&[a.clone(), a]).unwrap();
        assert!((vp.cosine(&b) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn orthogonal_is_near_zero() {
        let vp = Voiceprint::from_embeddings(&[unit(0, 4), unit(0, 4)]).unwrap();
        assert!(vp.cosine(&unit(1, 4)).abs() < 1e-5);
    }

    #[test]
    fn one_take_is_none() {
        assert!(Voiceprint::from_embeddings(&[vec![1.0, 0.0]]).is_none());
    }
}
