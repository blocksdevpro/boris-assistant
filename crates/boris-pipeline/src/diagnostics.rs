//! Startup / environment dumps for debugging clean installs.
//!
//! On a machine without a console (packaged Windows), this is the first place
//! to look: `~/.boris/logs/boris-desktop.*.log`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::devices;
use crate::paths::{self, preflight};

/// Emit a full environment snapshot (paths, models, audio, DLLs next to exe).
///
/// Safe to call multiple times; each call writes a clearly marked block.
pub fn log_environment(context: &str) {
    tracing::info!(%context, "========== BORIS DIAGNOSTICS BEGIN ==========");

    tracing::info!(
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        family = std::env::consts::FAMILY,
        "host"
    );

    if let Ok(cwd) = std::env::current_dir() {
        tracing::info!(cwd = %cwd.display(), "process working directory");
    } else {
        tracing::warn!("could not read current_dir");
    }

    if let Ok(exe) = std::env::current_exe() {
        tracing::info!(exe = %exe.display(), "process executable");
        if let Some(dir) = exe.parent() {
            log_dir_listing("exe_dir", dir, 40);
            log_sidecar_dlls(dir);
        }
    } else {
        tracing::warn!("could not read current_exe");
    }

    // Env vars that affect models / home / logging (never log API keys).
    for key in [
        "BORIS_HOME",
        "BORIS_LOG",
        "RUST_LOG",
        "BORIS_MODEL_BASE_URL",
        "OPENROUTER_MODEL",
        "OMP_NUM_THREADS",
        "USERPROFILE",
        "HOME",
        "LOCALAPPDATA",
        "PATH",
    ] {
        match std::env::var(key) {
            Ok(v) if key == "PATH" => {
                // PATH can be huge — just length + whether onnxruntime might resolve.
                tracing::info!(%key, len = v.len(), "env set");
            }
            Ok(v) if key == "OPENROUTER_API_KEY" => {
                tracing::info!(%key, present = !v.trim().is_empty(), "env set");
            }
            Ok(v) => tracing::info!(%key, value = %v, "env set"),
            Err(_) => tracing::debug!(%key, "env unset"),
        }
    }
    let key_present = std::env::var("OPENROUTER_API_KEY")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    tracing::info!(openrouter_api_key_in_env = key_present, "credentials env");

    let home = paths::boris_home();
    tracing::info!(boris_home = %home.display(), "paths");
    tracing::info!(logs_dir = %paths::logs_dir().display(), "paths");
    tracing::info!(models_dir = %paths::models_dir().display(), "paths");
    tracing::info!(sessions_dir = %paths::sessions_dir().display(), "paths");
    tracing::info!(parakeet = %paths::parakeet_dir().display(), "paths");
    tracing::info!(supertone_onnx = %paths::supertone_onnx_dir().display(), "paths");
    tracing::info!(supertone_voices = %paths::supertone_voices_dir().display(), "paths");

    // Directory trees that matter for STT/TTS.
    log_dir_listing("parakeet", &paths::parakeet_dir(), 50);
    log_dir_listing("supertone_onnx", &paths::supertone_onnx_dir(), 50);
    log_dir_listing("supertone_voices", &paths::supertone_voices_dir(), 50);
    log_dir_listing("logs", &paths::logs_dir(), 20);

    let report = preflight();
    tracing::info!(
        ok = report.ok,
        parakeet_ready = report.parakeet_ready,
        supertone_ready = report.supertone_ready,
        boris_home = %report.boris_home,
        messages = ?report.messages,
        "preflight"
    );

    // Audio devices — common failure on clean / locked-down PCs.
    let inputs = devices::list_input_devices();
    let outputs = devices::list_output_devices();
    tracing::info!(count = inputs.len(), "input devices");
    for d in &inputs {
        tracing::info!(id = %d.id, name = %d.name, is_default = d.is_default, "input device");
    }
    if inputs.is_empty() {
        tracing::error!("NO input devices found — mic permission or no hardware?");
    }
    tracing::info!(count = outputs.len(), "output devices");
    for d in &outputs {
        tracing::info!(id = %d.id, name = %d.name, is_default = d.is_default, "output device");
    }
    if outputs.is_empty() {
        tracing::error!("NO output devices found — speaker permission or no hardware?");
    }

    tracing::info!(%context, "========== BORIS DIAGNOSTICS END ==========");
}

