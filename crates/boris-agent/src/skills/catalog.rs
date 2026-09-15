//! Progressive-disclosure skill catalog for the system prompt.

use super::Skill;

/// Progressive-disclosure catalog for the system prompt.
pub fn format_skills_catalog(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "<skills>\n\
         You have reusable skill playbooks for specialized work. Load one only when the user's \
         intent matches its description. A mentioned keyword alone is not a match.\n\
         Do not invent skills that are not listed. User intent and scope remain authoritative.\n\n\
         Available skills:\n",
    );
    for s in skills {
        out.push_str(&format!("- **{}**: {}\n", s.name, s.description));
    }
    out.push_str(
        "\nWhen a skill applies, call load_skill with its name and use only the relevant guidance. \
         Loading a skill does not require todos, research, artifacts, or extra work.\n\
         </skills>",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::SkillSource;
    use std::path::PathBuf;

    #[test]
    fn empty_catalog() {
        assert!(format_skills_catalog(&[]).is_empty());
    }

    #[test]
    fn catalog_lists_name_and_load_hint() {
        let skills = vec![Skill {
            name: "research".into(),
            description: "Look things up".into(),
            file_path: PathBuf::from("/tmp/research/SKILL.md"),
            base_dir: PathBuf::from("/tmp/research"),
            source: SkillSource::User,
        }];
        let cat = format_skills_catalog(&skills);
        assert!(cat.contains("research"));
        assert!(cat.contains("Look things up"));
        assert!(cat.contains("load_skill"));
    }
}
