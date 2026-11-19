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
/// logging a warning (pool settings may not apply). If the ONNX Runtime
/// library itself cannot load (missing `onnxruntime.dll`, or the Visual C++
/// runtime it depends on), `ort` panics; that panic is caught here and
/// returned as an [`Error`] so the host can show it instead of dying silently.
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

    // A library `ort` cannot use must never reach it: see `preflight_dylib`.
    #[cfg(windows)]
    if let Err(problem) = preflight_dylib() {
        tracing::error!(%problem, "ONNX Runtime library check failed");
        return Err(Error::other(describe_load_failure(&problem)));
    }

    // `ort` loads the runtime library lazily on first API use and panics when
    // that fails. Catch it here: this is the first ORT call in the process.
    match std::panic::catch_unwind(configure_ort_environment) {
        Ok(result) => result,
        Err(payload) => {
            let detail = boris_core::panic_message(payload.as_ref()).unwrap_or("unknown panic");
            tracing::error!(%detail, "ONNX Runtime failed to load");
            Err(Error::other(describe_load_failure(detail)))
        }
    }
}

fn configure_ort_environment() -> Result<()> {
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

/// Check the ONNX Runtime library before `ort` loads it.
///
/// With `ort/load-dynamic` (on in the product through `st-tts`), `ort`
/// 2.0.0-rc.12 deadlocks instead of failing when the library will not load,
/// is too old, or is not ONNX Runtime: it builds its error with an ORT call,
/// which re-enters the `std::sync::Once` it is initializing and blocks the
/// thread forever (no panic, no log). Windows 11 also ships an old ONNX
/// Runtime in System32 that a bare-name search can find. Running the same
/// checks here first turns every one of those cases into an error the host
/// can show. Resolves the path exactly as `ort` does. When `ort` links the
/// library at load time instead, this only finds the module already loaded.
///
/// Returns a detail starting with [`LOAD_FAILURE_MARKER`] on failure.
#[cfg(windows)]
fn preflight_dylib() -> std::result::Result<(), String> {
    use std::ffi::{c_char, CStr};

    type GetApiBase = unsafe extern "system" fn() -> *const ApiBase;
    /// Prefix of `OrtApiBase` (stable C ABI since ONNX Runtime 1.0).
    #[repr(C)]
    struct ApiBase {
        _get_api: unsafe extern "system" fn(u32) -> *const std::ffi::c_void,
        get_version_string: unsafe extern "system" fn() -> *const c_char,
    }

    let path = dylib_path();
    let fail = |why: String| format!("{LOAD_FAILURE_MARKER}: `{}`: {why}", path.display());
    // SAFETY: this is the library `ort` loads next anyway; loading it runs
    // only its own initialization. Symbols are used while `lib` is alive.
    unsafe {
        let lib = libloading::Library::new(&path).map_err(|e| fail(e.to_string()))?;
        let get_base = lib
            .get::<GetApiBase>(b"OrtGetApiBase")
            .map_err(|_| fail("not an ONNX Runtime library (no OrtGetApiBase)".into()))?;
        let base = get_base();
        if base.is_null() {
            return Err(fail("OrtGetApiBase returned null".into()));
        }
        let version = CStr::from_ptr(((*base).get_version_string)())
            .to_string_lossy()
            .into_owned();
        check_version(&version).map_err(fail)?;
        tracing::info!(path = %path.display(), %version, "ONNX Runtime library checked");
    }
    Ok(())
}

/// The library path `ort` will load: `ORT_DYLIB_PATH`, else
/// `onnxruntime.dll`; a relative path is taken beside the exe when present,
/// else left to the Windows DLL search.
#[cfg(windows)]
fn dylib_path() -> std::path::PathBuf {
    let path: std::path::PathBuf = match std::env::var("ORT_DYLIB_PATH") {
        Ok(s) if !s.is_empty() => s.into(),
        _ => "onnxruntime.dll".into(),
    };
    if path.is_absolute() {
        return path;
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join(&path)))
        .filter(|beside_exe| beside_exe.exists())
        .unwrap_or(path)
}

