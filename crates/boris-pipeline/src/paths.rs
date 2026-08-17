//! Boris home directory — user data under `~/.boris` (override: `$BORIS_HOME`).
//!
//! Layout mirrors Grok Build's `$GROK_HOME` model (see `assets/grok-build`):
//! one prefs file, secrets separate, sessions / memory / skills / logs as
//! first-class roots. Boris keeps a local `models/` tree for offline STT/TTS.
//!
//! ```text
//! ~/.boris/
//!   config.toml              # prefs only (TOML sections) — like ~/.grok/config.toml
//!   auth.json                # secrets only — like ~/.grok/auth.json
//!   sessions/
//!     desktop/               # voice MVP bucket (Grok uses encode_cwd(cwd))
//!       current.json
//!       {uuid}/              # Grok-like UUID session id
//!         summary.json       # Grok-like session summary
//!         chat_history.jsonl # full transcript (user/assistant/tool_result/system)
//!         events.jsonl       # lightweight turn events
//!         tool_calls.jsonl   # primary tool audit (session-scoped)
//!         todos.json         # session plan list
//!         memory.md          # session turn log (LTM append)
//!         artifacts/         # visual cards: index.json + `{slug}-{id}.{ext}`
//!         subagents/         # child subagent artifacts
//!   memory/
//!     MEMORY.md              # single global curated knowledge
//!     profile.json
//!     notes.jsonl
//!     desktop/               # workspace bucket when no project cwd
//!       MEMORY.md            # workspace-scoped curated notes (not per-chat logs)
//!   skills/                  # user skills (<name>/SKILL.md)
//!   logs/
//!     boris-desktop.*.log
//!     audit/                 # legacy global audit (unused by engine)
//!       tool_calls.jsonl
//!   models/
//!     parakeet/ | supertone/ | livekit/ | silero/
//!   speaker/
//!     live.json              # wake liveness enroll (acoustic takes)
//!   state/
//!     workspace/             # default agent write root (was top-level sandbox/)
//! ```
//!
//! Project-local (optional, Grok-style):
//! ```text
//! <repo>/.boris/skills/<name>/SKILL.md
//! ```
//!
//! On first access after upgrade, [`migrate_home_if_needed`] rewrites the old
//! flat layout (settings.json, top-level audit/, sandbox/, flat sessions/).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Env override for the home root (default: `$HOME/.boris` / `%USERPROFILE%\.boris`).
pub const BORIS_HOME_ENV: &str = "BORIS_HOME";

/// Fixed sessions / memory workspace bucket for desktop voice (no project cwd).
pub const DESKTOP_WORKSPACE: &str = "desktop";

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

// ── Config / auth ────────────────────────────────────────────────────────────

/// Prefs file (`~/.boris/config.toml`).
pub fn config_path() -> PathBuf {
    boris_home().join("config.toml")
}

/// Secrets file (`~/.boris/auth.json`).
pub fn auth_path() -> PathBuf {
    boris_home().join("auth.json")
}

/// Legacy settings path (migrated → config.toml + auth.json).
pub fn legacy_settings_path() -> PathBuf {
    boris_home().join("settings.json")
}

// ── Models ───────────────────────────────────────────────────────────────────

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

pub fn speaker_dir() -> PathBuf {
    boris_home().join("speaker")
}

pub fn silero_dir() -> PathBuf {
    models_dir().join("silero")
}

// ── Sessions (Grok: sessions/{cwd-encoded}/) ─────────────────────────────────

/// Root of all sessions (`~/.boris/sessions`).
pub fn sessions_root() -> PathBuf {
    boris_home().join("sessions")
}

/// Active workspace sessions dir (desktop voice → `sessions/desktop`).
pub fn sessions_dir() -> PathBuf {
    sessions_root().join(DESKTOP_WORKSPACE)
}

// ── Logs + audit ─────────────────────────────────────────────────────────────

pub fn logs_dir() -> PathBuf {
    boris_home().join("logs")
}

pub fn ensure_logs_dir() -> std::io::Result<()> {
    fs::create_dir_all(logs_dir())
}

