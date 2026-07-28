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
