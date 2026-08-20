//! Wake liveness policy: reject speaker-played TTS / TV, accept a live mouth.
//!
//! Perception (`AcousticFeat` / `playback_z`) lives in `boris-sense`. This
//! module owns the enroll profile under `~/.boris/speaker/` and the cutoff.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use boris_sense::{
    compute_acoustic_feat, AcousticFeat, AcousticModel, SpeakerEmbedder, Voiceprint, COSINE_REJECT,
    ENROLL_COSINE_MIN, MATCH_Z_REJECT, PLAYBACK_Z_REJECT,
};

/// ~320 ms of Silero speech. Shorter crops are room noise in a 2 s wake window.
const MIN_SPEECH_HOPS: u32 = 10;
/// Enroll takes must sit near each other or the profile becomes “any sound”.
const ENROLL_MATCH_MAX: f32 = 2.6;

use crate::paths;

/// How a wake crop was classified. Unknown = no profile yet → do not block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WakeOrigin {
    Live,
    /// Band-limited vs enroll (Translate / TTS / YouTube out of a speaker).
    Playback {
        z: f32,
    },
    /// Speech, but not the enrolled voice / room (false wake, other talk).
    Mismatch {
        z: f32,
    },
    /// Wake window had no usable speech.
    TooShort,
    /// Gate off or no profile yet → do not block.
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProfileFile {
    takes: Vec<AcousticFeat>,
    #[serde(default)]
    embeddings: Vec<Vec<f32>>,
}

/// In-memory enroll + classify.
pub struct WakeLiveness {
    enabled: bool,
    takes: Vec<AcousticFeat>,
    embeddings: Vec<Vec<f32>>,
    model: Option<AcousticModel>,
    voiceprint: Option<Voiceprint>,
    embedder: Option<SpeakerEmbedder>,
}

