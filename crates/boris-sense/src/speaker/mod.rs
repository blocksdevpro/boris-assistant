//! Live-vs-loudspeaker acoustics plus optional CAM++ speaker embeddings.
//!
//! Playback (TV / TTS out of a speaker) still uses cheap spectral cues.
//! Identity (“is this the taught voice”) uses cosine vs enrolled embeddings
//! when a [`SpeakerEmbedder`] is loaded. Policy stays in `boris-pipeline`.

mod acoustics;
mod embed;
mod fbank;
mod voiceprint;

pub use acoustics::{
    compute_acoustic_feat, AcousticFeat, AcousticModel, MATCH_Z_REJECT, PLAYBACK_Z_REJECT,
};
pub use embed::SpeakerEmbedder;
pub use voiceprint::{Voiceprint, COSINE_REJECT, ENROLL_COSINE_MIN};
