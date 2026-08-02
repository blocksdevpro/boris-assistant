mod orchestrator;
mod overlay_win;
mod tray;

use std::path::PathBuf;
use std::sync::OnceLock;

use boris_pipeline::{
    ensure_logs_dir, load_settings, logs_dir, save_settings, AppSettings, DeviceDto,
    DownloadProgress, ModelsInstallReport, ModelsStatus, PreflightReport, StatusPicture,
};
use orchestrator::AppState;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Keeps the non-blocking file writer alive for the process lifetime.
static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

/// Active log file path (for UI / `get_log_path`).
static LOG_FILE_PATH: OnceLock<PathBuf> = OnceLock::new();

fn default_env_filter() -> EnvFilter {
    // Prefer RUST_LOG; BORIS_LOG is an alias for people who only set Boris env vars.
    let raw = std::env::var("RUST_LOG")
        .or_else(|_| std::env::var("BORIS_LOG"))
        .ok();

    if let Some(spec) = raw {
        match EnvFilter::try_new(spec) {
            Ok(f) => return f,
            Err(e) => {
                eprintln!("invalid RUST_LOG/BORIS_LOG filter ({e}); using defaults");
            }
        }
    }

    // Packaged installs on other machines: default to DEBUG for every Boris crate
    // so `~/.boris/logs` has enough detail without setting env vars.
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
fn init_tracing() {
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
            // Fallback: still build something so try_init path is uniform.
            // Write next to CWD if home is broken.
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

    let filter = default_env_filter();

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

    // Capture panics into the same log stream.
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

    tracing::info!(
        log_dir = %logs_dir().display(),
        debug_assertions = cfg!(debug_assertions),
        "Boris desktop logging initialized (pipeline crates default to DEBUG)"
    );

    // Dump paths / models / audio / sidecar DLLs immediately so a crash on
    // first Start still leaves a useful file on the other PC.
    boris_pipeline::log_environment("app_boot");
}

/// Path hint for the log directory / active file (for UI debug copy).
#[tauri::command]
fn get_log_path() -> String {
    LOG_FILE_PATH
        .get()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| logs_dir().join("boris-desktop.log").display().to_string())
}

