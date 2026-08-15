//! STT / TTS load helpers for the sequential turn loop.
//!
//! Models load one step ahead on short helper threads (STT while capturing,
//! TTS while the agent thinks) so the UI never shows "loading model" chrome —
//! only real phases (Hearing / Reading / Thinking / Talking).

use std::thread::{self, JoinHandle};

use boris_core::TurnId;
use boris_inference::{SpeechToText, TextToSpeech};

pub(super) type SttBox = Box<dyn SpeechToText>;
pub(super) type TtsBox = Box<dyn TextToSpeech>;

/// How long voice models stay resident after a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ModelResidency {
    /// Eager eviction (original behavior).
    LowMemory,
    /// Keep models through an active turn/follow-up chain, then release both
    /// when Boris returns to idle. This avoids reload churn during a dialogue
    /// without retaining weights for the whole powered-on session.
    Balanced,
    /// Keep models loaded while the engine/session is active.
    LowLatency,
}

impl ModelResidency {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "low_memory" | "low-memory" => Self::LowMemory,
            "low_latency" | "low-latency" => Self::LowLatency,
            _ => Self::Balanced,
        }
    }

    pub fn should_evict_at_handoff(self) -> bool {
        matches!(self, Self::LowMemory)
    }

    pub fn should_evict_idle(self) -> bool {
        !matches!(self, Self::LowLatency)
    }
}

/// Result lane for one request sent to a reusable model-loader thread.
pub(super) enum ModelLoadJob<T> {
    Pending(std::sync::mpsc::Receiver<(T, Result<(), String>)>),
    /// Worker startup/send failed; `T` remains owned by the engine.
    Ready(T, Result<(), String>),
}

type LoadFn<T> = fn(&mut T) -> Result<(), String>;

enum LoaderCommand<T> {
    Load {
        model: T,
        result: std::sync::mpsc::SyncSender<(T, Result<(), String>)>,
    },
}

/// One reusable OS thread per model kind. Requests still transfer exclusive
/// model ownership, but turns no longer create and tear down loader threads.
pub(super) struct ModelLoader<T: Send + 'static> {
    tx: Option<std::sync::mpsc::SyncSender<LoaderCommand<T>>>,
    worker: Option<JoinHandle<()>>,
    load_inline: LoadFn<T>,
    label: &'static str,
}

impl<T: Send + 'static> ModelLoader<T> {
    fn new(thread_name: &'static str, label: &'static str, load: LoadFn<T>) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<LoaderCommand<T>>(1);
        match thread::Builder::new()
            .name(thread_name.into())
            .spawn(move || {
                while let Ok(LoaderCommand::Load { mut model, result }) = rx.recv() {
                    let started = std::time::Instant::now();
                    let loaded = load(&mut model);
                    if loaded.is_ok() {
                        tracing::info!(
                            model = label,
                            ms = started.elapsed().as_millis() as u64,
                            "model preload ready"
                        );
                    }
                    if result.send((model, loaded)).is_err() {
                        tracing::warn!(model = label, "model load result receiver dropped");
                    }
                }
            }) {
            Ok(worker) => Self {
                tx: Some(tx),
                worker: Some(worker),
                load_inline: load,
                label,
            },
            Err(error) => {
                tracing::error!(%error, model = label, "failed to spawn reusable model loader");
                Self {
                    tx: None,
                    worker: None,
                    load_inline: load,
                    label,
                }
            }
        }
    }

    pub fn load(&self, mut model: T) -> ModelLoadJob<T> {
        let Some(tx) = self.tx.as_ref() else {
            let result = (self.load_inline)(&mut model);
            return ModelLoadJob::Ready(model, result);
        };
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        match tx.send(LoaderCommand::Load {
            model,
            result: result_tx,
        }) {
            Ok(()) => ModelLoadJob::Pending(result_rx),
            Err(std::sync::mpsc::SendError(LoaderCommand::Load { mut model, .. })) => {
                tracing::error!(model = self.label, "reusable model loader disconnected");
                let result = (self.load_inline)(&mut model);
                ModelLoadJob::Ready(model, result)
            }
        }
    }
}

