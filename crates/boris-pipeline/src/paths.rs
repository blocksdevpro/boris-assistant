//! Boris home directory — user data lives under `~/.boris`.
//!
//! Layout (owned by us; not the legacy `config.toml`):
//!
//! ```text
//! ~/.boris/
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

/// True if Parakeet has something loadable (int8 encoder or any onnx).
pub fn parakeet_looks_ready(dir: &Path) -> bool {
    dir.join("encoder-model.int8.onnx").is_file()
        || dir.join("encoder-model.onnx").is_file()
        || dir.join("config.json").is_file()
}

pub fn supertone_looks_ready(onnx: &Path, voices: &Path) -> bool {
    (onnx.join("tts.json").is_file() || onnx.join("vocoder.onnx").is_file())
        && voices.join("M4.json").is_file()
}

/// One-time / best-effort seed: copy from a discovered workspace `assets/models`
/// into `~/.boris/models` when the home models are empty.
///
/// Safe to call every launch — no-ops when already populated.
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
            copy_dir_recursive(&from_onnx, &onnx).map_err(|e| format!("copy supertone onnx: {e}"))?;
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