pub fn ensure_sessions_dir() -> std::io::Result<()> {
    fs::create_dir_all(sessions_dir())
}

/// Structured per-turn performance traces (`~/.boris/traces`).
pub fn traces_dir() -> PathBuf {
    boris_home().join("traces")
}

/// Append-only engine trace stream consumed by `cargo xtask trace-report`.
pub fn turn_traces_path() -> PathBuf {
    traces_dir().join("turns.jsonl")
}

/// Legacy global tool audit directory (`~/.boris/logs/audit`).
///
/// **Unused by the engine.** Primary tool audit is session-scoped:
/// `{sessions}/desktop/{uuid}/tool_calls.jsonl`, bound via
/// [`boris_agent::Agent::set_audit_path`] at session start.
pub fn audit_dir() -> PathBuf {
    logs_dir().join("audit")
}

/// Legacy global append-only tool audit path (`~/.boris/logs/audit/tool_calls.jsonl`).
///
/// **Unused by the engine** (no dual-write). Kept for migration helpers and
/// tests. Session bind writes `{session}/tool_calls.jsonl` instead.
pub fn audit_path() -> PathBuf {
    audit_dir().join("tool_calls.jsonl")
}

// ── Memory (Grok: memory/ + workspace subdirs) ───────────────────────────────

pub fn memory_dir() -> PathBuf {
    boris_home().join("memory")
}

pub fn notes_path() -> PathBuf {
    memory_dir().join("notes.jsonl")
}

pub fn profile_path() -> PathBuf {
    memory_dir().join("profile.json")
}

/// Global curated knowledge (`~/.boris/memory/MEMORY.md`).
pub fn memory_md_path() -> PathBuf {
    memory_dir().join("MEMORY.md")
}

/// Workspace memory root for desktop voice (`~/.boris/memory/desktop`).
pub fn memory_workspace_dir() -> PathBuf {
    memory_dir().join(DESKTOP_WORKSPACE)
}

/// Legacy path: old LTM turn logs lived here (`memory/desktop/sessions/`).
///
/// New code writes session logs to `{sessions_dir()}/{id}/memory.md` instead.
/// Kept for migration of home layout only — engine no longer appends here.
pub fn memory_sessions_dir() -> PathBuf {
    memory_workspace_dir().join("sessions")
}

// ── Skills ───────────────────────────────────────────────────────────────────

pub fn skills_dir() -> PathBuf {
    boris_home().join("skills")
}

// ── Agent write root (was top-level sandbox/) ────────────────────────────────

/// Default agent write root (`~/.boris/state/workspace`).
pub fn workspace_dir() -> PathBuf {
    boris_home().join("state").join("workspace")
}

/// Alias kept for call sites that still say "sandbox".
pub fn sandbox_dir() -> PathBuf {
    workspace_dir()
}

/// Ensure agent-related dirs exist.
pub fn ensure_agent_dirs() -> std::io::Result<()> {
    fs::create_dir_all(workspace_dir())?;
    fs::create_dir_all(audit_dir())?;
    fs::create_dir_all(skills_dir())?;
    fs::create_dir_all(memory_dir())?;
    fs::create_dir_all(memory_workspace_dir())?;
    fs::create_dir_all(traces_dir())?;
    // Session turn logs live under sessions/{id}/memory.md — do not create
    // the legacy memory/desktop/sessions tree.
    fs::create_dir_all(memory_workspace_dir())?;
    Ok(())
}

pub fn ensure_model_dirs() -> std::io::Result<()> {
    for d in [
        parakeet_dir(),
        supertone_onnx_dir(),
        supertone_voices_dir(),
        livekit_dir(),
        silero_dir(),
    ] {
        fs::create_dir_all(&d)?;
    }
    Ok(())
}

// ── Migration from flat legacy layout ────────────────────────────────────────