/// Accept frontend / webview log lines into the same file as Rust.
///
/// Levels: `error` | `warn` | `info` | `debug` (anything else → info).
#[tauri::command]
fn frontend_log(level: String, message: String, context: Option<String>) {
    let ctx = context.as_deref().unwrap_or("");
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

#[tauri::command]
fn get_status(state: State<'_, AppState>) -> StatusPicture {
    state.status()
}

/// Model readiness gate for the UI (paths under `~/.boris`).
#[tauri::command]
fn preflight_check() -> PreflightReport {
    let report = AppState::preflight();
    tracing::debug!(
        ok = report.ok,
        messages = ?report.messages,
        "preflight_check"
    );
    report
}

#[tauri::command]
fn start_engine(
    app: AppHandle,
    state: State<'_, AppState>,
    api_key: String,
    model: Option<String>,
) -> Result<(), String> {
    let key_from_env = api_key.trim().is_empty();
    let key = if key_from_env {
        std::env::var("OPENROUTER_API_KEY").unwrap_or_default()
    } else {
        api_key
    };
    let model = model.or_else(|| std::env::var("OPENROUTER_MODEL").ok());

    tracing::info!(
        key_source = if key_from_env { "env" } else { "ui" },
        key_present = !key.trim().is_empty(),
        model = ?model.as_deref(),
        "start_engine command"
    );

    state.start(key, model, move |picture| {
        let _ = app.emit("status", picture);
    })
    .map_err(|e| {
        tracing::error!(error = %e, "start_engine failed");
        e
    })
    .map(|()| {
        tracing::info!("start_engine ok");
    })
}

#[tauri::command]
fn stop_engine(state: State<'_, AppState>) -> Result<(), String> {
    tracing::info!("stop_engine command");
    state.stop().map_err(|e| {
        tracing::error!(error = %e, "stop_engine failed");
        e
    })
}

#[tauri::command]
fn list_input_devices() -> Vec<DeviceDto> {
    let list = AppState::list_inputs();
    tracing::debug!(count = list.len(), "list_input_devices");
    list
}

#[tauri::command]
fn list_output_devices() -> Vec<DeviceDto> {
    let list = AppState::list_outputs();
    tracing::debug!(count = list.len(), "list_output_devices");
    list
}

#[tauri::command]
fn switch_input(state: State<'_, AppState>, device_id: String) -> Result<(), String> {
    tracing::info!(%device_id, "switch_input command");
    state.switch_input(device_id).map_err(|e| {
        tracing::error!(error = %e, "switch_input failed");
        e
    })
}

#[tauri::command]
fn switch_output(state: State<'_, AppState>, device_id: String) -> Result<(), String> {
    tracing::info!(%device_id, "switch_output command");
    state.switch_output(device_id).map_err(|e| {
        tracing::error!(error = %e, "switch_output failed");
        e
    })
}

#[tauri::command]
fn models_status() -> ModelsStatus {
    let status = boris_pipeline::models_status();
    tracing::debug!(
        parakeet = status.parakeet_ready,
        supertone = status.supertone_ready,
        "models_status"
    );
    status
}

/// Download missing STT/TTS models into `~/.boris/models`.
///
/// Runs on a worker thread so the UI stays responsive for multi-hundred-MB
/// transfers. Emits `models-progress` ([`DownloadProgress`]) while running.
#[tauri::command]
async fn download_models(app: AppHandle) -> Result<ModelsInstallReport, String> {
    tracing::info!("download_models started");
    // Blocking reqwest must not run on the async/UI path — that freezes the
    // window ("Not Responding") for the entire install (~900 MB).
    let report = tauri::async_runtime::spawn_blocking(move || {
        boris_pipeline::install_models(|progress: DownloadProgress| {
            let _ = app.emit("models-progress", &progress);
        })
    })
    .await
    .map_err(|e| {
        let msg = format!("download task failed: {e}");
        tracing::error!(error = %msg, "download_models join failed");
        msg
    })?
    .map_err(|e| {
        tracing::error!(error = %e, "download_models install failed");
        e
    })?;

    tracing::info!(
        ok = report.ok,
        downloaded = report.files_downloaded,
        failed = report.files_failed,
        "download_models finished"
    );
    Ok(report)
}

/// Restore OpenRouter key + model from `~/.boris/settings.json`.
#[tauri::command]
fn get_settings() -> Result<AppSettings, String> {
    match load_settings() {
        Ok(s) => {
            tracing::debug!(
                has_key = !s.openrouter_api_key.trim().is_empty(),
                model = %s.openrouter_model,
                "get_settings ok"
            );
            Ok(s)
        }
        Err(e) => {
            tracing::warn!(error = %e, "get_settings failed");
            Err(e)
        }
    }
}

/// Persist OpenRouter key + model to `~/.boris/settings.json` (never logged).
#[tauri::command]
fn save_app_settings(settings: AppSettings) -> Result<(), String> {
    tracing::info!(
        has_key = !settings.openrouter_api_key.trim().is_empty(),
        model = %settings.openrouter_model,
        "save_app_settings"
    );
    save_settings(&settings).map_err(|e| {
        tracing::error!(error = %e, "save_app_settings failed");
        e
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();
    tracing::info!("starting Tauri app");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            get_status,
            preflight_check,
            start_engine,
            stop_engine,
            list_input_devices,
            list_output_devices,
            switch_input,
            switch_output,
            models_status,
            download_models,
            get_settings,
            save_app_settings,
            get_log_path,
            frontend_log,
        ])
        .setup(|app| {
            tracing::info!("app setup begin");

            // Overlay is `"create": false` in config — build with explicit transparent API.
            match overlay_win::spawn_overlay_window(app.handle()) {
                Ok(()) => tracing::info!("overlay window spawned"),
                Err(e) => {
                    tracing::error!(error = %e, "failed to spawn overlay window");
                    return Err(e.into());
                }
            }

            // Tray keeps control after the main console is closed/hidden.
            if let Err(e) = tray::setup_tray(app.handle()) {
                tracing::error!(error = %e, "failed to create system tray");
            }

            if let Some(state) = app.try_state::<AppState>() {
                let _ = app.emit("status", state.status());
            }

            if let Some(main) = app.get_webview_window("main") {
                tracing::info!(visible = main.is_visible().unwrap_or(false), "main window present");
            } else {
                tracing::warn!("main window missing at setup");
            }

            tracing::info!(
                log_hint = %get_log_path(),
                "app setup complete — logs written under ~/.boris/logs"
            );
            Ok(())
        })
        // Closing the console hides it; only tray "Quit Boris" exits the app.
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                tracing::info!("main window hidden to tray");
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