/// `version` (e.g. `1.24.20260218.6`) is new enough for this `ort` build.
#[cfg_attr(not(windows), allow(dead_code))]
fn check_version(version: &str) -> std::result::Result<(), String> {
    let minor = version
        .split('.')
        .nth(1)
        .and_then(|m| m.parse::<u32>().ok())
        .unwrap_or(0);
    if minor < ort::MINOR_VERSION {
        return Err(format!(
            "ONNX Runtime {version} is too old (Boris needs 1.{}.x or newer)",
            ort::MINOR_VERSION
        ));
    }
    Ok(())
}

/// Start of every library-load failure detail (also `ort`'s own wording).
const LOAD_FAILURE_MARKER: &str = "Failed to load ONNX Runtime dylib";

/// What the user can do when `onnxruntime.dll` (or the VC++ runtime it
/// imports) fails to load.
const LOAD_FAILURE_FIX: &str =
    "Reinstall Boris, or install the Microsoft Visual C++ Redistributable (x64) and restart Boris.";

/// User-facing reason for an ORT load failure, keeping the raw detail.
fn describe_load_failure(detail: &str) -> String {
    if detail.contains(LOAD_FAILURE_MARKER) {
        format!(
            "Boris's speech engine (onnxruntime.dll) could not load. {LOAD_FAILURE_FIX} ({detail})"
        )
    } else {
        format!("ONNX Runtime failed to start: {detail}")
    }
}

/// Serialize tests that touch the process-global ORT env or build sessions.
///
/// `init()` / `Session::builder` are not safe to overlap across rustc's
/// default parallel test threads — they deadlock on the 1-thread pool.
#[cfg(test)]
pub(crate) fn lock_ort_for_test() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    pin_test_dylib();
    guard
}

/// Test builds that unify `ort/load-dynamic` (running this crate's tests in
/// one `cargo test` with `boris-desktop`, via `st-tts`) load
/// `onnxruntime.dll` by name from `target/<profile>/deps`, which has none.
/// Windows then falls back to its own ONNX Runtime 1.17 in System32, and
/// init hangs. Point them at the copy `ort` places in `target/<profile>`.
/// No effect on builds that link ORT statically.
#[cfg(test)]
fn pin_test_dylib() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if std::env::var_os("ORT_DYLIB_PATH").is_some() {
            return;
        }
        let dll = std::env::current_exe().ok().and_then(|exe| {
            Some(exe.parent()?.parent()?.join(if cfg!(windows) {
                "onnxruntime.dll"
            } else {
                "libonnxruntime.so"
            }))
        });
        if let Some(dll) = dll.filter(|p| p.is_file()) {
            // SAFETY: runs once, under the ORT test lock, before any ORT call
            // in this process; nothing else in the tests writes the env.
            unsafe { std::env::set_var("ORT_DYLIB_PATH", dll) };
        }
    });
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
    fn old_onnx_runtime_is_rejected_before_ort_sees_it() {
        // Windows 11's System32 copy.
        let err = check_version("1.17.260914-0959.1.os-germanium.2f9444f").unwrap_err();
        assert!(err.contains("1.17") && err.contains("too old"), "{err}");
        assert!(check_version(&format!("1.{}.20260218.6", ort::MINOR_VERSION)).is_ok());
        assert!(check_version(&format!("1.{}.0", ort::MINOR_VERSION + 1)).is_ok());
        assert!(check_version("garbage").is_err());
    }

    #[test]
    fn dylib_load_failure_names_the_fix() {
        let msg = describe_load_failure(
            "Failed to load ONNX Runtime dylib: failed to load from `onnxruntime.dll`",
        );
        assert!(msg.contains("Visual C++ Redistributable"));
        assert!(msg.contains("onnxruntime.dll"));
        assert!(
            !msg.contains("  "),
            "no stray indentation in user text: {msg}"
        );
        let other = describe_load_failure("boom");
        assert!(other.contains("boom"));
        assert!(!other.contains("Visual C++"));
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