/// Best-effort rewrite of the old flat `~/.boris` tree into the Grok-like layout.
/// Safe to call every launch (no-ops when already migrated).
pub fn migrate_home_if_needed() {
    let home = boris_home();
    if !home.is_dir() {
        return;
    }

    // audit/ → logs/audit/
    let old_audit = home.join("audit");
    let new_audit = audit_dir();
    if old_audit.is_dir() && !new_audit.join("tool_calls.jsonl").is_file() {
        if let Err(e) = fs::create_dir_all(&new_audit) {
            tracing::warn!(error = %e, "migrate: create logs/audit failed");
        } else {
            move_file_if_present(
                &old_audit.join("tool_calls.jsonl"),
                &new_audit.join("tool_calls.jsonl"),
            );
            // Remove empty old dir if possible.
            let _ = fs::remove_dir(&old_audit);
        }
    }

    // sandbox/ → state/workspace/
    let old_sandbox = home.join("sandbox");
    let new_ws = workspace_dir();
    if old_sandbox.is_dir() {
        if let Err(e) = fs::create_dir_all(&new_ws) {
            tracing::warn!(error = %e, "migrate: create state/workspace failed");
        } else {
            move_dir_contents(&old_sandbox, &new_ws);
            let _ = fs::remove_dir(&old_sandbox);
        }
    }

    // sessions/{id}/ → sessions/desktop/{id}/ (any top-level session dir except desktop/)
    // Also move top-level current.json into desktop/ if present.
    let sess_root = sessions_root();
    let desktop = sessions_dir();
    if sess_root.is_dir() {
        let needs_migrate = fs::read_dir(&sess_root)
            .map(|rd| {
                rd.filter_map(|e| e.ok()).any(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let path = e.path();
                    (path.is_dir() && name != DESKTOP_WORKSPACE)
                        || (path.is_file() && name == "current.json")
                })
            })
            .unwrap_or(false);
        if needs_migrate {
            if let Err(e) = fs::create_dir_all(&desktop) {
                tracing::warn!(error = %e, "migrate: create sessions/desktop failed");
            } else if let Ok(rd) = fs::read_dir(&sess_root) {
                for entry in rd.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let path = entry.path();
                    if path.is_dir() && name != DESKTOP_WORKSPACE {
                        let dest = desktop.join(&name);
                        if !dest.exists() {
                            if let Err(e) = fs::rename(&path, &dest) {
                                tracing::warn!(error = %e, from = %path.display(), "migrate session dir");
                            }
                        }
                    } else if path.is_file() && name == "current.json" {
                        let dest = desktop.join("current.json");
                        if !dest.exists() {
                            move_file_if_present(&path, &dest);
                        }
                    }
                }
            }
        }
    }

    // memory/sessions → memory/desktop/sessions
    let old_mem_sess = memory_dir().join("sessions");
    let new_mem_sess = memory_sessions_dir();
    if old_mem_sess.is_dir() {
        if let Err(e) = fs::create_dir_all(&new_mem_sess) {
            tracing::warn!(error = %e, "migrate: create memory/desktop/sessions failed");
        } else {
            move_dir_contents(&old_mem_sess, &new_mem_sess);
            let _ = fs::remove_dir(&old_mem_sess);
        }
    }

    // Ensure desktop MEMORY.md scaffold exists if global does and desktop missing.
    let desk_mem = memory_workspace_dir().join("MEMORY.md");
    if memory_md_path().is_file() && !desk_mem.is_file() {
        if let Err(e) = fs::create_dir_all(memory_workspace_dir()) {
            tracing::warn!(error = %e, "migrate: create memory/desktop failed");
        } else {
            let _ = fs::write(
                &desk_mem,
                "# Project Memory — desktop\n\
                 \n\
                 > Auto-populated by Boris. Edit freely.\n\
                 > Workspace-scoped notes for the desktop voice assistant.\n",
            );
        }
    }
}

fn move_file_if_present(from: &Path, to: &Path) {
    if !from.is_file() || to.exists() {
        return;
    }
    if let Some(parent) = to.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(e) = fs::rename(from, to) {
        // Cross-volume fallback: copy + remove.
        if let Err(e2) = fs::copy(from, to).and_then(|_| fs::remove_file(from)) {
            tracing::warn!(
                error = %e,
                copy_error = %e2,
                from = %from.display(),
                to = %to.display(),
                "migrate file failed"
            );
        }
    }
}

