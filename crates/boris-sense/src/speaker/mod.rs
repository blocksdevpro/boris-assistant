//! Live-vs-loudspeaker perception for a wake crop.
//!
//! Computes cheap spectral features and a one-sided **playback** score.
//! Accept / reject policy stays in `boris-pipeline`.

mod acoustics;

pub use acoustics::{
    compute_acoustic_feat, AcousticFeat, AcousticModel, MATCH_Z_REJECT, PLAYBACK_Z_REJECT,
};
