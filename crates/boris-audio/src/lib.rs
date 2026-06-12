pub mod buffer;
pub mod capture;
pub mod pipeline;
pub mod resampler;

pub const AUDIO_CHUNK_SIZE: u32 = 1024;
pub const AUDIO_TARGET_RATE: u32 = 16_000;
