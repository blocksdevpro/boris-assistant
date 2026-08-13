//! Windows-safe artifact slugs, language extensions, and on-disk names.

/// Max characters kept from the title when building a slug.
pub const MAX_SLUG_CHARS: usize = 60;

/// Device names that cannot be a Windows filename stem (case-insensitive).
const RESERVED_STEMS: &[&str] = &[
    "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
    "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Lowercase ASCII slug for a card title (`Rename photos` → `rename-photos`).
///
/// Empty / punctuation-only titles become `card`. Reserved Windows device
/// names are prefixed (`CON` → `card-con`) so Explorer can open the file.
pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in title.chars() {
        if out.chars().count() >= MAX_SLUG_CHARS {
            break;
        }
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if matches!(c, ' ' | '-' | '_' | '.' | '/' | '\\') && !out.is_empty() && !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let slug = out.trim_matches('-');
    let slug = if slug.is_empty() { "card" } else { slug };
    if RESERVED_STEMS.contains(&slug) {
        format!("card-{slug}")
    } else {
        slug.to_string()
    }
}

/// File extension for an artifact (no leading dot). Markdown is always `md`.
pub fn extension_for(kind: super::ArtifactKind, language: Option<&str>) -> &'static str {
    if matches!(kind, super::ArtifactKind::Markdown) {
        return "md";
    }
    language_extension(language.unwrap_or(""))
}

/// Map a code-language hint to a short extension. Unknown → `txt`.
pub fn language_extension(language: &str) -> &'static str {
    match language.trim().to_ascii_lowercase().as_str() {
        "rust" | "rs" => "rs",
        "python" | "py" => "py",
        "javascript" | "js" | "node" => "js",
        "typescript" | "ts" => "ts",
        "tsx" => "tsx",
        "jsx" => "jsx",
        "powershell" | "ps1" | "pwsh" => "ps1",
        "bash" | "sh" | "shell" | "zsh" => "sh",
        "json" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yml",
        "html" => "html",
        "css" => "css",
        "go" | "golang" => "go",
        "c" => "c",
        "cpp" | "c++" | "cxx" => "cpp",
        "java" => "java",
        "csharp" | "cs" | "c#" => "cs",
        "ruby" | "rb" => "rb",
        "swift" => "swift",
        "kotlin" | "kt" => "kt",
        "sql" => "sql",
        "xml" => "xml",
        "markdown" | "md" => "md",
        "text" | "txt" | "plaintext" | "plain" => "txt",
        _ => "txt",
    }
}

/// On-disk name: `{slug}-{id}.{ext}` (e.g. `rename-photos-a1f3c9.ps1`).
pub fn artifact_filename(slug: &str, id: &str, ext: &str) -> String {
    format!("{slug}-{id}.{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::artifacts::ArtifactKind;

    #[test]
    fn slugify_basic_title() {
        assert_eq!(slugify("Rename photos"), "rename-photos");
        assert_eq!(slugify("  Weekly Meal Plan  "), "weekly-meal-plan");
    }

    #[test]
    fn slugify_strips_punctuation_and_collapses_dashes() {
        assert_eq!(slugify("Hello, world!!"), "hello-world");
        assert_eq!(slugify("a   b___c"), "a-b-c");
    }

    #[test]
    fn slugify_empty_and_reserved() {
        assert_eq!(slugify(""), "card");
        assert_eq!(slugify("???"), "card");
        assert_eq!(slugify("CON"), "card-con");
        assert_eq!(slugify("nul"), "card-nul");
        assert_eq!(slugify("COM1"), "card-com1");
    }

    #[test]
    fn slugify_caps_length() {
        let long = "a".repeat(80);
        assert_eq!(slugify(&long).chars().count(), MAX_SLUG_CHARS);
    }

    #[test]
    fn extension_markdown_ignores_language() {
        assert_eq!(extension_for(ArtifactKind::Markdown, Some("rust")), "md");
        assert_eq!(extension_for(ArtifactKind::Markdown, None), "md");
    }

    #[test]
    fn extension_code_languages() {
        assert_eq!(extension_for(ArtifactKind::Code, Some("powershell")), "ps1");
        assert_eq!(extension_for(ArtifactKind::Code, Some("Rust")), "rs");
        assert_eq!(extension_for(ArtifactKind::Code, Some("nope")), "txt");
        assert_eq!(extension_for(ArtifactKind::Code, None), "txt");
    }

    #[test]
    fn filename_includes_slug_and_id() {
        assert_eq!(
            artifact_filename("rename-photos", "a1f3c9", "ps1"),
            "rename-photos-a1f3c9.ps1"
        );
    }
}