impl WakeLiveness {
    pub fn load(enabled: bool, embedder: Option<SpeakerEmbedder>) -> Self {
        let (takes, embeddings) = load_profile();
        let model = AcousticModel::from_takes(&takes);
        let voiceprint = Voiceprint::from_embeddings(&embeddings);
        if embedder.is_some() && voiceprint.is_none() && model.is_some() {
            tracing::info!(
                "wake liveness: taught profile has no embeddings yet — identity check waits for re-teach"
            );
        }
        Self {
            enabled,
            takes,
            embeddings,
            model,
            voiceprint,
            embedder,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn is_ready(&self) -> bool {
        self.model.is_some()
    }

    pub fn take_count(&self) -> usize {
        self.takes.len()
    }

    /// Classify a VAD-cropped wake window.
    pub fn classify(&mut self, pcm: &[f32], speech_hops: u32) -> WakeOrigin {
        if !self.enabled {
            return WakeOrigin::Unknown;
        }
        let Some(model) = self.model.as_ref() else {
            return WakeOrigin::Unknown;
        };
        if speech_hops < MIN_SPEECH_HOPS {
            return WakeOrigin::TooShort;
        }
        let Some(feat) = compute_acoustic_feat(pcm) else {
            return WakeOrigin::TooShort;
        };
        let play = model.playback_z(feat);
        if play >= PLAYBACK_Z_REJECT {
            return WakeOrigin::Playback { z: play };
        }
        if let (Some(embedder), Some(vp)) = (self.embedder.as_mut(), self.voiceprint.as_ref()) {
            match embedder.embed(pcm) {
                Ok(Some(emb)) => {
                    let cos = vp.cosine(&emb);
                    if cos < COSINE_REJECT {
                        tracing::debug!(cosine = cos, "wake identity miss");
                        return WakeOrigin::Mismatch { z: 1.0 - cos };
                    }
                    return WakeOrigin::Live;
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(error = %e, "speaker embed failed — acoustic fallback"),
            }
        }
        if self.embedder.is_some() {
            // CAM++ is loaded. Brightness mismatch is what fails at 15–30 cm,
            // so do not use it as identity — either cosine already ran, or this
            // crop could not be embedded / the profile still needs a re-teach.
            return WakeOrigin::Live;
        }
        let miss = model.mismatch_z(feat);
        if miss >= MATCH_Z_REJECT {
            return WakeOrigin::Mismatch { z: miss };
        }
        WakeOrigin::Live
    }

    /// Record one enroll take from a wake crop. Persists when `target` takes
    /// have been gathered.
    pub fn add_take(
        &mut self,
        pcm: &[f32],
        speech_hops: u32,
        target: u32,
    ) -> Result<EnrollProgress, String> {
        if speech_hops < MIN_SPEECH_HOPS {
            return Err("that wasn’t speech — say Boris toward the mic".into());
        }
        let feat = compute_acoustic_feat(pcm)
            .ok_or_else(|| "need a clearer take — speak closer to the mic".to_string())?;
        let embedding = match self.embedder.as_mut() {
            Some(embedder) => match embedder.embed(pcm) {
                Ok(Some(e)) => Some(e),
                Ok(None) => {
                    return Err("need a longer take — say Boris toward the mic".into());
                }
                Err(e) => {
                    tracing::warn!(error = %e, "speaker embed on enroll take failed");
                    None
                }
            },
            None => None,
        };
        if let Some(ref emb) = embedding {
            if let Some(vp) = Voiceprint::from_embeddings(&self.embeddings) {
                if vp.cosine(emb) < ENROLL_COSINE_MIN {
                    return Err("that didn’t match the other takes — say Boris again".into());
                }
            }
        } else if let Some(model) = AcousticModel::from_takes(&self.takes) {
            let miss = model.mismatch_z(feat);
            if miss >= ENROLL_MATCH_MAX {
                return Err("that didn’t match the other takes — say Boris again".into());
            }
        }
        self.takes.push(feat);
        if let Some(emb) = embedding {
            self.embeddings.push(emb);
        }
        let have = self.takes.len() as u32;
        let want = target.clamp(2, 8);
        if have >= want {
            self.model = AcousticModel::from_takes(&self.takes);
            self.voiceprint = Voiceprint::from_embeddings(&self.embeddings);
            save_profile(&self.takes, &self.embeddings)?;
        }
        Ok(EnrollProgress {
            have,
            want,
            ready: self.model.is_some(),
        })
    }

    pub fn clear(&mut self) {
        self.takes.clear();
        self.embeddings.clear();
        self.model = None;
        self.voiceprint = None;
        let path = profile_path();
        if path.is_file() {
            if let Err(e) = fs::remove_file(&path) {
                tracing::warn!(error = %e, path = %path.display(), "clear wake liveness profile");
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnrollProgress {
    pub have: u32,
    pub want: u32,
    pub ready: bool,
}

/// Snapshot for desktop settings (no engine thread required).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LivenessStatus {
    pub enrolled: bool,
    pub takes: u32,
}

pub fn liveness_status() -> LivenessStatus {
    let (takes, _) = load_profile();
    LivenessStatus {
        enrolled: AcousticModel::from_takes(&takes).is_some(),
        takes: takes.len() as u32,
    }
}

pub fn clear_liveness_profile() {
    let path = profile_path();
    if path.is_file() {
        let _ = fs::remove_file(path);
    }
}

fn profile_path() -> PathBuf {
    paths::speaker_dir().join("live.json")
}

fn load_profile() -> (Vec<AcousticFeat>, Vec<Vec<f32>>) {
    let path = profile_path();
    let Ok(raw) = fs::read_to_string(&path) else {
        return (Vec::new(), Vec::new());
    };
    match serde_json::from_str::<ProfileFile>(&raw) {
        Ok(p) => (p.takes, p.embeddings),
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "wake liveness profile unreadable");
            (Vec::new(), Vec::new())
        }
    }
}

fn save_profile(takes: &[AcousticFeat], embeddings: &[Vec<f32>]) -> Result<(), String> {
    fs::create_dir_all(paths::speaker_dir()).map_err(|e| format!("create speaker dir: {e}"))?;
    let path = profile_path();
    let json = serde_json::to_string_pretty(&ProfileFile {
        takes: takes.to_vec(),
        embeddings: embeddings.to_vec(),
    })
    .map_err(|e| format!("serialize profile: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, secs: f32) -> Vec<f32> {
        let n = (secs * 16_000.0) as usize;
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / 16_000.0).sin() * 0.4)
            .collect()
    }

    fn gate(enabled: bool, takes: Vec<AcousticFeat>, model: Option<AcousticModel>) -> WakeLiveness {
        WakeLiveness {
            enabled,
            takes,
            embeddings: Vec::new(),
            model,
            voiceprint: None,
            embedder: None,
        }
    }

    #[test]
    fn disabled_is_unknown() {
        let mut g = gate(false, Vec::new(), None);
        assert_eq!(g.classify(&tone(220.0, 1.0), 20), WakeOrigin::Unknown);
    }

    #[test]
    fn live_enroll_accepts_similar_tone() {
        let a = compute_acoustic_feat(&tone(200.0, 1.0)).unwrap();
        let b = compute_acoustic_feat(&tone(210.0, 1.0)).unwrap();
        let mut g = gate(true, vec![a, b], AcousticModel::from_takes(&[a, b]));
        assert_eq!(g.classify(&tone(205.0, 1.0), 20), WakeOrigin::Live);
    }

    #[test]
    fn short_crop_is_rejected_when_enrolled() {
        let a = compute_acoustic_feat(&tone(200.0, 1.0)).unwrap();
        let b = compute_acoustic_feat(&tone(210.0, 1.0)).unwrap();
        let mut g = gate(true, vec![a, b], AcousticModel::from_takes(&[a, b]));
        assert_eq!(g.classify(&[], 0), WakeOrigin::TooShort);
    }

    #[test]
    fn dark_clip_is_playback() {
        let bright: Vec<f32> = (0..16_000)
            .map(|i| {
                let t = i as f32 / 16_000.0;
                (2.0 * std::f32::consts::PI * 400.0 * t).sin() * 0.3
                    + (2.0 * std::f32::consts::PI * 3500.0 * t).sin() * 0.25
            })
            .collect();
        let f1 = compute_acoustic_feat(&bright).unwrap();
        let f2 = compute_acoustic_feat(&bright).unwrap();
        let mut g = gate(true, vec![f1, f2], AcousticModel::from_takes(&[f1, f2]));
        match g.classify(&tone(160.0, 1.0), 20) {
            WakeOrigin::Playback { z } => assert!(z >= PLAYBACK_Z_REJECT, "z={z}"),
            other => panic!("expected playback, got {other:?}"),
        }
    }
}
