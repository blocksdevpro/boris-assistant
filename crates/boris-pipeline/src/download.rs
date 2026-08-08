//! HTTP install of STT/TTS models into `~/.boris/models`.
//!
//! # Layout written
//!
//! ```text
//! ~/.boris/models/
//!   parakeet/          # STT (onnx-asr style)
//!   supertone/onnx/    # TTS graphs
//!   supertone/voices/  # M4.json
//! ```
//!
//! # Source URLs
//!
//! Default remote sources (verified Hugging Face repos):
//!
//! | Component  | Default base (per-file absolute URLs below) |
//! |------------|-----------------------------------------------|
//! | Parakeet   | `https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/main` |
//! | Supertone  | `https://huggingface.co/Supertone/supertonic-3/resolve/main` |
//!
//! Override everything with env `BORIS_MODEL_BASE_URL`. When set, each file is
//! fetched from `{BORIS_MODEL_BASE_URL}/{local_rel}` where `local_rel` matches
//! the path under `~/.boris/models` (e.g. `parakeet/encoder-model.int8.onnx`,
//! `supertone/onnx/vocoder.onnx`, `supertone/voices/M4.json`). Host a mirror
//! with that tree for offline / private installs.
//!
//! Optional auth: `HF_TOKEN` or `HUGGING_FACE_HUB_TOKEN` is sent as a Bearer
//! token when present (helps with rate limits on large LFS blobs).
//!
//! LiveKit wake is **not** downloaded here — it is embedded in the desktop binary.
//!
//! # Filenames (required product set)
//!
//! **parakeet/** — `encoder-model.int8.onnx`, `decoder_joint-model.int8.onnx`,
//! `config.json`, `vocab.txt`, `nemo128.onnx`
//!
//! **supertone/onnx/** — `tts.json`, `vocoder.onnx`, `text_encoder.onnx`,
//! `vector_estimator.onnx`, `duration_predictor.onnx`, `unicode_indexer.json`
//!
//! **supertone/voices/** — `M4.json` (from upstream `voice_styles/M4.json`)

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::{
    boris_home, ensure_model_dirs, models_dir, parakeet_dir, parakeet_looks_ready,
    supertone_looks_ready, supertone_onnx_dir, supertone_onnx_is_multilingual,
    supertone_version_problem, supertone_voices_dir,
};

/// Env: override base URL; relative paths are appended (see module docs).
pub const BORIS_MODEL_BASE_URL_ENV: &str = "BORIS_MODEL_BASE_URL";

/// Hugging Face resolve base for Parakeet TDT 0.6b v2 int8 (onnx-asr layout).
pub const DEFAULT_PARAKEET_HF_BASE: &str =
    "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/main";

/// Hugging Face resolve base for Supertonic 3.
pub const DEFAULT_SUPERTONE_HF_BASE: &str =
    "https://huggingface.co/Supertone/supertonic-3/resolve/main";

/// Logical product component for progress reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelComponent {
    Parakeet,
    Supertone,
}

impl ModelComponent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Parakeet => "parakeet",
            Self::Supertone => "supertone",
        }
    }
}

/// Per-file progress / terminal state for one download step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadFileStatus {
    Starting,
    Downloading,
    Skipped,
    Done,
    Failed,
}

/// Progress event (also the payload for Tauri `models-progress`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub component: ModelComponent,
    /// Filename only (e.g. `encoder-model.int8.onnx`).
    pub file_name: String,
    /// Path relative to `~/.boris/models`.
    pub relative_path: String,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub status: DownloadFileStatus,
    #[serde(default)]
    pub message: Option<String>,
}

/// Snapshot for UI / `models_status` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsStatus {
    pub home: String,
    pub models_dir: String,
    pub parakeet_ready: bool,
    pub parakeet_dir: String,
    pub supertone_ready: bool,
    pub supertone_onnx_dir: String,
    pub supertone_voices_dir: String,
    /// Local relative paths still missing or too small.
    pub missing: Vec<String>,
    /// `BORIS_MODEL_BASE_URL` if set.
    pub base_url_override: Option<String>,
}

