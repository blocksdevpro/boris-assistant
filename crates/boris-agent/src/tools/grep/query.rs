//! Parse grep tool args, including Grok/Claude aliases (`-A`/`-B`/`-C`/`-i`).

use serde_json::Map;

use crate::tool::{
    optional_bool_keys, optional_string, optional_string_keys, optional_u64_keys, require_string,
    ToolError,
};

use super::{DEFAULT_LIMIT, MAX_CONTEXT, MAX_LIMIT};

/// How matches are projected back to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutputMode {
    /// `path:line:content` (and `path:line-` context lines). Default.
    Content,
    /// Unique file paths that contain a match (`rg -l`).
    FilesWithMatches,
    /// `path:count` per file (`rg -c`).
    Count,
}

/// Normalized grep request.
#[derive(Debug, Clone)]
pub(super) struct GrepQuery {
    pub pattern: String,
    pub path: Option<String>,
    pub glob: Option<String>,
    pub file_type: Option<String>,
    pub ignore_case: bool,
    pub multiline: bool,
    pub before: usize,
    pub after: usize,
    pub output_mode: OutputMode,
    pub limit: usize,
}

impl GrepQuery {
    pub(super) fn parse(obj: &Map<String, serde_json::Value>) -> Result<Self, ToolError> {
        let pattern = require_string(obj, "pattern")?;
        if pattern.trim().is_empty() {
            return Err(ToolError::invalid_args("pattern is empty"));
        }

        let glob = optional_string(obj, "glob").filter(|s| !s.is_empty());
        let file_type = optional_string_keys(obj, &["type", "file_type"]).filter(|s| !s.is_empty());
        let ignore_case = optional_bool_keys(obj, &["ignore_case", "-i"]).unwrap_or(false);
        let multiline = optional_bool_keys(obj, &["multiline"]).unwrap_or(false);

        let context = optional_u64_keys(obj, &["context", "-C"])
            .map(|n| n as usize)
            .unwrap_or(0)
            .min(MAX_CONTEXT);
        let mut before = optional_u64_keys(obj, &["before", "before_context", "-B"])
            .map(|n| n as usize)
            .unwrap_or(0)
            .min(MAX_CONTEXT);
        let mut after = optional_u64_keys(obj, &["after", "after_context", "-A"])
            .map(|n| n as usize)
            .unwrap_or(0)
            .min(MAX_CONTEXT);
        if context > 0 {
            if before == 0 {
                before = context;
            }
            if after == 0 {
                after = context;
            }
        }

        let output_mode =
            parse_output_mode(optional_string_keys(obj, &["output_mode", "mode"]).as_deref())?;
        let default_limit = match output_mode {
            OutputMode::Content => DEFAULT_LIMIT,
            OutputMode::FilesWithMatches | OutputMode::Count => 500,
        };
        let cap = match output_mode {
            OutputMode::Content => MAX_LIMIT,
            OutputMode::FilesWithMatches | OutputMode::Count => 2000,
        };
        let limit = optional_u64_keys(obj, &["head_limit", "limit"])
            .map(|n| n as usize)
            .unwrap_or(default_limit)
            .clamp(1, cap);

        Ok(Self {
            pattern,
            path: optional_string(obj, "path").filter(|s| !s.is_empty()),
            glob,
            file_type,
            ignore_case,
            multiline,
            before,
            after,
            output_mode,
            limit,
        })
    }
}

fn parse_output_mode(raw: Option<&str>) -> Result<OutputMode, ToolError> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(OutputMode::Content);
    };
    match raw.to_ascii_lowercase().as_str() {
        "content" => Ok(OutputMode::Content),
        "files_with_matches" | "files" | "files-with-matches" | "-l" => {
            Ok(OutputMode::FilesWithMatches)
        }
        "count" | "-c" => Ok(OutputMode::Count),
        other => Err(ToolError::invalid_args(format!(
            "unknown output_mode '{other}'. Use content, files_with_matches, or count."
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: serde_json::Value) -> Map<String, serde_json::Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn grok_aliases_and_context() {
        let q = GrepQuery::parse(&obj(json!({
            "pattern": "TODO",
            "-i": true,
            "-C": 2,
            "head_limit": 20,
            "output_mode": "content"
        })))
        .unwrap();
        assert!(q.ignore_case);
        assert_eq!(q.before, 2);
        assert_eq!(q.after, 2);
        assert_eq!(q.limit, 20);
        assert_eq!(q.output_mode, OutputMode::Content);
    }

    #[test]
    fn before_after_override_context() {
        let q = GrepQuery::parse(&obj(json!({
            "pattern": "fn",
            "context": 4,
            "-B": 1,
            "-A": 8
        })))
        .unwrap();
        assert_eq!(q.before, 1);
        assert_eq!(q.after, 8);
    }

    #[test]
    fn files_mode_alias() {
        let q = GrepQuery::parse(&obj(json!({
            "pattern": "main",
            "mode": "files"
        })))
        .unwrap();
        assert_eq!(q.output_mode, OutputMode::FilesWithMatches);
    }

    #[test]
    fn empty_pattern_rejected() {
        let err = GrepQuery::parse(&obj(json!({ "pattern": "  " }))).unwrap_err();
        assert!(err.message.contains("empty"));
    }

    #[test]
    fn unknown_mode_rejected() {
        let err = GrepQuery::parse(&obj(json!({
            "pattern": "x",
            "output_mode": "json"
        })))
        .unwrap_err();
        assert!(err.message.contains("output_mode"));
    }
}
