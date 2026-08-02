//! Boris home directory — user data lives under `~/.boris`.
//!
//! Layout (owned by us; not the legacy `config.toml`):
//!
//! ```text
//! ~/.boris/
//!   settings.json        # OpenRouter key + model (desktop)
//!   sessions/            # voice session meta + JSONL transcripts (agent store)
//!   memory/
//!     notes.jsonl        # durable notes for builtin memory tools
//!     profile.json       # active personal context (name, prefs, facts)
//!   logs/                # desktop + pipeline file logs (release builds)
//!   models/
//!     parakeet/          # STT
//!     supertone/
//!       onnx/            # TTS graphs
//!       voices/          # voice json (e.g. M4.json)
//!     livekit/           # optional wake onnx on disk
//! ```
//!
//! Override root with env `BORIS_HOME`. Never reads `config.toml`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Env override for the home root (default: `$HOME/.boris` / `%USERPROFILE%\.boris`).
pub const BORIS_HOME_ENV: &str = "BORIS_HOME";

pub fn boris_home() -> PathBuf {
    if let Ok(p) = std::env::var(BORIS_HOME_ENV) {
        let p = PathBuf::from(p);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".boris")
}

fn home_dir() -> Option<PathBuf> {
    // Prefer USERPROFILE on Windows (dirs not required).
    if let Ok(p) = std::env::var("USERPROFILE") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    if let Ok(p) = std::env::var("HOME") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    None
}

pub fn models_dir() -> PathBuf {
    boris_home().join("models")
}

pub fn parakeet_dir() -> PathBuf {
    models_dir().join("parakeet")
}

pub fn supertone_onnx_dir() -> PathBuf {
    models_dir().join("supertone").join("onnx")
}

pub fn supertone_voices_dir() -> PathBuf {
    models_dir().join("supertone").join("voices")
}

pub fn livekit_dir() -> PathBuf {
    models_dir().join("livekit")
}

/// Directory for voice session metadata + transcripts (`~/.boris/sessions`).
pub fn sessions_dir() -> PathBuf {
    boris_home().join("sessions")
}

/// Directory for desktop / pipeline log files (`~/.boris/logs`).
pub fn logs_dir() -> PathBuf {
    boris_home().join("logs")
}

/// Ensure `~/.boris/logs` exists (called at desktop startup).
pub fn ensure_logs_dir() -> std::io::Result<()> {
    fs::create_dir_all(logs_dir())
}

/// Ensure `~/.boris/sessions` exists.
pub fn ensure_sessions_dir() -> std::io::Result<()> {
    fs::create_dir_all(sessions_dir())
}

/// Directory for durable agent memory (`~/.boris/memory`).
///
/// The notes tool may create this on first write; callers need not pre-create.
pub fn memory_dir() -> PathBuf {
    boris_home().join("memory")
}

/// Append-only notes store for builtin memory tools (`~/.boris/memory/notes.jsonl`).
pub fn notes_path() -> PathBuf {
    memory_dir().join("notes.jsonl")
}

/// Durable personal context profile (`~/.boris/memory/profile.json`).
pub fn profile_path() -> PathBuf {
    memory_dir().join("profile.json")
}

/// Ensure the directory tree exists under `~/.boris/models`.
pub fn ensure_model_dirs() -> std::io::Result<()> {
    for d in [
        parakeet_dir(),
        supertone_onnx_dir(),
        supertone_voices_dir(),
        livekit_dir(),
    ] {
        fs::create_dir_all(&d)?;
    }
    Ok(())
}

/// True if Parakeet has the real ONNX graphs needed to load (not just config).
pub fn parakeet_looks_ready(dir: &Path) -> bool {
    let encoder =
        dir.join("encoder-model.int8.onnx").is_file() || dir.join("encoder-model.onnx").is_file();
    let decoder = dir.join("decoder_joint-model.int8.onnx").is_file()
        || dir.join("decoder_joint-model.onnx").is_file()
        || dir.join("decoder-model.int8.onnx").is_file()
        || dir.join("decoder-model.onnx").is_file();
    let vocab = dir.join("vocab.txt").is_file();
    // nemo128 is part of the onnx-asr feature pipeline; require it so a partial
    // copy does not look "ready".
    let fe = dir.join("nemo128.onnx").is_file();
    encoder && decoder && vocab && fe
}

/// True if Supertone has loadable graphs + default voice (not config-only).
pub fn supertone_looks_ready(onnx: &Path, voices: &Path) -> bool {
    onnx.join("vocoder.onnx").is_file()
        && onnx.join("text_encoder.onnx").is_file()
        && onnx.join("vector_estimator.onnx").is_file()
        && onnx.join("duration_predictor.onnx").is_file()
        && onnx.join("tts.json").is_file()
        && voices.join("M4.json").is_file()
}