/// Aggregate result of [`install_models`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsInstallReport {
    pub ok: bool,
    pub parakeet_ready: bool,
    pub supertone_ready: bool,
    pub files_downloaded: u32,
    pub files_skipped: u32,
    pub files_failed: u32,
    pub errors: Vec<String>,
}

struct CatalogEntry {
    component: ModelComponent,
    /// Under `~/.boris/models/…`
    local_rel: &'static str,
    /// Path under the component's default HF base (not used when env override is set).
    default_remote_rel: &'static str,
    /// Minimum acceptable on-disk size (bytes) to treat as present.
    min_bytes: u64,
    default_base: &'static str,
}

/// Required product files + conservative min sizes (skip re-download if met).
const CATALOG: &[CatalogEntry] = &[
    // Parakeet (int8 onnx-asr)
    CatalogEntry {
        component: ModelComponent::Parakeet,
        local_rel: "parakeet/encoder-model.int8.onnx",
        default_remote_rel: "encoder-model.int8.onnx",
        min_bytes: 100_000_000, // ~652 MB
        default_base: DEFAULT_PARAKEET_HF_BASE,
    },
    CatalogEntry {
        component: ModelComponent::Parakeet,
        local_rel: "parakeet/decoder_joint-model.int8.onnx",
        default_remote_rel: "decoder_joint-model.int8.onnx",
        min_bytes: 1_000_000, // ~9 MB
        default_base: DEFAULT_PARAKEET_HF_BASE,
    },
    CatalogEntry {
        component: ModelComponent::Parakeet,
        local_rel: "parakeet/config.json",
        default_remote_rel: "config.json",
        min_bytes: 20,
        default_base: DEFAULT_PARAKEET_HF_BASE,
    },
    CatalogEntry {
        component: ModelComponent::Parakeet,
        local_rel: "parakeet/vocab.txt",
        default_remote_rel: "vocab.txt",
        min_bytes: 100,
        default_base: DEFAULT_PARAKEET_HF_BASE,
    },
    CatalogEntry {
        component: ModelComponent::Parakeet,
        local_rel: "parakeet/nemo128.onnx",
        default_remote_rel: "nemo128.onnx",
        min_bytes: 10_000, // ~140 KB
        default_base: DEFAULT_PARAKEET_HF_BASE,
    },
    // Supertone graphs
    CatalogEntry {
        component: ModelComponent::Supertone,
        local_rel: "supertone/onnx/tts.json",
        default_remote_rel: "onnx/tts.json",
        min_bytes: 100,
        default_base: DEFAULT_SUPERTONE_HF_BASE,
    },
    CatalogEntry {
        component: ModelComponent::Supertone,
        local_rel: "supertone/onnx/unicode_indexer.json",
        default_remote_rel: "onnx/unicode_indexer.json",
        min_bytes: 1_000,
        default_base: DEFAULT_SUPERTONE_HF_BASE,
    },
    CatalogEntry {
        component: ModelComponent::Supertone,
        local_rel: "supertone/onnx/duration_predictor.onnx",
        default_remote_rel: "onnx/duration_predictor.onnx",
        min_bytes: 100_000, // ~3.7 MB
        default_base: DEFAULT_SUPERTONE_HF_BASE,
    },
    CatalogEntry {
        component: ModelComponent::Supertone,
        local_rel: "supertone/onnx/text_encoder.onnx",
        default_remote_rel: "onnx/text_encoder.onnx",
        min_bytes: 1_000_000, // ~36 MB
        default_base: DEFAULT_SUPERTONE_HF_BASE,
    },
    CatalogEntry {
        component: ModelComponent::Supertone,
        local_rel: "supertone/onnx/vector_estimator.onnx",
        default_remote_rel: "onnx/vector_estimator.onnx",
        min_bytes: 10_000_000, // ~256 MB
        default_base: DEFAULT_SUPERTONE_HF_BASE,
    },
    CatalogEntry {
        component: ModelComponent::Supertone,
        local_rel: "supertone/onnx/vocoder.onnx",
        default_remote_rel: "onnx/vocoder.onnx",
        min_bytes: 10_000_000, // ~101 MB
        default_base: DEFAULT_SUPERTONE_HF_BASE,
    },
    // Voice (upstream dir is voice_styles/)
    CatalogEntry {
        component: ModelComponent::Supertone,
        local_rel: "supertone/voices/M4.json",
        default_remote_rel: "voice_styles/M4.json",
        min_bytes: 1_000, // ~290 KB
        default_base: DEFAULT_SUPERTONE_HF_BASE,
    },
];

