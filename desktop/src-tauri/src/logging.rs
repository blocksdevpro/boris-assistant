//! Dual-sink tracing for the desktop host.
//!
//! # Responsibility
//!
//! Host-only observability: file rotation under `~/.boris/logs`, optional stderr
//! in debug builds, panic capture, and the `frontend_log` / `get_log_path` IPC
//! surfaces so the webview can share the same log stream.
//!
//! Voice/engine diagnostics are emitted by `boris_pipeline` and sibling crates;
//! this module only **configures the subscriber** and bootstraps environment dumps.

use std::path::PathBuf;
use std::sync::OnceLock;

use boris_pipeline::{ensure_logs_dir, logs_dir};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Keeps the non-blocking file writer alive for the process lifetime.
static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

/// Active log file path hint (for UI / `get_log_path`).
static LOG_FILE_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Build the env filter: prefer `RUST_LOG`, else `BORIS_LOG`, else packaged defaults.
fn env_filter_from_vars() -> EnvFilter {
    let raw = std::env::var("RUST_LOG")
        .or_else(|_| std::env::var("BORIS_LOG"))
        .ok();

    if let Some(spec) = raw {
        match EnvFilter::try_new(spec) {
            Ok(filter) => return filter,
            Err(e) => {
                eprintln!("invalid RUST_LOG/BORIS_LOG filter ({e}); using defaults");
            }
        }
    }

    // Packaged installs: DEBUG for every Boris crate so `~/.boris/logs` has
    // enough detail without setting env vars.
    // Quieter: RUST_LOG=warn  |  noisier: RUST_LOG=trace
    EnvFilter::new(
        "warn,\
         boris_desktop=debug,\
         boris_desktop_lib=debug,\
         boris_pipeline=debug,\
         boris_audio=debug,\
         boris_sense=debug,\
         boris_agent=debug,\
         boris_stt_parakeet=debug,\
         boris_tts_supertone=debug,\
         boris_tts_kokoro=debug,\
         boris_core=debug,\
         boris_inference=debug",
    )
}

/// Dual sink: daily-rotated file under `~/.boris/logs` + stdout in debug builds.
///
/// Release Windows builds use `windows_subsystem = "windows"` (no console), so
/// the file is the only place to debug installed / packaged apps.
pub fn init_tracing() {
    if let Err(e) = ensure_logs_dir() {
        eprintln!("could not create log dir: {e}");
    }

    let log_dir = logs_dir();
    let file_appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("boris-desktop")
        .filename_suffix("log")
        .build(&log_dir)
        .unwrap_or_else(|e| {
            eprintln!("log file appender failed ({e}); falling back to stdout-only");
            // Write next to CWD if home is broken so try_init path stays uniform.
            tracing_appender::rolling::Builder::new()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix("boris-desktop")
                .filename_suffix("log")
                .build(".")
                .expect("cwd log appender")
        });

    // Best-effort path hint (rolling names include date; point at the directory).
    let _ = LOG_FILE_PATH.set(log_dir.join("boris-desktop.log"));

    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    let _ = LOG_GUARD.set(guard);

    let filter = env_filter_from_vars();

    let file_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_file(true)
        .with_line_number(true);

    // Debug / `cargo tauri dev`: also print to the terminal.
    // Release packaged app: file only (no console window on Windows).
    let result = if cfg!(debug_assertions) {
        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .with(
                fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_ansi(true)
                    .with_target(true),
            )
            .try_init()
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .try_init()
    };

    if let Err(e) = result {
        eprintln!("tracing init failed (already set?): {e}");
    }

    install_panic_hook();

    tracing::info!(
        log_dir = %logs_dir().display(),
        debug_assertions = cfg!(debug_assertions),
        "Boris desktop logging initialized (pipeline crates default to DEBUG)"
    );

    // Dump paths / models / audio / sidecar DLLs immediately so a crash on
    // first Start still leaves a useful file on the other PC.
    boris_pipeline::log_environment("app_boot");
}

/// Capture panics into the same log stream as normal tracing.
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".into());
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".into()
        };
        tracing::error!(%location, %payload, "PANIC");
        prev(info);
    }));
}

/// Path hint for the log directory / active file (for UI debug copy).
pub fn log_path_hint() -> String {
    LOG_FILE_PATH
        .get()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| logs_dir().join("boris-desktop.log").display().to_string())
}

/// Accept frontend / webview log lines into the same file as Rust.
///
/// Levels: `error` | `warn` | `info` | `debug` (anything else → info).
pub fn write_frontend_log(level: &str, message: &str, context: Option<&str>) {
    let ctx = context.unwrap_or("");
    match level.to_ascii_lowercase().as_str() {
        "error" => tracing::error!(target: "boris_desktop::frontend", %message, %ctx, "ui"),
        "warn" | "warning" => {
            tracing::warn!(target: "boris_desktop::frontend", %message, %ctx, "ui")
        }
        "debug" | "trace" => {
            tracing::debug!(target: "boris_desktop::frontend", %message, %ctx, "ui")
        }
        _ => tracing::info!(target: "boris_desktop::frontend", %message, %ctx, "ui"),
    }
}
