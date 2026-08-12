//! Tauri build script.
//!
//! On Windows, stages ONNX Runtime runtime DLLs into `resources/ort/` so
//! `tauri.conf.json` `bundle.resources` can ship them next to the executable
//! (required for clean-machine installs; ort's `copy-dylibs` only covers
//! `cargo run` / `target/{profile}/`). Debug checks warn when the runtime is
//! unavailable; release builds fail so installers cannot omit it silently.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    // Always ensure the resources dir exists so `bundle.resources` path is valid
    // even when no DLLs are staged (empty dir is skipped by Tauri).
    let resources_ort =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("resources/ort");
    if let Err(e) = fs::create_dir_all(&resources_ort) {
        println!(
            "cargo:warning=failed to create {}: {e}",
            resources_ort.display()
        );
    }

    #[cfg(windows)]
    {
        if let Err(e) = stage_ort_runtime_dlls(&resources_ort) {
            if is_release_profile() {
                panic!(
                    "ORT runtime DLL staging failed for a release build: {e}. \\
                     A Windows installer must include onnxruntime.dll and DirectML.dll. \\
                     Build once with the configured ORT backend or populate the ort.pyke.io cache."
                );
            }
            // Keep normal debug `cargo check` / first-time development builds
            // usable. A later release build will repeat staging and fail closed.
            println!("cargo:warning=ORT runtime DLL staging: {e}");
        }
    }

    tauri_build::build()
}

/// Tauri's production bundles compile with Cargo's release profile.
#[cfg(windows)]
fn is_release_profile() -> bool {
    env::var("PROFILE").is_ok_and(|profile| profile == "release")
}

/// Runtime DLLs we ship next to the Windows app binary.
///
/// - `onnxruntime.dll`: shared ORT runtime when present beside the cargo binary
///   (ort `copy-dylibs` / older dynamic layouts).
/// - `DirectML.dll`: companion EP DLL for pyke static Windows ORT builds.
#[cfg(windows)]
const ESSENTIAL_DLL_NAMES: &[&str] = &["onnxruntime.dll", "DirectML.dll"];

/// Copy ORT / DirectML DLLs into `resources/ort/` for the Windows installer.
#[cfg(windows)]
fn stage_ort_runtime_dlls(dest_dir: &Path) -> Result<(), String> {
    // Clear previously staged DLLs so stale CUDA/TRT artifacts from older
    // staging runs (or wrong feature-set caches) are not re-bundled.
    if let Ok(entries) = fs::read_dir(dest_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("dll"))
                == Some(true)
            {
                let _ = fs::remove_file(&path);
            }
        }
    }

    let mut staged: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // 1) Prefer DLLs already placed next to the binary by ort `copy-dylibs`
    //    for *this* profile (correct feature set for the current build).
    //    Only ship the essentials — never CUDA/TensorRT provider packs unless
    //    we intentionally enable those ort features later.
    for dir in target_profile_dirs() {
        if !dir.is_dir() {
            continue;
        }
        stage_named_from_dir(&dir, ESSENTIAL_DLL_NAMES, dest_dir, &mut staged, &mut seen)?;
    }

    // 2) Fall back to pyke download cache for essentials only (DirectML is
    //    often only in the cache, not always copied to target/).
    if !seen.contains("directml.dll") || !seen.contains("onnxruntime.dll") {
        for dir in ort_cache_dirs() {
            if !dir.is_dir() {
                continue;
            }
            stage_named_from_dir(&dir, ESSENTIAL_DLL_NAMES, dest_dir, &mut staged, &mut seen)?;
            if seen.contains("directml.dll") && seen.contains("onnxruntime.dll") {
                break;
            }
        }
    }

    let missing: Vec<&str> = ESSENTIAL_DLL_NAMES
        .iter()
        .copied()
        .filter(|name| !seen.contains(&name.to_ascii_lowercase()))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "missing required runtime DLL(s): {}; searched target/ and the ort.pyke.io cache; \
             run a full Windows build first so ort can download/copy them",
            missing.join(", ")
        ));
    }

    println!(
        "cargo:warning=staged ORT runtime for bundle: {}",
        staged.join(", ")
    );
    Ok(())
}

#[cfg(windows)]
fn stage_named_from_dir(
    dir: &Path,
    names: &[&str],
    dest_dir: &Path,
    staged: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
) -> Result<(), String> {
    for name in names {
        let key = name.to_ascii_lowercase();
        if seen.contains(&key) {
            continue;
        }
        let src = dir.join(name);
        if src.is_file() {
            copy_dll(&src, dest_dir, name, staged, seen)?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn copy_dll(
    src: &Path,
    dest_dir: &Path,
    name: &str,
    staged: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
) -> Result<(), String> {
    let dest = dest_dir.join(name);
    fs::copy(src, &dest)
        .map_err(|e| format!("copy {} -> {}: {e}", src.display(), dest.display()))?;
    println!("cargo:rerun-if-changed={}", src.display());
    seen.insert(name.to_ascii_lowercase());
    staged.push(name.to_string());
    Ok(())
}

/// `target/{profile}` (and triple-prefixed) where ort `copy-dylibs` places DLLs.
#[cfg(windows)]
fn target_profile_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let profile = env::var("PROFILE").unwrap_or_else(|_| "release".into());
    let target_triple = env::var("TARGET").unwrap_or_default();

    if let Ok(td) = env::var("CARGO_TARGET_DIR") {
        let base = PathBuf::from(td);
        dirs.push(base.join(&profile));
        if !target_triple.is_empty() {
            dirs.push(base.join(&target_triple).join(&profile));
        }
    }

    // Workspace-relative target/ (src-tauri is desktop/src-tauri → ../../target)
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    for up in ["../../target", "../../../target", "target"] {
        let base = manifest.join(up);
        dirs.push(base.join(&profile));
        if !target_triple.is_empty() {
            dirs.push(base.join(&target_triple).join(&profile));
        }
    }

    // OUT_DIR is .../target/{profile}/build/{pkg}/out → ancestors().nth(3) = profile dir
    if let Ok(out) = env::var("OUT_DIR") {
        let out = PathBuf::from(out);
        if let Some(profile_dir) = out.ancestors().nth(3) {
            dirs.push(profile_dir.to_path_buf());
        }
    }

    dirs
}

/// pyke ORT download cache — used only as fallback for essential DLLs.
#[cfg(windows)]
fn ort_cache_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let target_triple = env::var("TARGET").unwrap_or_else(|_| "x86_64-pc-windows-msvc".into());

    let Ok(local) = env::var("LOCALAPPDATA") else {
        return dirs;
    };
    let dfbin = PathBuf::from(local)
        .join("ort.pyke.io")
        .join("dfbin")
        .join(&target_triple);
    if !dfbin.is_dir() {
        return dirs;
    }

    let Ok(entries) = fs::read_dir(&dfbin) else {
        return dirs;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        // Prefer extract roots that look like the default ("none") package:
        // contain DirectML.dll + onnxruntime.lib at the top level.
        let has_directml = p.join("DirectML.dll").is_file();
        let has_onnx_dll = p.join("onnxruntime.dll").is_file();
        if has_directml || has_onnx_dll {
            dirs.push(p.clone());
        }
        // Older nested layout
        let nested = p.join("onnxruntime").join("lib");
        if nested.join("DirectML.dll").is_file() || nested.join("onnxruntime.dll").is_file() {
            dirs.push(nested);
        }
    }
    dirs
}