fn file_name_of(rel: &str) -> String {
    Path::new(rel)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| rel.to_string())
}

fn file_ok(path: &Path, min_bytes: u64) -> bool {
    match fs::metadata(path) {
        Ok(m) if m.is_file() && m.len() >= min_bytes => true,
        _ => false,
    }
}

/// Remove installed Supertone files so Install models can fetch Supertonic 3.
fn purge_supertone_install(models_root: &Path) {
    let rels = [
        "supertone/onnx/tts.json",
        "supertone/onnx/tts.yml",
        "supertone/onnx/unicode_indexer.json",
        "supertone/onnx/duration_predictor.onnx",
        "supertone/onnx/text_encoder.onnx",
        "supertone/onnx/vector_estimator.onnx",
        "supertone/onnx/vocoder.onnx",
        "supertone/voices/M4.json",
    ];
    for rel in rels {
        let path = models_root.join(rel);
        if path.is_file() {
            match fs::remove_file(&path) {
                Ok(()) => tracing::info!(path = %path.display(), "removed outdated Supertone file"),
                Err(e) => tracing::warn!(path = %path.display(), error = %e, "failed to remove outdated Supertone file"),
            }
        }
    }
}

fn resolve_url(entry: &CatalogEntry, base_override: Option<&str>) -> String {
    if let Some(base) = base_override {
        let base = base.trim_end_matches('/');
        return format!("{base}/{}", entry.local_rel);
    }
    let base = entry.default_base.trim_end_matches('/');
    format!("{base}/{}", entry.default_remote_rel)
}

