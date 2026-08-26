//! Tools so the model can actively update durable personal context mid-turn.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::memory::profile::{FactCategory, UserFact, UserProfile};
use crate::memory::store::ProfileStore;
use crate::tool::{
    optional_bool, optional_string, optional_u64, require_object, require_string,
    truncate_tool_result, Permission, Tool, ToolError, ToolKind, ToolMeta, ToolRisk,
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
                },
                "memory_key": {
                    "type": "string",
                    "description": "Stable semantic slot for corrections, e.g. home_city or preferred_editor"
                },
                "expires_in_days": {
                    "type": "integer",
                    "description": "Optional lifetime for temporary facts"
                }
            },
            "required": ["fact"]
        })
    }

    fn meta(&self) -> ToolMeta {
        // Durable write — align with remember_note / update_user_profile.
        ToolMeta::with_risk(ToolRisk::Moderate)
            .kind(ToolKind::Memory)
            .permissions(&[Permission::FsWrite])
            .read_only(false)
            .max_concurrency(1)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let fact = require_string(obj, "fact")?;
        let category = optional_string(obj, "category")
            .map(|c| FactCategory::parse(&c))
            .unwrap_or(FactCategory::Other);
        if fact.trim().len() < 3 {
            return Err(ToolError::invalid_args("fact too short"));
        }
        let memory_key = optional_string(obj, "memory_key");
        let expires_in_days = optional_u64(obj, "expires_in_days");
        with_profile(&self.profile, &self.store, |p| {
            let mut record = UserFact::new(fact, category, "tool");
            if let Some(key) = memory_key {
                record = record.with_memory_key(key);
            }
            if let Some(days) = expires_in_days {
                const DAY_MS: u64 = 24 * 60 * 60 * 1_000;
                record.expires_at_ms = Some(
                    crate::memory::now_ms().saturating_add(days.min(3_650).saturating_mul(DAY_MS)),
                );
            }
            p.add_or_refresh_fact(record);
            Ok(())
        })?;
        Ok(truncate_tool_result("Saved user fact.".into()))
    }
}

/// Honor explicit correction/privacy requests without erasing the audit trail.
pub struct ForgetUserMemoryTool {
    profile: SharedProfile,
    store: ProfileStore,
}

impl ForgetUserMemoryTool {
    pub fn with_path(profile: SharedProfile, path: impl Into<PathBuf>) -> Self {
        Self {
            profile,
            store: ProfileStore::new(path),
        }
    }
}

#[async_trait]
impl Tool for ForgetUserMemoryTool {
    fn name(&self) -> &str {
        "forget_user_memory"
    }

    fn description(&self) -> &str {
        "Forget personal memory only when the human explicitly asks. Matching facts are tombstoned for audit and excluded from future context. Use all=true only for an explicit request to forget everything."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Fact or topic to forget" },
                "all": { "type": "boolean", "description": "Forget all personal context" }
            },
            "required": []
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Moderate)
            .kind(ToolKind::Memory)
            .permissions(&[Permission::FsWrite])
            .read_only(false)
            .max_concurrency(1)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let all = optional_bool(obj, "all").unwrap_or(false);
        let query = optional_string(obj, "query").unwrap_or_default();
        if !all && query.trim().is_empty() {
            return Err(ToolError::invalid_args("provide query or all=true"));
        }
        let changed = with_profile(&self.profile, &self.store, |profile| {
            if all {
                profile.forget_all();
                Ok(profile.facts.len())
            } else {
                Ok(profile.forget_matching(&query))
            }
        })?;
        Ok(truncate_tool_result(format!(
            "Forgot personal memory ({changed} matching record(s))."
        )))
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
            .read_only(false)
            .max_concurrency(1)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
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
        // Read-only recall — align with recall_notes / memory_get.
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::Memory)
            .permissions(&[Permission::FsRead])
            .read_only(true)
            .max_concurrency(8)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        _args: Value,
    ) -> Result<String, ToolError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::profile::UserProfile;
    use std::sync::{Arc, Mutex};

    fn dummy_profile() -> SharedProfile {
        Arc::new(Mutex::new(UserProfile::default()))
    }

    #[test]
    fn memory_profile_tool_meta_aligned() {
        let profile = dummy_profile();
        let path = std::env::temp_dir().join("boris-profile-meta-test.json");
        let save = SaveUserFactTool::with_path(profile.clone(), &path);
        let update = UpdateUserProfileTool::with_path(profile.clone(), &path);
        let forget = ForgetUserMemoryTool::with_path(profile.clone(), &path);
        let get = GetUserContextTool::new(profile);

        let save_m = save.meta();
        assert_eq!(save_m.read_only, Some(false));
        assert_eq!(save_m.max_concurrency, Some(1));
        assert!(save_m.permissions.contains(&Permission::FsWrite));

        let update_m = update.meta();
        assert_eq!(update_m.read_only, Some(false));
        assert_eq!(update_m.max_concurrency, Some(1));

        let forget_m = forget.meta();
        assert_eq!(forget_m.read_only, Some(false));
        assert!(forget_m.permissions.contains(&Permission::FsWrite));

        let get_m = get.meta();
        assert_eq!(get_m.read_only, Some(true));
        assert!(get_m.permissions.contains(&Permission::FsRead));
        assert!(get_m.is_read_only());
    }
}
