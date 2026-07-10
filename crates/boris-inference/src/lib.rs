pub mod vad;
pub mod wakeword;

use std::time::Duration;

use boris_core::{error::Result, AudioBuffer, AudioSample, AUDIO_TARGET_RATE};

pub const WAKEWORD_THRESHOLD: f32 = 0.5;
pub const WAKEWORD_WINDOW_SIZE: usize = 32_000; // 2 sec audio, 16 kHz
pub const WAKEWORD_PROCESSING_INTERVAL: Duration = Duration::from_millis(80);

pub const VAD_INITIAL_TIMEOUT: Duration = Duration::from_millis(1600);
pub const VAD_SILENCE_WINDOW: Duration = Duration::from_millis(600);

pub const VAD_PROCESSING_INTERVAL: Duration = Duration::from_millis(40);
pub const VAD_WINDOW_SIZE: usize = 160; // 10 ms at 16 kHz (WebRTC frame size)

/// Configure ONNX Runtime **before** any model sessions are created.
///
/// LiveKit wakeword builds 3 ORT sessions (mel / embedding / classifier). Without
/// a process-global pool, each session gets its own multi-core thread pool and
/// idle workers **spin**, which shows up as high Idle CPU and dozens of threads.
///
/// Must be called once at process start, before [`crate::wakeword::LivekitWakeWord::new`].
pub fn init_onnx_runtime() {
    // Cap OpenMP too — some ORT builds ignore session intra-op settings.
    if std::env::var_os("OMP_NUM_THREADS").is_none() {
        // SAFETY: single-threaded init path before any ORT/OpenMP work starts.
        unsafe { std::env::set_var("OMP_NUM_THREADS", "1") };
    }

    let pool = match ort::environment::GlobalThreadPoolOptions::default()
        .with_intra_threads(1)
        .and_then(|p| p.with_inter_threads(1))
        .and_then(|p| p.with_spin_control(false))
    {
        Ok(pool) => pool,
        Err(e) => {
            tracing::warn!(error = %e, "failed to configure ORT global thread pool");
            return;
        }
    };

    // commit() returns false if something else already configured the env.
    if !ort::init()
        .with_name("boris")
        .with_telemetry(false)
        .with_global_thread_pool(pool)
        .commit()
    {
        tracing::warn!(
            "ORT environment already configured; wakeword thread-pool settings may not apply"
        );
    } else {
        tracing::info!("ORT: shared 1-thread global pool (spin disabled) for wakeword inference");
    }
}

pub trait WakeWord: Send {
    fn predict(&mut self, audio: &[AudioSample]) -> Result<f32>;
}

pub trait Vad: Send {
    fn predict(&mut self, audio: &[AudioSample]) -> Result<bool>;
}

pub trait SpeechToText: Send {
    fn load(&mut self) -> Result<()> {
        Ok(())
    }
    fn unload(&mut self) -> Result<()> {
        Ok(())
    }
    fn transcribe(&mut self, audio: &[AudioSample]) -> Result<String>;
}

pub trait TextToSpeech: Send {
    fn load(&mut self) -> Result<()> {
        Ok(())
    }
    fn unload(&mut self) -> Result<()> {
        Ok(())
    }
    fn synthesize(&mut self, text: &str) -> Result<AudioBuffer>;
}

/// Converts a normalized &[f32] audio sample (-1.0..1.0) into PCM16 Vec<i16>.
///
/// Values outside the range are clamped.
#[inline]
pub fn f32_to_pcm16_samples(audio: &[AudioSample]) -> Vec<i16> {
    audio
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .collect()
}

/// Convert a wall duration into a sample count at `sample_rate`.
///
/// Used so VAD / wakeword thresholds are expressed in ms but enforced in
/// **audio time** (samples processed), not `Instant` wall clock.
pub fn duration_to_samples(d: Duration, sample_rate: u32) -> usize {
    let secs = d.as_secs_f64();
    (secs * sample_rate as f64).round() as usize
}

/// Samples of non-speech after speech before endpointing (`VAD_SILENCE_WINDOW`).
pub fn vad_silence_samples() -> usize {
    duration_to_samples(VAD_SILENCE_WINDOW, AUDIO_TARGET_RATE)
}

/// Samples of non-speech before any speech before giving up (`VAD_INITIAL_TIMEOUT`).
pub fn vad_initial_timeout_samples() -> usize {
    duration_to_samples(VAD_INITIAL_TIMEOUT, AUDIO_TARGET_RATE)
}
