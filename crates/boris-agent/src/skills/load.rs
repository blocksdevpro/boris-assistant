//! Discover and parse skills from project / user / extra paths.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::frontmatter::{is_valid_name, parse_frontmatter, strip_frontmatter};
use super::{LoadedSkills, Skill, SkillDiagnostic, SkillSource};

/// Parse one `SKILL.md` path into a [`Skill`].
pub fn parse_skill_file(path: &Path, source: SkillSource) -> Result<Skill, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;

    let (name, description) = parse_frontmatter(&content)
        .ok_or_else(|| "missing or incomplete frontmatter (need name + description)".to_string())?;

    if !is_valid_name(&name) {
        return Err(format!(
            "invalid skill name '{name}' (use lowercase, digits, hyphens)"
        ));
    }

    if let Some(parent) = path.parent() {
        if let Some(dir_name) = parent.file_name().and_then(|n| n.to_str()) {
            if dir_name != name {
                return Err(format!(
                    "skill name '{name}' does not match directory '{dir_name}'"
                ));
            }
        }
    }

    let base_dir = path.parent().unwrap_or(path).to_path_buf();
    Ok(Skill {
        name,
        description,
        file_path: path.to_path_buf(),
        base_dir,
        source,
    })
}

fn scan_skills_dir(dir: &Path, source: SkillSource) -> (Vec<Skill>, Vec<SkillDiagnostic>) {
    let mut skills = Vec::new();
    let mut diagnostics = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (skills, diagnostics);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_file = path.join("SKILL.md");
        if !skill_file.is_file() {
            continue;
        }
        match parse_skill_file(&skill_file, source) {
            Ok(skill) => skills.push(skill),
            Err(msg) => diagnostics.push(SkillDiagnostic {
                message: msg,
                path: skill_file,
            }),
        }
    }
    (skills, diagnostics)
}

/// Walk up from `cwd` collecting `.boris/skills/` directories (stop at git root).
pub fn project_skills_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut current = Some(cwd);
    while let Some(dir) = current {
        let candidate = dir.join(".boris").join("skills");
        if candidate.is_dir() {
            dirs.push(candidate);
        }
        if dir.join(".git").exists() {
            break;
        }
        current = dir.parent();
    }
    dirs
}

/// User-global skills directory (`~/.boris/skills` or under `boris_home`).
pub fn user_skills_dir(boris_home: &Path) -> PathBuf {
    boris_home.join("skills")
}

/// Discover skills. First name wins (project → user → extras → bundled path).
pub fn load_skills(
    cwd: Option<&Path>,
    boris_home: &Path,
    extra_paths: &[PathBuf],
    include_user: bool,
) -> LoadedSkills {
    let mut all = LoadedSkills::default();
    let mut seen: HashSet<String> = HashSet::new();

    let mut add = |skills: Vec<Skill>, diags: Vec<SkillDiagnostic>| {
        for skill in skills {
            if seen.contains(&skill.name) {
                continue;
            }
            seen.insert(skill.name.clone());
            all.skills.push(skill);
        }
        all.diagnostics.extend(diags);
    };

    if let Some(cwd) = cwd {
        for dir in project_skills_dirs(cwd) {
            let (s, d) = scan_skills_dir(&dir, SkillSource::Project);
            add(s, d);
        }
    }

    if include_user {
        let user_dir = user_skills_dir(boris_home);
        if user_dir.is_dir() {
            let (s, d) = scan_skills_dir(&user_dir, SkillSource::User);
            add(s, d);
        }
    }

    for path in extra_paths {
        let path = if path.is_dir() {
            path.join("SKILL.md")
        } else {
            path.clone()
        };
        if path.is_file() {
            match parse_skill_file(&path, SkillSource::Extra) {
                Ok(skill) => {
                    if !seen.contains(&skill.name) {
                        seen.insert(skill.name.clone());
                        all.skills.push(skill);
                    }
                }
                Err(msg) => all.diagnostics.push(SkillDiagnostic { message: msg, path }),
            }
        }
    }

    // Stable order for prompts/tests.
    all.skills.sort_by(|a, b| a.name.cmp(&b.name));
    all
}

/// Load full skill body (frontmatter stripped) for tool observation.
pub fn load_skill_body(skill: &Skill) -> Result<String, String> {
    let content = std::fs::read_to_string(&skill.file_path)
        .map_err(|e| format!("failed to read {}: {e}", skill.file_path.display()))?;
    let body = strip_frontmatter(&content).trim();
    if body.is_empty() {
        return Err(format!("skill '{}' has empty body", skill.name));
    }
    // Grok-style envelope: name + description + path attributes, body inside.
    Ok(format!(
        "<skill name=\"{}\" description=\"{}\" path=\"{}\">\n\
         Base directory for relative refs: {}\n\
         Source: {:?}\n\n\
         {}\n\
         </skill>\n\n\
         Follow this skill's steps using your tools. Keep spoken replies short (1–2 sentences). \
         Work autonomously until the skill goal is done or you need a real user decision.",
        skill.name,
        skill.description.replace('"', "'"),
        skill.file_path.display(),
        skill.base_dir.display(),
        skill.source,
        body
    ))
}
