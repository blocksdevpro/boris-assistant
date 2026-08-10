//! Process-global ONNX Runtime setup for the wake-word path.

use boris_core::{Error, Result};

/// Configure ONNX Runtime **before** any model sessions are created.
///
/// LiveKit wakeword builds 3 ORT sessions (mel / embedding / classifier). Without
/// a process-global pool, each session gets its own multi-core thread pool and
/// idle workers **spin**, which shows up as high Idle CPU and dozens of threads.
///
/// Must be called once at process start, before
/// [`crate::wake::LivekitWakeWord::try_new`].
///
/// # Errors
///
/// Returns [`Error`] if the global thread pool cannot be configured. If another
/// component already committed the ORT environment, this returns `Ok(())` after
/// logging a warning (pool settings may not apply).
pub fn init_onnx_runtime() -> Result<()> {
    // Cap OpenMP too — some ORT builds ignore session intra-op settings.
    if std::env::var_os("OMP_NUM_THREADS").is_none() {
        // SAFETY: single-threaded init path before any ORT/OpenMP work starts.
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
