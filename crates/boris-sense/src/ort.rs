//! Process-global ONNX Runtime setup for the wake-word path.

use std::sync::OnceLock;

use boris_core::{Error, Result};

/// Guards the actual init logic so [`init_onnx_runtime`] runs it at most once
/// per process, regardless of how many times (or how many threads) call it.
static ORT_INIT: OnceLock<Result<()>> = OnceLock::new();

/// Configure ONNX Runtime **before** any model sessions are created.
///
/// LiveKit wakeword builds 3 ORT sessions (mel / embedding / classifier). Without
/// a process-global pool, each session gets its own multi-core thread pool and
/// idle workers **spin**, which shows up as high Idle CPU and dozens of threads.
///
/// Must be called once at process start, before
/// [`crate::wake::LivekitWakeWord::try_new`].
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
            "ORT environment already configured; wakeword thread-pool settings may not apply"
        );
    } else {
        tracing::info!("ORT: shared 1-thread global pool (spin disabled) for wakeword inference");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_calls_are_idempotent_and_agree() {
        // Whatever the first call resolves to (Ok or Err depends on the ORT
        // build/environment in CI), repeat calls must return the same result
        // without panicking or re-running the init body.
        let first = init_onnx_runtime();
        let second = init_onnx_runtime();
        assert_eq!(first, second);
    }

    #[test]
    fn concurrent_calls_all_agree() {
        let handles: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(init_onnx_runtime))
            .collect();
        let results: Vec<Result<()>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let first = &results[0];
        for r in &results {
            assert_eq!(r, first);
        }
    }
}
