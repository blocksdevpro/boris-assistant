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

/// Visual C++ runtime that `onnxruntime.dll` imports. Clean Windows installs
/// (and Windows Sandbox) do not have it; without these beside the exe the
/// ORT library fails to load and nothing can listen. Shipped app-local, as
/// Microsoft's VC++ redistributable terms allow.
#[cfg(windows)]
const CRT_DLL_NAMES: &[&str] = &[
    "msvcp140.dll",
    "msvcp140_1.dll",
    "vcruntime140.dll",
    "vcruntime140_1.dll",
];

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

    for dir in crt_redist_dirs() {
        if CRT_DLL_NAMES
            .iter()
            .all(|name| seen.contains(&name.to_ascii_lowercase()))
        {
            break;
        }
        stage_named_from_dir(&dir, CRT_DLL_NAMES, dest_dir, &mut staged, &mut seen)?;
    }

    let missing: Vec<&str> = ESSENTIAL_DLL_NAMES
        .iter()
        .chain(CRT_DLL_NAMES)
        .copied()
        .filter(|name| !seen.contains(&name.to_ascii_lowercase()))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "missing required runtime DLL(s): {}; searched target/, the ort.pyke.io cache, the VC++ redist folder and System32; \
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

/// Where to copy the VC++ runtime from, best first: the redist folder of the
/// Visual Studio that is building us (the files Microsoft ships for app-local
/// use), then any installed VS, then this machine's System32.
#[cfg(windows)]
fn crt_redist_dirs() -> Vec<PathBuf> {
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86") => "x86",
        _ => "x64",
    };
    let mut roots: Vec<PathBuf> = Vec::new();
    // Set inside a VS developer prompt: ...\VC\Redist\MSVC\<version>\
    if let Ok(dir) = env::var("VCToolsRedistDir") {
        roots.push(PathBuf::from(dir));
    }
    for install in vs_install_dirs() {
        let msvc = install.join("VC").join("Redist").join("MSVC");
        let Ok(entries) = fs::read_dir(&msvc) else {
            continue;
        };
        let mut versions: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        versions.sort();
        versions.reverse(); // newest toolset first
        roots.extend(versions);
    }

    let mut dirs = Vec::new();
    for root in roots {
        let Ok(entries) = fs::read_dir(root.join(arch)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            // Microsoft.VC143.CRT, Microsoft.VC145.CRT, …
            if name.starts_with("microsoft.vc") && name.ends_with(".crt") {
                dirs.push(entry.path());
            }
        }
    }
    if arch == "x64" {
        if let Ok(root) = env::var("SystemRoot") {
            dirs.push(PathBuf::from(root).join("System32"));
        }
    }
    dirs
}

/// Visual Studio installs reported by `vswhere`, newest first.
#[cfg(windows)]
fn vs_install_dirs() -> Vec<PathBuf> {
    let Some(program_files) = env::var_os("ProgramFiles(x86)") else {
        return Vec::new();
    };
    let vswhere = PathBuf::from(program_files)
        .join("Microsoft Visual Studio")
        .join("Installer")
        .join("vswhere.exe");
    let Ok(out) = std::process::Command::new(vswhere)
        .args([
            "-all",
            "-products",
            "*",
            "-sort",
            "-property",
            "installationPath",
        ])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}
