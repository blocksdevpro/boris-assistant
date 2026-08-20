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
//! Default remote sources are immutable Hugging Face revisions:
//!
//! | Component  | Default base (per-file absolute URLs below) |
//! |------------|-----------------------------------------------|
//! | Parakeet   | `istupakov/parakeet-tdt-0.6b-v2-onnx` @ `0bbb45a3365852604aef28b538a8f066f4ccaa85` |
//! | Supertone  | `Supertone/supertonic-3` @ `724fb5abbf5502583fb520898d45929e62f02c0b` |
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
//! # Integrity verification
//!
//! Each [`CatalogEntry`] has a `min_bytes` floor and a required pinned SHA-256
//! digest. **Downloads** are hashed after the size check and before they replace
//! an existing file; a mismatch deletes the temp file and fails that entry.
//!
//! **UI status / skip decisions** use size + path presence only (plus the
//! Supertonic generation check). Full SHA-256 over ~1 GB of weights is reserved
//! for the download path — re-hashing on every `models_status` poll froze the
//! desktop main window for tens of seconds ("Not Responding").
//!
//! `BORIS_MODEL_BASE_URL` must use `https://`. The SHA-256 verification is
//! still required for mirrors, so a TLS-valid but malicious or stale mirror
//! cannot substitute a model payload.
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
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{PipelineError, Result as PipelineResult};
use crate::paths::{
    boris_home, ensure_model_dirs, models_dir, parakeet_dir, parakeet_looks_ready,
    supertone_looks_ready, supertone_onnx_dir, supertone_onnx_is_multilingual,
    supertone_version_problem, supertone_voices_dir,
};

/// Env: override base URL; relative paths are appended (see module docs).
pub const BORIS_MODEL_BASE_URL_ENV: &str = "BORIS_MODEL_BASE_URL";

/// Immutable Hugging Face revision for Parakeet TDT 0.6b v2 int8.
pub const PARAKEET_HF_REVISION: &str = "0bbb45a3365852604aef28b538a8f066f4ccaa85";

/// Hugging Face resolve base for the pinned Parakeet TDT 0.6b v2 int8 revision.
pub const DEFAULT_PARAKEET_HF_BASE: &str =
    "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/0bbb45a3365852604aef28b538a8f066f4ccaa85";

/// Immutable Hugging Face revision for Supertonic 3.
pub const SUPERTONE_HF_REVISION: &str = "724fb5abbf5502583fb520898d45929e62f02c0b";

/// Hugging Face resolve base for the pinned Supertonic 3 revision.
pub const DEFAULT_SUPERTONE_HF_BASE: &str =
    "https://huggingface.co/Supertone/supertonic-3/resolve/724fb5abbf5502583fb520898d45929e62f02c0b";