fn move_dir_contents(from: &Path, to: &Path) {
    let Ok(rd) = fs::read_dir(from) else {
        return;
    };
    for entry in rd.flatten() {
        let dest = to.join(entry.file_name());
        if dest.exists() {
            continue;
        }
        let src = entry.path();
        if let Err(e) = fs::rename(&src, &dest) {
            if src.is_file() {
                let _ = fs::copy(&src, &dest).and_then(|_| fs::remove_file(&src));
            } else {
                tracing::warn!(error = %e, from = %src.display(), "migrate dir entry failed");
            }
        }
    }
}

// ── Model readiness (unchanged semantics) ────────────────────────────────────

pub fn parakeet_looks_ready(dir: &Path) -> bool {
    let encoder =
        dir.join("encoder-model.int8.onnx").is_file() || dir.join("encoder-model.onnx").is_file();
    let decoder = dir.join("decoder_joint-model.int8.onnx").is_file()
        || dir.join("decoder_joint-model.onnx").is_file()
        || dir.join("decoder-model.int8.onnx").is_file()
        || dir.join("decoder-model.onnx").is_file();
    let vocab = dir.join("vocab.txt").is_file();
    let fe = dir.join("nemo128.onnx").is_file();
    encoder && decoder && vocab && fe
}

pub fn supertone_looks_ready(onnx: &Path, voices: &Path) -> bool {
    onnx.join("vocoder.onnx").is_file()
        && onnx.join("text_encoder.onnx").is_file()
        && onnx.join("vector_estimator.onnx").is_file()
        && onnx.join("duration_predictor.onnx").is_file()
        && onnx.join("unicode_indexer.json").is_file()
        && onnx.join("tts.json").is_file()
        && voices.join("M4.json").is_file()
        // st-tts wraps every utterance as `<en>…</en>`. Supertonic 1
        // (opensource-en) maps `<`/`>`/`/` to unknown tokens → garbage audio
        // like "an an an an". Only accept multilingual v2/v3 graphs.
        && supertone_onnx_is_multilingual(onnx)
}

/// True when `tts.json` is Supertonic 2/3 multilingual (compatible with st-tts lang tags).
///
/// Supertonic 1 is `"split": "opensource-en"` / `tts_version` v1.5.x and must be rejected.
pub fn supertone_onnx_is_multilingual(onnx: &Path) -> bool {
    let path = onnx.join("tts.json");
    let Ok(raw) = fs::read_to_string(&path) else {
        return false;
    };
    if raw.contains("opensource-en") {
        return false;
    }
    raw.contains("opensource-multilingual")
        || raw.contains("\"tts_version\": \"v1.6")
        || raw.contains("\"tts_version\": \"v1.7")
        || raw.contains("\"tts_version\":\"v1.6")
        || raw.contains("\"tts_version\":\"v1.7")
}

