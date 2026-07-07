pub mod buffer;
pub mod capture;
pub mod playback;
pub mod resampler;

// Re-export from core so existing call sites don't break.
pub use boris_core::AUDIO_TARGET_RATE;

pub const AUDIO_CHUNK_SIZE: u32 = 512;