/// TCP connect budget for each model HTTP request.
const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Per-operation / idle budget for headers and each body read.
///
/// Blocking reqwest applies this to `send()` and to each `Read` on the body, so
/// a stalled connection fails without capping total time for multi‑hundred‑MB
/// files that keep making progress.
const DOWNLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

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
    /// Expected SHA-256 hex digest of the downloaded file. A mismatch is a
    /// hard failure: the partial/mismatched file is deleted rather than left
    /// on disk to be picked up by a later run.
    sha256: &'static str,
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
        // Git-LFS SHA-256 at PARAKEET_HF_REVISION.
        sha256: "3e0581fda6ab843888b51e56d7ee78b6d5bc3237ec113af1f732d1d5286aa155",
    },
    CatalogEntry {
        component: ModelComponent::Parakeet,
        local_rel: "parakeet/decoder_joint-model.int8.onnx",
        default_remote_rel: "decoder_joint-model.int8.onnx",
        min_bytes: 1_000_000, // ~9 MB
        default_base: DEFAULT_PARAKEET_HF_BASE,
        // Git-LFS SHA-256 at PARAKEET_HF_REVISION.
        sha256: "a449f49acd68979d418651dd2dcb737cc0f1bf0225e009e29ee326354edbf7d3",
    },
    CatalogEntry {
        component: ModelComponent::Parakeet,
        local_rel: "parakeet/config.json",
        default_remote_rel: "config.json",
        min_bytes: 20,
        default_base: DEFAULT_PARAKEET_HF_BASE,
        sha256: "666903c76b9798caf2c210afd4f6cd60b08a8dbf9800ec8d7a3bc0d2148ac466",
    },
    CatalogEntry {
        component: ModelComponent::Parakeet,
        local_rel: "parakeet/vocab.txt",
        default_remote_rel: "vocab.txt",
        min_bytes: 100,
        default_base: DEFAULT_PARAKEET_HF_BASE,
        sha256: "ec182b70dd42113aff6c5372c75cac58c952443eb22322f57bbd7f53977d497d",
    },
    CatalogEntry {
        component: ModelComponent::Parakeet,
        local_rel: "parakeet/nemo128.onnx",
        default_remote_rel: "nemo128.onnx",
        min_bytes: 10_000, // ~140 KB
        default_base: DEFAULT_PARAKEET_HF_BASE,
        // Git-LFS SHA-256 at PARAKEET_HF_REVISION.
        sha256: "a9fde1486ebfcc08f328d75ad4610c67835fea58c73ba57e3209a6f6cf019e9f",
    },
    // Supertone graphs
    CatalogEntry {
        component: ModelComponent::Supertone,
        local_rel: "supertone/onnx/tts.json",
        default_remote_rel: "onnx/tts.json",
        min_bytes: 100,
        default_base: DEFAULT_SUPERTONE_HF_BASE,
        sha256: "42078d3aef1cd43ab43021f3c54f47d2d75ceb4e75f627f118890128b06a0d09",
    },
    CatalogEntry {
        component: ModelComponent::Supertone,
        local_rel: "supertone/onnx/unicode_indexer.json",
        default_remote_rel: "onnx/unicode_indexer.json",
        min_bytes: 1_000,
        default_base: DEFAULT_SUPERTONE_HF_BASE,
        sha256: "9bf7346e43883a81f8645c81224f786d43c5b57f3641f6e7671a7d6c493cb24f",
    },
    CatalogEntry {
        component: ModelComponent::Supertone,
        local_rel: "supertone/onnx/duration_predictor.onnx",
        default_remote_rel: "onnx/duration_predictor.onnx",
        min_bytes: 100_000, // ~3.7 MB
        default_base: DEFAULT_SUPERTONE_HF_BASE,
        // Git-LFS SHA-256 at SUPERTONE_HF_REVISION.
        sha256: "c3eb91414d5ff8a7a239b7fe9e34e7e2bf8a8140d8375ffb14718b1c639325db",
    },
    CatalogEntry {
        component: ModelComponent::Supertone,
        local_rel: "supertone/onnx/text_encoder.onnx",
        default_remote_rel: "onnx/text_encoder.onnx",
        min_bytes: 1_000_000, // ~36 MB
        default_base: DEFAULT_SUPERTONE_HF_BASE,
        // Git-LFS SHA-256 at SUPERTONE_HF_REVISION.
        sha256: "c7befd5ea8c3119769e8a6c1486c4edc6a3bc8365c67621c881bbb774b9902ff",
    },
    CatalogEntry {
        component: ModelComponent::Supertone,
        local_rel: "supertone/onnx/vector_estimator.onnx",
        default_remote_rel: "onnx/vector_estimator.onnx",
        min_bytes: 10_000_000, // ~256 MB
        default_base: DEFAULT_SUPERTONE_HF_BASE,
        // Git-LFS SHA-256 at SUPERTONE_HF_REVISION.
        sha256: "883ac868ea0275ef0e991524dc64f16b3c0376efd7c320af6b53f5b780d7c61c",
    },
    CatalogEntry {
        component: ModelComponent::Supertone,
        local_rel: "supertone/onnx/vocoder.onnx",
        default_remote_rel: "onnx/vocoder.onnx",
        min_bytes: 10_000_000, // ~101 MB
        default_base: DEFAULT_SUPERTONE_HF_BASE,
        // Git-LFS SHA-256 at SUPERTONE_HF_REVISION.
        sha256: "085de76dd8e8d5836d6ca66826601f615939218f90e519f70ee8a36ed2a4c4ba",
    },
    // Voice (upstream dir is voice_styles/)
    CatalogEntry {
        component: ModelComponent::Supertone,
        local_rel: "supertone/voices/M4.json",
        default_remote_rel: "voice_styles/M4.json",
        min_bytes: 1_000, // ~290 KB
        default_base: DEFAULT_SUPERTONE_HF_BASE,
        sha256: "ca8eefad4fcd989c9379032ff3e50738adc547eeb5e221b82593a6d7b3bac303",
    },
];

