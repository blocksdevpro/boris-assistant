//! Tools so the model can actively update durable personal context mid-turn.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::memory::profile::{FactCategory, UserFact, UserProfile};
use crate::memory::store::ProfileStore;
use crate::tool::{
    optional_string, require_object, require_string, truncate_tool_result, Permission, Tool,
    ToolError, ToolKind, ToolMeta, ToolRisk,
};

/// Shared mutable profile used by tools + engine (same process).
pub type SharedProfile = Arc<Mutex<UserProfile>>;

fn with_profile<R>(
    profile: &SharedProfile,
    store: &ProfileStore,
    f: impl FnOnce(&mut UserProfile) -> Result<R, ToolError>,
) -> Result<R, ToolError> {
    let mut guard = profile
        .lock()
        .map_err(|_| ToolError::failed("profile lock poisoned"))?;
    let out = f(&mut guard)?;
    store
        .save(&guard)
        .map_err(|e| ToolError::failed(format!("save profile: {e}")))?;
    Ok(out)
}

/// Persist a durable fact about the user.
pub struct SaveUserFactTool {
    profile: SharedProfile,
    store: ProfileStore,
}

impl SaveUserFactTool {
    pub fn new(profile: SharedProfile, store: ProfileStore) -> Self {
        Self { profile, store }
    }

    pub fn with_path(profile: SharedProfile, path: impl Into<PathBuf>) -> Self {
        Self::new(profile, ProfileStore::new(path))
    }
}

#[async_trait]
impl Tool for SaveUserFactTool {
    fn name(&self) -> &str {
        "save_user_fact"
    }

    fn description(&self) -> &str {
        "Save a durable fact about the human user for future conversations \
         (name details, preferences, projects, people). Use when they share \
         something lasting about themselves. Do not save one-off chit-chat."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "fact": {
                    "type": "string",
                    "description": "Short factual phrase about the user"
                },
                "category": {
                    "type": "string",
                    "description": "identity | preference | project | relationship | habit | other"
                }
            },
            "required": ["fact"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Moderate)
            .kind(ToolKind::Memory)
            .permissions(&[Permission::FsWrite])
    }

    async fn execute(&self, _ctx: &crate::tool_context::ToolCallContext, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let fact = require_string(obj, "fact")?;
        let category = optional_string(obj, "category")
            .map(|c| FactCategory::parse(&c))
            .unwrap_or(FactCategory::Other);
        if fact.trim().len() < 3 {
            return Err(ToolError::invalid_args("fact too short"));
        }
        with_profile(&self.profile, &self.store, |p| {
            p.add_or_refresh_fact(UserFact::new(fact, category, "tool"));
            Ok(())
        })?;
        Ok(truncate_tool_result("Saved user fact.".into()))
    }
}

/// Set name / address-as on the profile.
pub struct UpdateUserProfileTool {
    profile: SharedProfile,
    store: ProfileStore,
}

impl UpdateUserProfileTool {
    pub fn new(profile: SharedProfile, store: ProfileStore) -> Self {
        Self { profile, store }
    }

    pub fn with_path(profile: SharedProfile, path: impl Into<PathBuf>) -> Self {
        Self::new(profile, ProfileStore::new(path))
    }
}

#[async_trait]
impl Tool for UpdateUserProfileTool {
    fn name(&self) -> &str {
        "update_user_profile"
    }

    fn description(&self) -> &str {
        "Update the user's profile fields: preferred_name, address_as, or a preference line. \
         Call when they say their name, how to address them, or a lasting preference."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "preferred_name": { "type": "string" },
                "address_as": { "type": "string" },
                "preference": {
                    "type": "string",
                    "description": "One preference line to remember"
                },
                "ongoing": {
                    "type": "string",
                    "description": "Current project or topic to track"
                }
            },
            "required": []
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Moderate)
            .kind(ToolKind::Memory)
            .permissions(&[Permission::FsWrite])
    }

    async fn execute(&self, _ctx: &crate::tool_context::ToolCallContext, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let mut changed = false;
        with_profile(&self.profile, &self.store, |p| {
            if let Some(n) = optional_string(obj, "preferred_name") {
                p.set_preferred_name(n);
                changed = true;
            }
            if let Some(a) = optional_string(obj, "address_as") {
                let a = a.trim().to_string();
                if !a.is_empty() {
                    p.address_as = Some(a);
                    p.touch();
                    changed = true;
                }
            }
            if let Some(pref) = optional_string(obj, "preference") {
                p.add_preference(pref);
                changed = true;
            }
            if let Some(on) = optional_string(obj, "ongoing") {
                p.add_ongoing(on);
                changed = true;
            }
            if !changed {
                return Err(ToolError::invalid_args(
                    "provide preferred_name, address_as, preference, and/or ongoing",
                ));
            }
            Ok(())
        })?;
        Ok(truncate_tool_result("Updated user profile.".into()))
    }
}

/// Read back the current personal context (for the model, not speech).
pub struct GetUserContextTool {
    profile: SharedProfile,
}

impl GetUserContextTool {
    pub fn new(profile: SharedProfile) -> Self {
        Self { profile }
    }
}

#[async_trait]
impl Tool for GetUserContextTool {
    fn name(&self) -> &str {
        "get_user_context"
    }

    fn description(&self) -> &str {
        "Read the durable personal context currently known about the user \
         (name, preferences, facts). Use when you need to recall who they are."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::Memory)
            .permissions(&[Permission::FsRead])
    }

    async fn execute(&self, _ctx: &crate::tool_context::ToolCallContext, _args: Value) -> Result<String, ToolError> {
        let guard = self
            .profile
            .lock()
            .map_err(|_| ToolError::failed("profile lock poisoned"))?;
        if guard.is_empty() {
            return Ok("No personal context stored yet.".into());
        }
        Ok(truncate_tool_result(guard.render_block(2000)))
    }
}