fn hf_token() -> Option<String> {
    std::env::var("HF_TOKEN")
        .or_else(|_| std::env::var("HUGGING_FACE_HUB_TOKEN"))
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// Current install readiness under `~/.boris/models`.
pub fn models_status() -> ModelsStatus {
    let pk = parakeet_dir();
    let onnx = supertone_onnx_dir();
    let voices = supertone_voices_dir();
    let base = models_dir();
    let supertone_ready = supertone_looks_ready(&onnx, &voices);
    // Wrong-generation weights pass size checks but must be treated as missing.
    let force_supertone = !supertone_ready && onnx.join("tts.json").is_file();

    let mut missing = Vec::new();
    for e in CATALOG {
        let path = base.join(e.local_rel);
        let wrong_gen = force_supertone && e.component == ModelComponent::Supertone;
        if wrong_gen || !file_ok(&path, e.min_bytes) {
            missing.push(e.local_rel.to_string());
        }
    }
    if let Some(problem) = supertone_version_problem(&onnx) {
        missing.push(format!("VERSION: {problem}"));
    }

    ModelsStatus {
        home: boris_home().display().to_string(),
        models_dir: base.display().to_string(),
        parakeet_ready: parakeet_looks_ready(&pk),
        parakeet_dir: pk.display().to_string(),
        supertone_ready,
        supertone_onnx_dir: onnx.display().to_string(),
        supertone_voices_dir: voices.display().to_string(),
        missing,
        base_url_override: std::env::var(BORIS_MODEL_BASE_URL_ENV)
            .ok()
            .filter(|s| !s.is_empty()),
    }
}

/// Download missing model files into `~/.boris/models`.
///
/// Skips files that already exist and pass the size check. Invokes `on_progress`
/// for each state change (and periodically while downloading large files).
pub fn install_models(
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<ModelsInstallReport, String> {
    ensure_model_dirs().map_err(|e| format!("create ~/.boris/models: {e}"))?;

    let base_override = std::env::var(BORIS_MODEL_BASE_URL_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty());
    if let Some(ref b) = base_override {
        tracing::info!(base = %b, "using BORIS_MODEL_BASE_URL for model install");
    }

    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        // Large models (~650 MB); no overall request timeout — stream until done.
        .timeout(None)
        .user_agent(concat!("boris-pipeline/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let root = models_dir();
    let onnx_dir = supertone_onnx_dir();
    // Old Supertonic 1 English-only graphs must be replaced (size checks pass).
    let reinstall_supertone = onnx_dir.join("tts.json").is_file()
        && !supertone_onnx_is_multilingual(&onnx_dir);
    if reinstall_supertone {
        if let Some(problem) = supertone_version_problem(&onnx_dir) {
            tracing::warn!(%problem, "reinstalling Supertone models from Hugging Face");
            on_progress(DownloadProgress {
                component: ModelComponent::Supertone,
                file_name: "tts.json".into(),
                relative_path: "supertone/onnx/tts.json".into(),
                bytes_downloaded: 0,
                total_bytes: None,
                status: DownloadFileStatus::Starting,
                message: Some(problem),
            });
        }
        purge_supertone_install(&root);
    }

    let mut downloaded = 0u32;
    let mut skipped = 0u32;
    let mut failed = 0u32;
    let mut errors = Vec::new();

    for entry in CATALOG {
        let dest = root.join(entry.local_rel);
        let file_name = file_name_of(entry.local_rel);
        let rel = entry.local_rel.to_string();

        if file_ok(&dest, entry.min_bytes) {
            skipped += 1;
            on_progress(DownloadProgress {
                component: entry.component,
                file_name: file_name.clone(),
                relative_path: rel.clone(),
                bytes_downloaded: fs::metadata(&dest).map(|m| m.len()).unwrap_or(0),
                total_bytes: None,
                status: DownloadFileStatus::Skipped,
                message: Some("already present".into()),
            });
            continue;
        }

        on_progress(DownloadProgress {
            component: entry.component,
            file_name: file_name.clone(),
            relative_path: rel.clone(),
            bytes_downloaded: 0,
            total_bytes: None,
            status: DownloadFileStatus::Starting,
            message: None,
        });

        let url = resolve_url(entry, base_override.as_deref());
        match download_one(&client, &url, &dest, entry, &mut on_progress) {
            Ok(bytes) => {
                downloaded += 1;
                on_progress(DownloadProgress {
                    component: entry.component,
                    file_name: file_name.clone(),
                    relative_path: rel.clone(),
                    bytes_downloaded: bytes,
                    total_bytes: Some(bytes),
                    status: DownloadFileStatus::Done,
                    message: None,
                });
            }
            Err(e) => {
                failed += 1;
                let msg = format!("{}: {e}", entry.local_rel);
                tracing::error!(error = %msg, "model download failed");
                errors.push(msg.clone());
                on_progress(DownloadProgress {
                    component: entry.component,
                    file_name,
                    relative_path: rel,
                    bytes_downloaded: 0,
                    total_bytes: None,
                    status: DownloadFileStatus::Failed,
                    message: Some(msg),
                });
            }
        }
    }

    let pk_ready = parakeet_looks_ready(&parakeet_dir());
    let st_ready = supertone_looks_ready(&supertone_onnx_dir(), &supertone_voices_dir());
    let ok = failed == 0 && pk_ready && st_ready;

    Ok(ModelsInstallReport {
        ok,
        parakeet_ready: pk_ready,
        supertone_ready: st_ready,
        files_downloaded: downloaded,
        files_skipped: skipped,
        files_failed: failed,
        errors,
    })
}

fn download_one(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    entry: &CatalogEntry,
    on_progress: &mut impl FnMut(DownloadProgress),
) -> Result<u64, String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }

    let tmp = temp_path(dest);
    // Clean any previous partial.
    let _ = fs::remove_file(&tmp);

    tracing::info!(
        component = entry.component.as_str(),
        url = %url,
        dest = %dest.display(),
        "downloading model file"
    );

    let mut req = client.get(url);
    if let Some(token) = hf_token() {
        req = req.bearer_auth(token);
    }

    let mut response = req.send().map_err(|e| format!("request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        let snippet: String = body.chars().take(200).collect();
        return Err(format!("HTTP {status}: {snippet}"));
    }

    let total = response.content_length();
    let mut out = File::create(&tmp).map_err(|e| format!("create temp: {e}"))?;

    let mut buf = [0u8; 64 * 1024];
    let mut downloaded: u64 = 0;
    let mut last_emit = 0u64;
    let file_name = file_name_of(entry.local_rel);
    let rel = entry.local_rel.to_string();

    // Immediate "downloading" so UI leaves the stuck "starting · 0 B" state
    // while the first multi-MB chunk arrives (or while waiting on a slow host).
    on_progress(DownloadProgress {
        component: entry.component,
        file_name: file_name.clone(),
        relative_path: rel.clone(),
        bytes_downloaded: 0,
        total_bytes: total,
        status: DownloadFileStatus::Downloading,
        message: None,
    });

    loop {
        let n = response
            .read(&mut buf)
            .map_err(|e| format!("read body: {e}"))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])
            .map_err(|e| format!("write temp: {e}"))?;
        downloaded += n as u64;

        // First chunk always, then ~every 512 KiB (responsive without flooding).
        let first = last_emit == 0;
        if first || downloaded - last_emit >= 512 * 1024 || total == Some(downloaded) {
            last_emit = downloaded;
            on_progress(DownloadProgress {
                component: entry.component,
                file_name: file_name.clone(),
                relative_path: rel.clone(),
                bytes_downloaded: downloaded,
                total_bytes: total,
                status: DownloadFileStatus::Downloading,
                message: None,
            });
        }
    }

    out.flush().map_err(|e| format!("flush: {e}"))?;
    drop(out);

    if downloaded < entry.min_bytes {
        let _ = fs::remove_file(&tmp);
        return Err(format!(
            "downloaded {downloaded} bytes, expected at least {}",
            entry.min_bytes
        ));
    }

    // Replace destination atomically where possible.
    if dest.exists() {
        let _ = fs::remove_file(dest);
    }
    if let Err(e) = fs::rename(&tmp, dest) {
        // Cross-device fallback: copy then remove.
        fs::copy(&tmp, dest).map_err(|e2| {
            let _ = fs::remove_file(&tmp);
            format!("finalize file: rename {e} / copy {e2}")
        })?;
        let _ = fs::remove_file(&tmp);
    }

    if !dest.is_file() {
        return Err("finalize failed: destination missing after write".into());
    }

    Ok(downloaded)
}

fn temp_path(dest: &Path) -> PathBuf {
    let mut name = dest
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_else(|| "model.bin".into());
    name.push(".part");
    dest.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_local_paths_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for e in CATALOG {
            assert!(seen.insert(e.local_rel), "dup {}", e.local_rel);
        }
    }

    #[test]
    fn resolve_uses_override_layout() {
        let e = &CATALOG[0];
        let u = resolve_url(e, Some("https://cdn.example/models"));
        assert_eq!(
            u,
            "https://cdn.example/models/parakeet/encoder-model.int8.onnx"
        );
    }

    #[test]
    fn resolve_default_hf() {
        let e = &CATALOG[0];
        let u = resolve_url(e, None);
        assert!(u.contains("parakeet-tdt-0.6b-v2-onnx"));
        assert!(u.ends_with("/encoder-model.int8.onnx"));
    }
}