/// Human-readable reason when Supertone files are present but unusable.
pub fn supertone_version_problem(onnx: &Path) -> Option<String> {
    let path = onnx.join("tts.json");
    if !path.is_file() {
        return None;
    }
    if supertone_onnx_is_multilingual(onnx) {
        return None;
    }
    let raw = fs::read_to_string(&path).unwrap_or_default();
    let version = raw
        .split("\"tts_version\"")
        .nth(1)
        .and_then(|s| s.split('"').nth(1))
        .unwrap_or("unknown");
    let split = if raw.contains("opensource-en") {
        "opensource-en (Supertonic 1 English-only)"
    } else {
        "unknown / not multilingual"
    };
    Some(format!(
        "Supertone models are {split}, tts_version={version}. \
         Boris needs Supertonic 3 (opensource-multilingual from Hugging Face \
         Supertone/supertonic-3). Re-run Install models — old v1 weights produce \
         nonsense speech like \"an an an an\"."
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreflightReport {
    pub parakeet_ready: bool,
    pub supertone_ready: bool,
    pub boris_home: String,
    pub parakeet_dir: String,
    pub supertone_onnx_dir: String,
    pub supertone_voices_dir: String,
    pub ok: bool,
    pub messages: Vec<String>,
}

pub fn preflight() -> PreflightReport {
    migrate_home_if_needed();
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
        if let Some(problem) = supertone_version_problem(&onnx) {
            messages.push(problem);
        } else {
            messages.push(format!(
                "Supertone TTS models missing. Need Supertonic 3 onnx graphs under {} \
                 and M4.json under {} — or use Install models in the app.",
                onnx.display(),
                voices.display()
            ));
        }
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

pub fn bootstrap_models_if_needed() -> Result<(), String> {
    ensure_model_dirs().map_err(|e| format!("create ~/.boris/models: {e}"))?;

    // This runs on every engine spawn, including packaged releases where the
    // product models are already installed under ~/.boris. Check readiness
    // FIRST and short-circuit before walking the filesystem for workspace
    // `assets/models` — that walk is only useful for a dev checkout that
    // hasn't bootstrapped yet, so skip it entirely once both models are ready.
    let pk = parakeet_dir();
    let onnx = supertone_onnx_dir();
    let voices = supertone_voices_dir();
    if parakeet_looks_ready(&pk) && supertone_looks_ready(&onnx, &voices) {
        tracing::debug!("models already ready under ~/.boris; skipping dev-asset bootstrap walk");
        return Ok(());
    }

    let src_root = find_dev_assets_models();
    let Some(src_root) = src_root else {
        tracing::debug!("no workspace assets/models found for bootstrap");
        return Ok(());
    };

    if !parakeet_looks_ready(&pk) {
        let from = src_root.join("parakeet");
        if from.is_dir() {
            tracing::info!(from = %from.display(), to = %pk.display(), "seeding parakeet into ~/.boris");
            copy_dir_recursive(&from, &pk).map_err(|e| format!("copy parakeet: {e}"))?;
        }
    }

    if !supertone_looks_ready(&onnx, &voices) {
        let from_onnx = src_root.join("supertone").join("onnx");
        let from_voices = src_root.join("supertone").join("voices");
        // Never seed Supertonic 1 English-only assets — st-tts needs multilingual v2/v3.
        if from_onnx.is_dir() && !supertone_onnx_is_multilingual(&from_onnx) {
            tracing::warn!(
                from = %from_onnx.display(),
                "skipping supertone bootstrap: workspace assets are not multilingual \
                 Supertonic 2/3 (use Install models → Hugging Face supertonic-3)"
            );
        } else if from_onnx.is_dir() {
            tracing::info!(from = %from_onnx.display(), to = %onnx.display(), "seeding supertone onnx into ~/.boris");
            copy_dir_recursive(&from_onnx, &onnx)
                .map_err(|e| format!("copy supertone onnx: {e}"))?;
        }
        if from_voices.is_dir() && supertone_onnx_is_multilingual(&from_onnx) {
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

    let silero = silero_dir();
    if !silero.join("silero_vad.onnx").is_file() {
        let from = src_root.join("silero");
        if from.is_dir() {
            tracing::info!(from = %from.display(), to = %silero.display(), "seeding silero vad into ~/.boris");
            copy_dir_recursive(&from, &silero).map_err(|e| format!("copy silero: {e}"))?;
        }
    }

    Ok(())
}

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
        } else if ty.is_file() && !to.exists() {
            fs::copy(entry.path(), &to)?;
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

    #[test]
    fn sessions_under_desktop_bucket() {
        let s = sessions_dir();
        assert!(
            s.ends_with("sessions") || s.ends_with(DESKTOP_WORKSPACE),
            "{}",
            s.display()
        );
        assert!(s.to_string_lossy().contains("sessions"));
        assert!(s.to_string_lossy().contains(DESKTOP_WORKSPACE));
    }

    #[test]
    fn audit_nested_under_logs() {
        let a = audit_path();
        assert!(a.to_string_lossy().contains("logs"));
        assert!(a.to_string_lossy().contains("audit"));
        assert!(a.ends_with("tool_calls.jsonl"));
    }

    #[test]
    fn sandbox_is_state_workspace() {
        let s = sandbox_dir();
        assert!(s.to_string_lossy().contains("state"));
        assert!(s.to_string_lossy().contains("workspace"));
    }
}