fn file_name_of(rel: &str) -> String {
    Path::new(rel)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| rel.to_string())
}

/// Fast presence check: file exists and meets the catalog minimum size.
///
/// Used by [`models_status`] and install **skip** decisions so the UI never
/// re-reads multi-hundred-MB weights. Integrity (SHA-256) is enforced when a
/// file is freshly downloaded — see [`download_one`].
fn file_size_ok(path: &Path, entry: &CatalogEntry) -> bool {
    matches!(
        fs::metadata(path),
        Ok(m) if m.is_file() && m.len() >= entry.min_bytes
    )
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
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "failed to remove outdated Supertone file")
                }
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

/// `BORIS_MODEL_BASE_URL` must be an absolute HTTPS URL when set.
///
/// Model hashes protect the payload, but HTTPS also authenticates the source
/// and keeps model requests private. Local/offline mirrors must terminate TLS;
/// HTTP is deliberately unsupported in release builds.
fn validate_model_base_url(base: &str) -> Result<(), String> {
    let base = base.trim();
    if base.starts_with("https://") {
        return Ok(());
    }
    let preview: String = base.chars().take(80).collect();
    Err(format!(
        "{BORIS_MODEL_BASE_URL_ENV} must be an https:// URL, got '{preview}'"
    ))
}

fn format_download_error(context: &str, err: impl std::fmt::Display) -> String {
    let msg = err.to_string();
    // reqwest / hyper timeouts surface as "timed out" / "operation timed out".
    let lower = msg.to_ascii_lowercase();
    if lower.contains("timed out") || lower.contains("timeout") {
        format!("{context}: timed out (stalled or too slow): {msg}")
    } else {
        format!("{context}: {msg}")
    }
}

