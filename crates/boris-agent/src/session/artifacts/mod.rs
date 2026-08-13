//! Session-scoped visual cards (`{session_dir}/artifacts/`).
//!
//! Bodies live as named files (`{slug}-{id}.{ext}`). The catalog is
//! `index.json`. Chat history only stores a short tool receipt.

mod id;
mod slug;
mod store;

pub use id::{generate_artifact_id, is_artifact_id, normalize_artifact_id, ARTIFACT_ID_LEN};
pub use slug::{artifact_filename, extension_for, language_extension, slugify, MAX_SLUG_CHARS};
pub use store::ArtifactStore;

use serde::{Deserialize, Serialize};

/// Hard cap on a single card body (characters).
pub const MAX_ARTIFACT_BODY_CHARS: usize = 64_000;

/// Hard cap on stored cards per session.
pub const MAX_ARTIFACTS: usize = 100;

/// Title clip before slug + catalog (characters).
pub const MAX_TITLE_CHARS: usize = 80;

/// What the card renderer / file extension should treat this as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Markdown,
    Code,
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Code => "code",
        }
    }

    /// Parse `markdown` / `md` / `code`. Unknown → `None`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "markdown" | "md" | "text" => Some(Self::Markdown),
            "code" | "source" => Some(Self::Code),
            _ => None,
        }
    }
}

/// One card in the session catalog (`index.json` item).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactMeta {
    pub id: String,
    pub title: String,
    pub kind: ArtifactKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Filename under the artifacts directory (`rename-photos-a1f3c9.ps1`).
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub revision: u32,
}

/// On-disk `{artifacts}/index.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ArtifactIndex {
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub items: Vec<ArtifactMeta>,
}

impl ArtifactIndex {
    pub fn get(&self, id: &str) -> Option<&ArtifactMeta> {
        self.items.iter().find(|m| m.id == id)
    }
}

/// Input to [`ArtifactStore::present`].
#[derive(Debug, Clone)]
pub struct PresentRequest {
    /// Existing id to revise. `None` creates a new card.
    pub id: Option<String>,
    pub title: String,
    pub kind: ArtifactKind,
    pub language: Option<String>,
    pub body: String,
    pub turn_id: Option<String>,
    pub pinned: Option<bool>,
}

/// Result of a successful present (create or revise).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedArtifact {
    pub meta: ArtifactMeta,
    pub created: bool,
}
