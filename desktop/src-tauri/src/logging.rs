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
    // Install the crash-diagnostic panic hook FIRST, unconditionally, before
    // any fallible file I/O below. Packaged Windows builds have no console
    // (`windows_subsystem = "windows"`), so if log-appender construction
    // itself were to panic (or the process panics for any other reason
    // before a subscriber is live), this hook — not file logging — is the
    // only thing standing between us and a completely silent crash.
    install_panic_hook();

    if let Err(e) = ensure_logs_dir() {
        eprintln!("could not create log dir: {e}");
    }

    let log_dir = logs_dir();

    // Best-effort path hint (rolling names include date; point at the directory).
    let _ = LOG_FILE_PATH.set(log_dir.join("boris-desktop.log"));

    let primary = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("boris-desktop")
        .filename_suffix("log")
        .build(&log_dir);

    // If the home-dir appender fails, try next to CWD. If *that* also fails
    // (e.g. a fully read-only/permission-restricted environment), fall back
    // to a stdout-only subscriber instead of `.expect()`-panicking — a
    // subscriber init can't practically fail the way file I/O can, so this
    // path is effectively infallible and always leaves *some* diagnostics.
    let file_appender = primary.or_else(|e| {
        eprintln!("log file appender failed ({e}); falling back to cwd");
        tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("boris-desktop")
            .filename_suffix("log")
            .build(".")
            .inspect_err(|e2| {
                eprintln!(
                    "cwd log appender failed too ({e2}); falling back to stdout-only logging"
                );
            })
    });

    let filter = env_filter_from_vars();

    let result = match file_appender {
        Ok(appender) => {
            let (non_blocking, guard) = tracing_appender::non_blocking(appender);
            let _ = LOG_GUARD.set(guard);

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
            if cfg!(debug_assertions) {
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
            }
        }
        Err(_) => {
            // Both file sinks are unavailable. stdout still shows up under
            // `cargo tauri dev` / a console-attached run, and even in a
            // packaged no-console build this keeps the process from being
            // completely unobservable beyond the panic hook.
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    fmt::layer()
                        .with_writer(std::io::stdout)
                        .with_ansi(false)
                        .with_target(true),
                )
                .try_init()
        }
    };

    if let Err(e) = result {
        eprintln!("tracing init failed (already set?): {e}");
    }

    tracing::info!(
        log_dir = %logs_dir().display(),
        debug_assertions = cfg!(debug_assertions),
        "Boris desktop logging initialized (pipeline crates default to DEBUG)"
    );

    let pruned = prune_old_logs(&log_dir);
    if pruned > 0 {
        tracing::info!(pruned, log_dir = %log_dir.display(), "pruned old log files");
    } else {
        tracing::debug!(log_dir = %log_dir.display(), "log prune: nothing to remove");
    }

    // Dump paths / models / audio / sidecar DLLs so a crash on first Start
    // still leaves a useful file. Run off this thread: device enum + dir walks
    // can take hundreds of ms and previously delayed the first paint / made
    // the window look frozen at launch.
    std::thread::Builder::new()
        .name("boris-boot-diag".into())
        .spawn(|| {
            boris_pipeline::log_environment("app_boot");
        })
        .map(|_handle| ())
        .unwrap_or_else(|e| {
            // Fallback: still dump on this thread if we cannot spawn.
            eprintln!("boot diagnostics thread spawn failed ({e}); running inline");
            boris_pipeline::log_environment("app_boot");
        });
}

/// Delete old `boris-desktop.*.log` files past a retention window or count
/// cap so a long-running install doesn't accumulate one file per day forever.
///
/// Best-effort: never called before a fallback diagnostic path exists, and
/// never allowed to fail startup — a directory-read failure or an individual
/// delete failure is logged and otherwise ignored. Returns the number of
/// files removed.
const LOG_RETENTION_DAYS: u64 = 14;
const LOG_RETENTION_MAX_FILES: usize = 30;

