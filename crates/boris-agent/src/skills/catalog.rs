//! Progressive-disclosure skill catalog for the system prompt.

use super::Skill;

/// Progressive-disclosure catalog for the system prompt.
pub fn format_skills_catalog(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "<skills>\n\
         You have reusable skill playbooks for multi-step work. Each skill is a workflow: \
         load it when the user's request matches its description, then follow its steps with tools.\n\
         Do not invent skills that are not listed. Prefer loading a skill over freestyling complex work.\n\n\
         Available skills:\n",
    );
    for s in skills {
        out.push_str(&format!("- **{}**: {}\n", s.name, s.description));
    }
    out.push_str(
        "\nWhen a skill applies: call load_skill with its name first, then execute the steps. \
         Use todo_write for multi-step tracking when the skill is long.\n\
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