impl<T: Send + 'static> Drop for ModelLoader<T> {
    fn drop(&mut self) {
        // Disconnect wakes an idle worker. The engine joins only during runtime
        // teardown; any active load is allowed to return its model first.
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                tracing::warn!(
                    model = self.label,
                    "model loader thread panicked on shutdown"
                );
            }
        }
    }
}

pub(super) fn create_stt_loader() -> ModelLoader<SttBox> {
    ModelLoader::new("boris-stt-loader", "stt", |stt| {
        stt.load().map_err(|error| error.to_string())
    })
}

pub(super) fn create_tts_loader() -> ModelLoader<TtsBox> {
    ModelLoader::new("boris-tts-loader", "tts", |tts| {
        tts.load().map_err(|error| error.to_string())
    })
}

/// Drop STT + TTS weights from RAM. Safe if already unloaded.
pub(super) fn release_voice_models(
    stt: &mut dyn SpeechToText,
    tts: &mut dyn TextToSpeech,
    reason: &str,
) {
    if let Err(e) = stt.unload() {
        tracing::warn!(error = %e, %reason, "stt unload failed");
    }
    if let Err(e) = tts.unload() {
        tracing::warn!(error = %e, %reason, "tts unload failed");
    }
    tracing::info!(%reason, "STT/TTS released (idle RAM)");
}

pub(super) fn join_stt_load(job: ModelLoadJob<SttBox>) -> (SttBox, Result<(), String>) {
    match job {
        ModelLoadJob::Pending(rx) => rx.recv().unwrap_or_else(|_| {
            tracing::error!("stt loader disconnected before returning model");
            (Box::new(LostStt), Err("stt loader disconnected".into()))
        }),
        ModelLoadJob::Ready(stt, r) => (stt, r),
    }
}

pub(super) fn join_tts_load(job: ModelLoadJob<TtsBox>) -> (TtsBox, Result<(), String>) {
    match job {
        ModelLoadJob::Pending(rx) => rx.recv().unwrap_or_else(|_| {
            tracing::error!("tts loader disconnected before returning model");
            (Box::new(LostTts), Err("tts loader disconnected".into()))
        }),
        ModelLoadJob::Ready(tts, r) => (tts, r),
    }
}

pub(super) fn unload_stt(stt: &mut dyn SpeechToText, turn: TurnId) {
    if let Err(e) = stt.unload() {
        tracing::warn!(error = %e, %turn, "stt unload failed");
    } else {
        tracing::debug!(%turn, "stt unloaded");
    }
}

pub(super) fn unload_tts(tts: &mut dyn TextToSpeech, turn: TurnId) {
    if let Err(e) = tts.unload() {
        tracing::warn!(error = %e, %turn, "tts unload failed");
    } else {
        tracing::debug!(%turn, "tts unloaded");
    }
}

pub(super) fn maybe_unload_stt(
    stt: &mut dyn SpeechToText,
    turn: TurnId,
    residency: ModelResidency,
) {
    if residency.should_evict_at_handoff() {
        unload_stt(stt, turn);
    }
}

pub(super) fn maybe_unload_tts(
    tts: &mut dyn TextToSpeech,
    turn: TurnId,
    residency: ModelResidency,
) {
    if residency.should_evict_at_handoff() {
        unload_tts(tts, turn);
    }
}

pub(super) fn maybe_unload_idle(
    stt: &mut dyn SpeechToText,
    tts: &mut dyn TextToSpeech,
    turn: TurnId,
    residency: ModelResidency,
) {
    if residency.should_evict_idle() {
        unload_stt(stt, turn);
        unload_tts(tts, turn);
    }
}

/// Build the product STT backend (or a null stub when the feature is off).
pub(super) fn create_stt(model_dir: std::path::PathBuf) -> SttBox {
    #[cfg(feature = "stt-parakeet")]
    {
        Box::new(boris_stt_parakeet::ParakeetStt::with_model_dir(model_dir))
    }
    #[cfg(not(feature = "stt-parakeet"))]
    {
        let _ = model_dir;
        Box::new(NullStt)
    }
}

