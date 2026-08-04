//! Tools for progressive skill discovery and on-demand loading.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::skills::{self, LoadedSkills, Skill};
use crate::tool::{
    optional_string, require_object, require_string, truncate_tool_result, truncate_tool_result_to,
    Tool, ToolError, ToolKind, ToolMeta, ToolRisk, MAX_SKILL_RESULT_CHARS,
};

/// Shared skill registry for tools (same Arc the Agent holds).
pub type SharedSkills = Arc<Mutex<LoadedSkills>>;

/// List available skills (name + description).
pub struct ListSkillsTool {
    skills: SharedSkills,
}

impl ListSkillsTool {
    pub fn new(skills: SharedSkills) -> Self {
        Self { skills }
    }
}

#[async_trait]
impl Tool for ListSkillsTool {
    fn name(&self) -> &str {
        "list_skills"
    }

    fn description(&self) -> &str {
        "List available skill playbooks (name + short description). \
         Use when you need to see which multi-step workflows you can load."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe).kind(ToolKind::Skill)
    }

    async fn execute(&self, _ctx: &crate::tool_context::ToolCallContext, _args: Value) -> Result<String, ToolError> {
        let guard = self
            .skills
            .lock()
            .map_err(|_| ToolError::failed("skills lock poisoned"))?;
        if guard.skills.is_empty() {
            return Ok("No skills installed. User can add SKILL.md under ~/.boris/skills/<name>/."
                .into());
        }
        let mut out = format!("{} skill(s):\n", guard.skills.len());
        for s in &guard.skills {
            out.push_str(&format!("- {} — {}\n", s.name, s.description));
        }
        out.push_str("Load one with load_skill(name=...).");
        Ok(truncate_tool_result(out))
    }
}

/// Load full skill body for the current turn.
pub struct LoadSkillTool {
    skills: SharedSkills,
}

impl LoadSkillTool {
    pub fn new(skills: SharedSkills) -> Self {
        Self { skills }
    }
}

#[async_trait]
impl Tool for LoadSkillTool {
    fn name(&self) -> &str {
        "load_skill"
    }

    fn description(&self) -> &str {
        "Load the full instructions for a named skill playbook. Call this when the user's \
         request matches a skill description, then follow the steps with other tools. \
         Required arg: name (skill id, e.g. research, get-things-done)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Skill name (directory name), e.g. research"
                },
                "args": {
                    "type": "string",
                    "description": "Optional extra context or user args for this run"
                }
            },
            "required": ["name"]
        })
    }

    fn meta(&self) -> ToolMeta {
        // Skill playbooks must not be chopped by the default 4k observation cap.
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::Skill)
            .max_result_chars(MAX_SKILL_RESULT_CHARS)
    }

    async fn execute(&self, _ctx: &crate::tool_context::ToolCallContext, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let name = require_string(obj, "name")?;
        let name = name.trim();
        if name.is_empty() {
            return Err(ToolError::invalid_args("name is empty"));
        }
        let extra = optional_string(obj, "args");

        let skill: Skill = {
            let guard = self
                .skills
                .lock()
                .map_err(|_| ToolError::failed("skills lock poisoned"))?;
            guard
                .get(name)
                .cloned()
                .ok_or_else(|| {
                    let available = guard.names().join(", ");
                    ToolError::failed(format!(
                        "unknown skill '{name}'. Available: {available}"
                    ))
                })?
        };

        let mut body = skills::load_skill_body(&skill).map_err(ToolError::failed)?;
        if let Some(a) = extra {
            if !a.trim().is_empty() {
                body.push_str("\n\nUser args / extra context:\n");
                body.push_str(a.trim());
            }
        }
        Ok(truncate_tool_result_to(body, MAX_SKILL_RESULT_CHARS))
    }
}

/// Skill tools for registration.
pub fn skill_tools(skills: SharedSkills) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(ListSkillsTool::new(skills.clone())),
        Box::new(LoadSkillTool::new(skills)),
    ]
}
