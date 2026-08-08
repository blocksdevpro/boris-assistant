//! Skill discovery and progressive disclosure (Grok / tau style).
//!
//! Skills live as `SKILL.md` files with YAML frontmatter:
//!
//! ```text
//! ~/.boris/skills/<name>/SKILL.md     # user-global
//! <project>/.boris/skills/<name>/SKILL.md  # project-local
//! ```
//!
//! Only name + description go into the system prompt catalog. Full bodies are
//! loaded on demand via [`crate::tools::skills_tools`] tools so the model can run
//! multi-step playbooks without stuffing every skill into every turn.
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`frontmatter`] | parse/strip YAML frontmatter, name validation |
//! | [`load`]        | scan dirs, discover, load body envelope |
//! | [`catalog`]     | system-prompt skill list |
//! | [`defaults`]    | bundled starter `SKILL.md` installer |

mod catalog;
mod defaults;
mod frontmatter;
mod load;

use std::path::PathBuf;

pub use catalog::format_skills_catalog;
pub use defaults::ensure_default_skills;
pub use frontmatter::{is_valid_name, strip_frontmatter};
pub use load::{
    load_skill_body, load_skills, parse_skill_file, project_skills_dirs, user_skills_dir,
};

/// Where a skill was discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    Project,
    User,
    Bundled,
    Extra,
}

/// A discovered skill (metadata only until body is loaded).
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub file_path: PathBuf,
    pub base_dir: PathBuf,
    pub source: SkillSource,
}

#[derive(Debug, Clone)]
pub struct SkillDiagnostic {
    pub message: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct LoadedSkills {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

impl LoadedSkills {
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    pub fn names(&self) -> Vec<&str> {
        self.skills.iter().map(|s| s.name.as_str()).collect()
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn load_and_catalog() {
        let dir = std::env::temp_dir().join(format!("boris-skills-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        ensure_default_skills(&dir).unwrap();
        let loaded = load_skills(None, &dir, &[], true);
        assert!(loaded.skills.len() >= 3, "got {:?}", loaded.names());
        assert!(loaded.get("research").is_some());
        let cat = format_skills_catalog(&loaded.skills);
        assert!(cat.contains("research"));
        assert!(cat.contains("load_skill"));
        let body = load_skill_body(loaded.get("research").unwrap()).unwrap();
        assert!(body.contains("web_search"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn name_must_match_dir() {
        let dir = std::env::temp_dir().join(format!("boris-skills-mm-{}", std::process::id()));
        let skill_dir = dir.join("wrong");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: right\ndescription: x\n---\nbody",
        )
        .unwrap();
        assert!(parse_skill_file(&path, SkillSource::User).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
