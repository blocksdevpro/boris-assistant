//! Tool trait, errors, metadata, and author helpers.
//!
//! | Module | Role |
//! |--------|------|
//! | [`error`] | [`ToolError`] / [`ToolErrorKind`] |
//! | [`meta`] | [`ToolMeta`], risk, permissions, kind |
//! | [`args`] | `require_object` / `require_string` / … |
//! | [`output`] | truncation + soft-wrap |
//! | [`trait_`] | [`Tool`] trait (`trait` is a keyword → `trait_`) |

mod args;
mod error;
mod meta;
mod output;
mod trait_;

pub use args::{
    optional_bool, optional_string, optional_u64, require_object, require_string, value_type_name,
};
pub use error::{ToolError, ToolErrorKind};
pub use meta::{Permission, ToolKind, ToolMeta, ToolRisk};
pub use output::{
    soft_wrap_line, soft_wrap_text, truncate_tool_result, truncate_tool_result_to,
    DEFAULT_SOFT_WRAP_WIDTH, MAX_SKILL_RESULT_CHARS, MAX_TOOL_RESULT_CHARS,
};
pub use trait_::Tool;