/// Log files next to the executable that matter for ORT on Windows.
fn log_sidecar_dlls(exe_dir: &Path) {
    for name in [
        "onnxruntime.dll",
        "DirectML.dll",
        "onnxruntime_providers_shared.dll",
        "boris_desktop_lib.dll",
    ] {
        let p = exe_dir.join(name);
        if p.is_file() {
            let size = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            tracing::info!(file = %p.display(), size_bytes = size, "sidecar present");
        } else {
            tracing::warn!(file = %p.display(), "sidecar MISSING");
        }
    }
}

/// List a directory (non-recursive) with sizes — helpful when models are incomplete.
pub fn log_dir_listing(label: &str, dir: &Path, max_entries: usize) {
    if !dir.exists() {
        tracing::warn!(%label, path = %dir.display(), "directory does not exist");
        return;
    }
    if !dir.is_dir() {
        tracing::warn!(%label, path = %dir.display(), "path exists but is not a directory");
        return;
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(%label, path = %dir.display(), error = %e, "read_dir failed");
            return;
        }
    };

    let mut names: Vec<(String, u64, bool)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_dir = path.is_dir();
        let size = if is_dir {
            0
        } else {
            fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        names.push((name, size, is_dir));
    }
    names.sort_by(|a, b| a.0.cmp(&b.0));

    tracing::info!(
        %label,
        path = %dir.display(),
        entry_count = names.len(),
        "directory listing"
    );
    for (i, (name, size, is_dir)) in names.iter().enumerate() {
        if i >= max_entries {
            tracing::info!(
                %label,
                remaining = names.len() - max_entries,
                "… truncated listing"
            );
            break;
        }
        if *is_dir {
            tracing::info!(%label, entry = %name, kind = "dir", "  entry");
        } else {
            tracing::info!(%label, entry = %name, size_bytes = size, kind = "file", "  entry");
        }
    }
}

/// Extra detail when a model load fails: re-list the dir and note expected files.
pub fn log_model_load_failure(component: &str, dir: &Path, error: &str) {
    tracing::error!(%component, path = %dir.display(), %error, "model load failure detail");
    log_dir_listing(component, dir, 80);

    // Common expected files (best-effort hints).
    let expected: &[&str] = match component {
        "parakeet" | "stt" => &[
            "encoder-model.int8.onnx",
            "encoder-model.onnx",
            "decoder_joint-model.int8.onnx",
            "decoder_joint-model.onnx",
            "vocab.txt",
            "nemo128.onnx",
            "config.json",
        ],
        "supertone" | "tts" => &[
            "duration_predictor.onnx",
            "text_encoder.onnx",
            "vector_estimator.onnx",
            "vocoder.onnx",
            "tts.json",
            "tts.yml",
            "unicode_indexer.json",
        ],
        _ => &[],
    };
    for name in expected {
        let p = dir.join(name);
        let ok = p.is_file();
        let size = if ok {
            fs::metadata(&p).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };
        if ok {
            tracing::info!(%component, file = %name, size_bytes = size, "expected file OK");
        } else {
            tracing::warn!(%component, file = %name, "expected file MISSING");
        }
    }
}

/// Resolve and log whether a path looks writable (sessions/logs on restricted accounts).
pub fn log_writable_check(label: &str, dir: PathBuf) {
    match fs::create_dir_all(&dir) {
        Ok(()) => tracing::info!(%label, path = %dir.display(), "directory writable/created"),
        Err(e) => tracing::error!(%label, path = %dir.display(), error = %e, "directory NOT writable"),
    }
    let probe = dir.join(".boris_write_probe");
    match fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            tracing::info!(%label, path = %dir.display(), "write probe ok");
        }
        Err(e) => tracing::error!(%label, path = %dir.display(), error = %e, "write probe FAILED"),
    }
}