fn hf_token() -> Option<String> {
    std::env::var("HF_TOKEN")
        .or_else(|_| std::env::var("HUGGING_FACE_HUB_TOKEN"))
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// Current install readiness under `~/.boris/models`.
///
/// Cheap by design: metadata + min-size + Supertonic generation checks only.
/// Does **not** SHA-256 the weight files (that would block the desktop UI for
/// tens of seconds on a full install).
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
        if wrong_gen || !file_size_ok(&path, e) {
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
/// Skips files that already meet the size floor. Fresh downloads are SHA-256
/// verified before they replace the destination. Invokes `on_progress` for
/// each state change (and periodically while downloading large files).
pub fn install_models(
    mut on_progress: impl FnMut(DownloadProgress),
) -> PipelineResult<ModelsInstallReport> {
    ensure_model_dirs()
        .map_err(|e| PipelineError::download(format!("create ~/.boris/models: {e}")))?;

    let base_override = std::env::var(BORIS_MODEL_BASE_URL_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty());
    if let Some(ref b) = base_override {
        validate_model_base_url(b).map_err(PipelineError::download)?;
        tracing::info!(base = %b, "using BORIS_MODEL_BASE_URL for model install");
    }

    let client = reqwest::blocking::Client::builder()
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        // Idle/per-read budget so a hung peer cannot block install forever.
        // Progressing multi‑hundred‑MB downloads keep resetting this on each chunk.
        .timeout(DOWNLOAD_IDLE_TIMEOUT)
        .user_agent(concat!("boris-pipeline/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| PipelineError::download(format!("http client: {e}")))?;

    let root = models_dir();
    let onnx_dir = supertone_onnx_dir();
    // Old Supertonic 1 English-only graphs must be replaced (size checks pass).
    let reinstall_supertone =
        onnx_dir.join("tts.json").is_file() && !supertone_onnx_is_multilingual(&onnx_dir);
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

        // Size-only skip: do not re-hash multi-hundred-MB weights that already
        // passed verify on the download that wrote them. Re-hashing here made
        // "Install models" appear hung for ~20–30s on a complete install.
        if file_size_ok(&dest, entry) {
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

    let mut response = req
        .send()
        .map_err(|e| format_download_error("request failed", e))?;

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
        let n = response.read(&mut buf).map_err(|e| {
            format_download_error(&format!("read body after {downloaded} bytes"), e)
        })?;
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

    if let Err(error) = verify_sha256(&tmp, entry.sha256) {
        let _ = fs::remove_file(&tmp);
        return Err(error);
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

/// Compute the SHA-256 hex digest of `path` and compare (case-insensitively)
/// against `expected`. On mismatch, returns an error describing both digests
/// (the caller is responsible for deleting the bad file).
fn verify_sha256(path: &Path, expected: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};

    let mut f = File::open(path).map_err(|e| format!("open for hash check: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("read for hash check: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "sha256 mismatch: expected {expected}, got {actual} — file discarded (possible \
             tampered mirror or corrupted download)"
        ))
    }
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

    #[test]
    fn base_url_must_be_https() {
        assert!(validate_model_base_url("https://cdn.example/models").is_ok());
        assert!(validate_model_base_url("http://localhost:8080/m").is_err());
        assert!(validate_model_base_url("ftp://bad").is_err());
        assert!(validate_model_base_url("file:///tmp").is_err());
        assert!(validate_model_base_url("not-a-url").is_err());
    }

    #[test]
    fn timeout_errors_are_labeled_for_install_report() {
        let msg =
            format_download_error("request failed", "error sending request for url: timed out");
        assert!(msg.contains("timed out"), "{msg}");
        assert!(msg.contains("stalled"), "{msg}");
    }

    fn temp_file_with(bytes: &[u8], suffix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "boris-dl-hash-test-{}-{suffix}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn verify_sha256_accepts_matching_digest() {
        let path = temp_file_with(b"hello world", "ok");
        // Well-known test vector: sha256("hello world")
        let expected = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        let result = verify_sha256(&path, expected);
        let _ = fs::remove_file(&path);
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn verify_sha256_accepts_case_insensitive_digest() {
        let path = temp_file_with(b"hello world", "ok-upper");
        let expected = "B94D27B9934D3E08A52E52D7DA7DABFAC484EFE37A5380EE9088F7ACE2EFCDE9";
        let result = verify_sha256(&path, expected);
        let _ = fs::remove_file(&path);
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn verify_sha256_rejects_mismatched_digest_and_caller_deletes_file() {
        let path = temp_file_with(b"totally not the expected model bytes", "bad");
        let bogus_expected = "0000000000000000000000000000000000000000000000000000000000000000";
        let result = verify_sha256(&path, bogus_expected);
        assert!(result.is_err(), "expected mismatch to be rejected");
        let err = result.unwrap_err();
        assert!(err.contains("mismatch"), "{err}");
        // download_one deletes the tmp file itself on mismatch; verify_sha256 does not
        // touch the file, so simulate that half of the contract here.
        let _ = fs::remove_file(&path);
        assert!(!path.exists());
    }

    #[test]
    fn file_size_ok_requires_min_bytes_not_hash() {
        let entry = &CATALOG[CATALOG.len() - 1]; // M4.json, min 1_000
        let path = temp_file_with(&vec![b'x'; 2_000], "size-ok");
        assert!(file_size_ok(&path, entry));
        let _ = fs::remove_file(&path);

        let too_small = temp_file_with(b"tiny", "size-small");
        assert!(!file_size_ok(&too_small, entry));
        let _ = fs::remove_file(&too_small);

        let missing = std::env::temp_dir().join("boris-dl-size-missing-no-such-file");
        let _ = fs::remove_file(&missing);
        assert!(!file_size_ok(&missing, entry));
    }

    #[test]
    fn models_status_is_metadata_only_and_returns() {
        // Must not hang hashing real weights; empty/partial trees are fine.
        let status = models_status();
        assert!(!status.home.is_empty());
        assert!(!status.models_dir.is_empty());
        // Ready flags are independent of catalog hash work.
        let _ = (
            status.parakeet_ready,
            status.supertone_ready,
            status.missing,
        );
    }
}
