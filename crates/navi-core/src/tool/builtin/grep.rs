use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

const DEFAULT_MAX_RESULTS: usize = 50;
const MAX_RESULTS: usize = 500;

pub(crate) struct GrepTool {
    name: &'static str,
    project_root: PathBuf,
}

impl GrepTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self {
            name: "grep",
            project_root,
        }
    }

    pub(crate) fn search_alias(project_root: PathBuf) -> Self {
        Self {
            name: "search",
            project_root,
        }
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn definition(&self) -> ToolDefinition {
        let description = if self.name == "search" {
            "Alias for grep. Search project text files for a literal query/pattern. Use include to narrow by filename pattern, and limit/max_results to request more matches."
        } else {
            "Search project text files for a literal pattern. Use query/search as aliases for pattern, include to narrow by filename pattern, and limit/max_results to request more matches."
        };
        helpers::definition(self.name, description, ToolKind::Read, grep_json_schema())
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let input = GrepInput::from_json(&invocation.input)?;
        let project_root = self.project_root.clone();
        let result = tokio::task::spawn_blocking(move || run_grep(&project_root, input))
            .await
            .map_err(|e| anyhow::anyhow!("grep task join error: {e}"))??;
        Ok(helpers::ok(invocation.id, result))
    }
}

fn grep_json_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "pattern": {
                "type": "string",
                "description": "Literal text to search for. Regex is not required; special characters are matched literally.",
                "examples": ["needle"]
            },
            "query": {
                "type": "string",
                "description": "Alias for pattern, accepted for search-style calls."
            },
            "search": {
                "type": "string",
                "description": "Alias for pattern."
            },
            "path": {
                "type": "string",
                "description": "Project-relative directory or file to search. Defaults to project root."
            },
            "include": {
                "type": "string",
                "description": "Optional filename filter, e.g. *.rs, *.{rs,toml}, or Cargo.toml."
            },
            "max_results": {
                "type": "integer",
                "description": "Maximum number of matches to return. Defaults to 50 and is capped at 500."
            },
            "limit": {
                "type": "integer",
                "description": "Alias for max_results."
            },
            "case_sensitive": {
                "type": "boolean",
                "description": "Whether matching is case-sensitive. Defaults to true."
            },
            "regex": {
                "type": "boolean",
                "description": "Accepted for compatibility only. Matching remains literal."
            }
        },
        "anyOf": [
            { "required": ["pattern"] },
            { "required": ["query"] },
            { "required": ["search"] }
        ],
        "additionalProperties": false,
    })
}

struct GrepInput {
    pattern: String,
    path: String,
    include: Option<String>,
    max_results: usize,
    case_sensitive: bool,
}

impl GrepInput {
    fn from_json(input: &Value) -> Result<Self> {
        let pattern = helpers::optional_string(input, "pattern")
            .or_else(|| helpers::optional_string(input, "query"))
            .or_else(|| helpers::optional_string(input, "search"))
            .filter(|value| !value.is_empty())
            .context("missing required string `pattern` (or alias `query`/`search`)")?;
        let path = helpers::optional_string(input, "path").unwrap_or_else(|| ".".to_string());
        let include = helpers::optional_string(input, "include");
        let max_results = helpers::optional_u64(input, "max_results")
            .or_else(|| helpers::optional_u64(input, "limit"))
            .unwrap_or(DEFAULT_MAX_RESULTS as u64)
            .min(MAX_RESULTS as u64) as usize;
        let case_sensitive = helpers::optional_bool(input, "case_sensitive").unwrap_or(true);
        Ok(Self {
            pattern,
            path,
            include,
            max_results,
            case_sensitive,
        })
    }
}

fn run_grep(project_root: &Path, input: GrepInput) -> Result<Value> {
    let root = resolve_project_path(project_root, &input.path);
    let mut matches = Vec::new();
    collect_matches(project_root, &root, &input, &mut matches)?;
    let truncated = matches.len() >= input.max_results;
    Ok(json!({
        "matches": matches,
        "total": matches.len(),
        "truncated": truncated,
        "literal": true,
    }))
}

fn resolve_project_path(project_root: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    }
}

fn collect_matches(
    project_root: &Path,
    path: &Path,
    input: &GrepInput,
    matches: &mut Vec<Value>,
) -> Result<()> {
    if matches.len() >= input.max_results || should_skip(path) {
        return Ok(());
    }
    if path.is_file() {
        grep_file(project_root, path, input, matches)?;
        return Ok(());
    }
    if !path.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(path).with_context(|| format!("failed to list {}", path.display()))? {
        if matches.len() >= input.max_results {
            break;
        }
        collect_matches(project_root, &entry?.path(), input, matches)?;
    }
    Ok(())
}

fn grep_file(
    project_root: &Path,
    path: &Path,
    input: &GrepInput,
    matches: &mut Vec<Value>,
) -> Result<()> {
    if !matches_include(path, input.include.as_deref()) {
        return Ok(());
    }
    let Ok(content) = fs::read_to_string(path) else {
        return Ok(());
    };
    let pattern = if input.case_sensitive {
        input.pattern.clone()
    } else {
        input.pattern.to_lowercase()
    };
    let display_path = display_path(project_root, path);
    for (index, line) in content.lines().enumerate() {
        if matches.len() >= input.max_results {
            break;
        }
        let haystack = if input.case_sensitive {
            line.to_string()
        } else {
            line.to_lowercase()
        };
        if haystack.contains(&pattern) {
            matches.push(json!({
                "path": display_path,
                "line": index + 1,
                "text": line,
            }));
        }
    }
    Ok(())
}

fn display_path(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn matches_include(path: &Path, include: Option<&str>) -> bool {
    let Some(include) = include.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    let path = path.to_string_lossy();
    let file_name = Path::new(path.as_ref())
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path.as_ref());
    include
        .split(',')
        .map(str::trim)
        .any(|pattern| matches_one_include(&path, file_name, pattern))
}

fn matches_one_include(path: &str, file_name: &str, pattern: &str) -> bool {
    if pattern == "*" || pattern.is_empty() {
        return true;
    }
    if let Some(exts) = pattern
        .strip_prefix("*.{")
        .and_then(|p| p.strip_suffix('}'))
    {
        return exts
            .split(',')
            .map(str::trim)
            .any(|ext| file_name.ends_with(&format!(".{ext}")));
    }
    if let Some(ext) = pattern.strip_prefix("*.") {
        return file_name.ends_with(&format!(".{ext}"));
    }
    if pattern.contains('*') {
        let parts = pattern.split('*').filter(|part| !part.is_empty());
        let mut offset = 0usize;
        for part in parts {
            let Some(index) = path[offset..].find(part) else {
                return false;
            };
            offset += index + part.len();
        }
        return true;
    }
    path.ends_with(pattern) || file_name == pattern
}

fn should_skip(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                ".git"
                    | "target"
                    | "node_modules"
                    | ".cache"
                    | ".venv"
                    | "venv"
                    | "__pycache__"
                    | ".tox"
                    | "vendor"
                    | "dist"
                    | "build"
                    | "out"
                    | ".next"
                    | ".nuxt"
                    | ".output"
                    | ".parcel-cache"
                    | ".turbo"
                    | ".eslintcache"
                    | "coverage"
                    | ".nyc_output"
                    | "htmlcov"
                    | ".idea"
                    | ".vscode"
            )
        })
}