/// Snapshot for UI preflight gate + host start defense-in-depth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreflightReport {
    pub parakeet_ready: bool,
    pub supertone_ready: bool,
    pub boris_home: String,
    pub parakeet_dir: String,
    pub supertone_onnx_dir: String,
    pub supertone_voices_dir: String,
    /// Both STT and TTS model trees look ready.
    pub ok: bool,
    pub messages: Vec<String>,
}

/// Best-effort bootstrap then report whether required models are on disk.
pub fn preflight() -> PreflightReport {
    if let Err(e) = bootstrap_models_if_needed() {
        tracing::warn!(error = %e, "model bootstrap during preflight failed");
    }

    let home = boris_home();
    let pk = parakeet_dir();
    let onnx = supertone_onnx_dir();
    let voices = supertone_voices_dir();

    let parakeet_ready = parakeet_looks_ready(&pk);
    let supertone_ready = supertone_looks_ready(&onnx, &voices);

    let mut messages = Vec::new();
    if !parakeet_ready {
        messages.push(format!(
            "Parakeet STT models missing. Place encoder + decoder ONNX, vocab.txt, \
             and nemo128.onnx under {} — or use Install models in the app.",
            pk.display()
        ));
    }
    if !supertone_ready {
        messages.push(format!(
            "Supertone TTS models missing. Need full onnx graphs under {} \
             and M4.json under {} — or use Install models in the app.",
            onnx.display(),
            voices.display()
        ));
    }
    if messages.is_empty() {
        messages.push("Models look ready under ~/.boris.".into());
    }

    PreflightReport {
        parakeet_ready,
        supertone_ready,
        boris_home: home.display().to_string(),
        parakeet_dir: pk.display().to_string(),
        supertone_onnx_dir: onnx.display().to_string(),
        supertone_voices_dir: voices.display().to_string(),
        ok: parakeet_ready && supertone_ready,
        messages,
    }
}

/// One-time / best-effort seed: copy from a discovered workspace `assets/models`
/// into `~/.boris/models` when the home models are empty.
///
/// Safe to call every launch — no-ops when already populated.
/// Product path for missing models is HTTP install (`download` module).
pub fn bootstrap_models_if_needed() -> Result<(), String> {
    ensure_model_dirs().map_err(|e| format!("create ~/.boris/models: {e}"))?;

    let src_root = find_dev_assets_models();
    let Some(src_root) = src_root else {
        tracing::debug!("no workspace assets/models found for bootstrap");
        return Ok(());
    };

    let pk = parakeet_dir();
    if !parakeet_looks_ready(&pk) {
        let from = src_root.join("parakeet");
        if from.is_dir() {
            tracing::info!(from = %from.display(), to = %pk.display(), "seeding parakeet into ~/.boris");
            copy_dir_recursive(&from, &pk).map_err(|e| format!("copy parakeet: {e}"))?;
        }
    }

    let onnx = supertone_onnx_dir();
    let voices = supertone_voices_dir();
    if !supertone_looks_ready(&onnx, &voices) {
        let from_onnx = src_root.join("supertone").join("onnx");
        let from_voices = src_root.join("supertone").join("voices");
        if from_onnx.is_dir() {
            tracing::info!(from = %from_onnx.display(), to = %onnx.display(), "seeding supertone onnx into ~/.boris");
            copy_dir_recursive(&from_onnx, &onnx)
                .map_err(|e| format!("copy supertone onnx: {e}"))?;
        }
        if from_voices.is_dir() {
            tracing::info!(from = %from_voices.display(), to = %voices.display(), "seeding supertone voices into ~/.boris");
            copy_dir_recursive(&from_voices, &voices)
                .map_err(|e| format!("copy supertone voices: {e}"))?;
        }
    }

    let live = livekit_dir();
    if !live.join("boris-large.onnx").is_file() {
        let from = src_root.join("livekit");
        if from.is_dir() {
            tracing::info!(from = %from.display(), to = %live.display(), "seeding livekit wake into ~/.boris");
            copy_dir_recursive(&from, &live).map_err(|e| format!("copy livekit: {e}"))?;
        }
    }

    Ok(())
}

/// Walk CWD / parents / common Tauri cwd for `assets/models`.
fn find_dev_assets_models() -> Option<PathBuf> {
    let mut starts = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(p) = exe.parent() {
            starts.push(p.to_path_buf());
        }
    }
    // This crate lives at crates/boris-pipeline → workspace root is ../..
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    starts.push(manifest.join("../.."));
    starts.push(manifest.join("../../.."));

    for start in starts {
        let mut dir = start;
        for _ in 0..6 {
            let candidate = dir.join("assets").join("models");
            if candidate.is_dir() {
                return Some(candidate);
            }
            if !dir.pop() {
                break;
            }
        }
    }
    None
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else if ty.is_file() {
            if !to.exists() {
                fs::copy(entry.path(), &to)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_ends_with_boris() {
        let h = boris_home();
        assert!(
            h.ends_with(".boris") || std::env::var(BORIS_HOME_ENV).is_ok(),
            "unexpected home {}",
            h.display()
        );
    }
}