fn prune_old_logs(log_dir: &std::path::Path) -> usize {
    let entries = match std::fs::read_dir(log_dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::debug!(error = %e, dir = %log_dir.display(), "log prune: could not read log dir");
            return 0;
        }
    };

    let mut files: Vec<(PathBuf, std::time::SystemTime)> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("boris-desktop.") && n.ends_with(".log"))
        })
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((entry.path(), modified))
        })
        .collect();

    // Newest first: index 0 is the current/most-recent file.
    files.sort_by_key(|entry| std::cmp::Reverse(entry.1));

    let now = std::time::SystemTime::now();
    let max_age = std::time::Duration::from_secs(LOG_RETENTION_DAYS * 24 * 60 * 60);

    let mut pruned = 0usize;
    for (idx, (path, modified)) in files.iter().enumerate() {
        let too_old = now
            .duration_since(*modified)
            .map(|age| age > max_age)
            .unwrap_or(false);
        let beyond_cap = idx >= LOG_RETENTION_MAX_FILES;

        if !(too_old || beyond_cap) {
            continue;
        }

        match std::fs::remove_file(path) {
            Ok(()) => pruned += 1,
            Err(e) => {
                tracing::debug!(error = %e, path = %path.display(), "log prune: failed to remove file");
            }
        }
    }

    pruned
}

/// Capture panics into the same log stream as normal tracing.
///
/// Installed before any fallible logging setup (see [`init_tracing`]), so it
/// must not itself depend on a tracing subscriber being live to be useful.
/// It always does a raw `eprintln!` and a best-effort append to a fixed,
/// always-writable fallback file, then *additionally* emits a `tracing`
/// event for when a subscriber is up — belt and suspenders for a packaged,
/// no-console Windows build where this is the only diagnostic path.
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

        write_crash_fallback(&location, &payload);
        eprintln!("PANIC at {location}: {payload}");
        tracing::error!(%location, %payload, "PANIC");
        prev(info);
    }));
}

/// Append one line to `%TEMP%\boris-crash.txt` (or the platform temp dir
/// equivalent) — independent of tracing/file-appender state, so a panic that
/// happens before (or instead of) the real logging subscriber coming up
/// still leaves a breadcrumb on disk. Best-effort only; never panics itself.
fn write_crash_fallback(location: &str, payload: &str) {
    use std::io::Write;

    let secs_since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = std::env::temp_dir().join("boris-crash.txt");
    let line = format!("[unix:{secs_since_epoch}] PANIC at {location}: {payload}\n");

    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Path hint for the log directory / active file (for UI debug copy).
pub fn log_path_hint() -> String {
    LOG_FILE_PATH
        .get()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| logs_dir().join("boris-desktop.log").display().to_string())
}

/// Max characters kept per `frontend_log` field before truncation. Any JS
/// caller looping on this command (accidentally or otherwise) can otherwise
/// grow the log file unbounded — see [`prune_old_logs`] for the companion
/// file-count/age cap.
const FRONTEND_LOG_FIELD_MAX_CHARS: usize = 4096;

/// Truncate `s` to at most `FRONTEND_LOG_FIELD_MAX_CHARS` chars, appending a
/// `...[truncated]` marker when truncation happened. Truncates on a char
/// boundary so multi-byte UTF-8 text is never split mid-codepoint.
fn truncate_log_field(s: &str) -> std::borrow::Cow<'_, str> {
    if s.chars().count() <= FRONTEND_LOG_FIELD_MAX_CHARS {
        return std::borrow::Cow::Borrowed(s);
    }
    let truncated: String = s.chars().take(FRONTEND_LOG_FIELD_MAX_CHARS).collect();
    std::borrow::Cow::Owned(format!("{truncated}...[truncated]"))
}

/// Accept frontend / webview log lines into the same file as Rust.
///
/// Levels: `error` | `warn` | `info` | `debug` (anything else → info).
/// `message`/`context` are length-capped (see [`FRONTEND_LOG_FIELD_MAX_CHARS`])
/// before being written.
pub fn write_frontend_log(level: &str, message: &str, context: Option<&str>) {
    let message = truncate_log_field(message);
    let ctx = truncate_log_field(context.unwrap_or(""));
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
