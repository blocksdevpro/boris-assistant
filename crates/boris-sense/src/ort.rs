//! Process-global ONNX Runtime setup for wake-word and Silero VAD.

use std::sync::OnceLock;

use boris_core::{Error, Result};

/// Guards the actual init logic so [`init_onnx_runtime`] runs it at most once
/// per process, regardless of how many times (or how many threads) call it.
static ORT_INIT: OnceLock<Result<()>> = OnceLock::new();

/// Configure ONNX Runtime **before** any model sessions are created.
///
/// LiveKit wakeword builds 3 ORT sessions (mel / embedding / classifier) and
/// Silero VAD builds a fourth. Without a process-global pool, each session
/// gets its own multi-core thread pool and idle workers **spin**, which shows
/// up as high Idle CPU and dozens of threads.
///
/// Must be called once at process start, before constructing
/// [`crate::wake::LivekitWakeWord`] or [`crate::vad::SileroVad`].
///
/// Idempotent and safe to call repeatedly or concurrently from multiple
/// threads: the init body runs at most once per process (via
/// [`std::sync::OnceLock`]); later calls just return a clone of the first
/// call's result. This also removes a prior check-then-set race on the
/// `OMP_NUM_THREADS` env var, since only one call ever reaches that code.
///
/// # Errors
///
/// Returns [`Error`] if the global thread pool cannot be configured. If another
/// component already committed the ORT environment, this returns `Ok(())` after
/// logging a warning (pool settings may not apply).
pub fn init_onnx_runtime() -> Result<()> {
    ORT_INIT.get_or_init(init_onnx_runtime_once).clone()
}

/// One-time init body, run exactly once by [`init_onnx_runtime`] via `ORT_INIT`.
fn init_onnx_runtime_once() -> Result<()> {
    // Cap OpenMP too — some ORT builds ignore session intra-op settings.
    if std::env::var_os("OMP_NUM_THREADS").is_none() {
        // SAFETY: guarded by `OnceLock::get_or_init`, so this body runs at most
        // once per process, before any ORT/OpenMP work starts.
        unsafe { std::env::set_var("OMP_NUM_THREADS", "1") };
    }

    let pool = ort::environment::GlobalThreadPoolOptions::default()
        .with_intra_threads(1)
        .and_then(|p| p.with_inter_threads(1))
        .and_then(|p| p.with_spin_control(false))
        .map_err(|e| {
            tracing::warn!(error = %e, "failed to configure ORT global thread pool");
            Error::other(format!("failed to configure ORT global thread pool: {e}"))
        })?;

    // commit() returns false if something else already configured the env.
    if !ort::init()
        .with_name("boris")
        .with_telemetry(false)
        .with_global_thread_pool(pool)
        .commit()
    {
        tracing::warn!(
            "ORT environment already configured; wake/VAD thread-pool settings may not apply"
        );
    } else {
        tracing::info!("ORT: shared 1-thread global pool (spin disabled) for wake + VAD");
    }
    Ok(())
}

/// Serialize tests that touch the process-global ORT env or build sessions.
///
/// `init()` / `Session::builder` are not safe to overlap across rustc's
/// default parallel test threads — they deadlock on the 1-thread pool.
#[cfg(test)]
pub(crate) fn lock_ort_for_test() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_calls_are_idempotent_and_agree() {
        let _ort = lock_ort_for_test();
        // Whatever the first call resolves to (Ok or Err depends on the ORT
        // build/environment), repeat calls must return the same result
        // without panicking or re-running the init body.
        let first = init_onnx_runtime();
        let second = init_onnx_runtime();
        assert_eq!(first, second);
    }

    #[test]
    fn concurrent_calls_all_agree() {
        let _ort = lock_ort_for_test();
        // Finish process-global `ort::init` on this thread first. Racing
        // `commit()` from eight threads against the 1-thread pool deadlocks
        // (and also deadlocks any Silero test waiting on `lock_ort_for_test`).
        let primed = init_onnx_runtime();
        let handles: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(init_onnx_runtime))
            .collect();
        let results: Vec<Result<()>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let first = &results[0];
        assert_eq!(&primed, first);
        for r in &results {
            assert_eq!(r, first);
        }
    }
}