/// Build the product TTS backend (or a null stub when the feature is off).
pub(super) fn create_tts(
    model_dir: std::path::PathBuf,
    voice_dir: std::path::PathBuf,
    voice_id: &str,
) -> TtsBox {
    #[cfg(feature = "tts-supertone")]
    {
        Box::new(boris_tts_supertone::SupertoneTts::with_paths(
            model_dir, voice_dir, voice_id,
        ))
    }
    #[cfg(not(feature = "tts-supertone"))]
    {
        let _ = (model_dir, voice_dir, voice_id);
        Box::new(NullTts)
    }
}

/// Placeholder if the STT handle was lost (load-thread panic or empty slot recovery).
struct LostStt;
impl SpeechToText for LostStt {
    fn transcribe(&mut self, _: &[boris_core::AudioSample]) -> boris_core::Result<String> {
        Err(boris_core::Error::other(
            "STT model unavailable (load-thread panic or slot recovery)",
        ))
    }
}

struct LostTts;
impl TextToSpeech for LostTts {
    fn load(&mut self) -> boris_core::Result<()> {
        Err(boris_core::Error::other(
            "TTS model unavailable (worker detached or panicked)",
        ))
    }

    fn backend_id(&self) -> &str {
        "lost"
    }

    fn synthesize(&mut self, _: &str) -> boris_core::Result<boris_core::AudioBuffer> {
        Err(boris_core::Error::other(
            "TTS model unavailable (load-thread panic)",
        ))
    }
}

pub(super) fn lost_tts() -> TtsBox {
    Box::new(LostTts)
}

// ── Optional null backends when features are off ─────────────────────────────

#[cfg(not(feature = "stt-parakeet"))]
struct NullStt;

#[cfg(not(feature = "stt-parakeet"))]
impl SpeechToText for NullStt {
    fn transcribe(&mut self, _: &[boris_core::AudioSample]) -> boris_core::Result<String> {
        Err(boris_core::Error::other("stt-parakeet feature disabled"))
    }
}

#[cfg(not(feature = "tts-supertone"))]
struct NullTts;

#[cfg(not(feature = "tts-supertone"))]
impl TextToSpeech for NullTts {
    fn backend_id(&self) -> &str {
        "null"
    }

    fn synthesize(&mut self, _: &str) -> boris_core::Result<boris_core::AudioBuffer> {
        Err(boris_core::Error::other("tts-supertone feature disabled"))
    }
}

#[cfg(test)]
mod tests {
    use super::{ModelLoadJob, ModelLoader, ModelResidency};

    #[test]
    fn residency_handoff_and_idle_policy_is_explicit() {
        assert!(ModelResidency::LowMemory.should_evict_at_handoff());
        assert!(!ModelResidency::Balanced.should_evict_at_handoff());
        assert!(!ModelResidency::LowLatency.should_evict_at_handoff());

        assert!(ModelResidency::LowMemory.should_evict_idle());
        assert!(ModelResidency::Balanced.should_evict_idle());
        assert!(!ModelResidency::LowLatency.should_evict_idle());
    }

    #[test]
    fn reusable_loader_handles_multiple_requests_on_one_thread() {
        #[derive(Default)]
        struct Probe {
            loaded_on: Vec<std::thread::ThreadId>,
        }

        fn load(probe: &mut Probe) -> Result<(), String> {
            probe.loaded_on.push(std::thread::current().id());
            Ok(())
        }

        fn join(job: ModelLoadJob<Probe>) -> Probe {
            match job {
                ModelLoadJob::Pending(rx) => rx.recv().expect("loader result").0,
                ModelLoadJob::Ready(model, result) => {
                    result.expect("inline fallback load");
                    model
                }
            }
        }

        let loader = ModelLoader::new("boris-test-loader", "probe", load);
        let probe = join(loader.load(Probe::default()));
        let probe = join(loader.load(probe));
        assert_eq!(probe.loaded_on.len(), 2);
        assert_eq!(probe.loaded_on[0], probe.loaded_on[1]);
        assert_ne!(probe.loaded_on[0], std::thread::current().id());
    }
}
