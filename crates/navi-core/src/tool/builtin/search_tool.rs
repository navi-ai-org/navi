use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

const DEFAULT_MAX_RESULTS: usize = 50;
const MAX_RESULTS: usize = 500;
const FS_MAX_DEPTH: u64 = 10;

pub(crate) struct SearchTool {
    project_root: PathBuf,
    name: &'static str,
    mode: SearchToolMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchToolMode {
    Unified,
    Grep,
    FsBrowser,
    ListDir,
    Glob,
}

impl SearchTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self {
            project_root,
            name: "search",
            mode: SearchToolMode::Unified,
        }
    }

    pub(crate) fn grep(project_root: PathBuf) -> Self {
        Self {
            project_root,
            name: "grep",
            mode: SearchToolMode::Grep,
        }
    }

    pub(crate) fn fs_browser(project_root: PathBuf) -> Self {
        Self {
            project_root,
            name: "fs_browser",
            mode: SearchToolMode::FsBrowser,
        }
    }

    pub(crate) fn list_dir(project_root: PathBuf) -> Self {
        Self {
            project_root,
            name: "list_dir",
            mode: SearchToolMode::ListDir,
        }
    }

    pub(crate) fn glob(project_root: PathBuf) -> Self {
        Self {
            project_root,
            name: "glob",
            mode: SearchToolMode::Glob,
        }
    }
}

#[async_trait]
impl Tool for SearchTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            self.name,
            match self.mode {
                SearchToolMode::Unified => {
                    "Preferred project search/navigation tool. Actions: grep (literal text search), list (recursive file listing), tree (directory tree), find/glob (filename pattern), stat (file metadata). Prefer this over separate grep/list_dir/glob/fs_browser aliases. Paths in results are project-relative."
                }
                SearchToolMode::Grep => {
                    "Search project files for literal text. Special characters are matched literally."
                }
                SearchToolMode::FsBrowser => {
                    "Browse the project filesystem. Actions: list (recursive files), tree, find (filename pattern), stat. Paths in results are project-relative."
                }
                SearchToolMode::ListDir => {
                    "Recursively list project files under a path (skips build dirs). Returns project-relative paths. Use fs_browser action=tree for directory structure."
                }
                SearchToolMode::Glob => {
                    "Find project files whose path or filename matches a glob-like pattern."
                }
            },
            ToolKind::Read,
            match self.mode {
                SearchToolMode::Unified => unified_search_schema(),
                SearchToolMode::Grep => grep_schema(),
                SearchToolMode::FsBrowser => fs_browser_schema(),
                SearchToolMode::ListDir => list_dir_schema(),
                SearchToolMode::Glob => glob_schema(),
            },
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let action = match self.mode {
            SearchToolMode::Unified | SearchToolMode::FsBrowser => {
                helpers::required_string(&invocation.input, "action")?.to_string()
            }
            SearchToolMode::Grep => "grep".to_string(),
            SearchToolMode::ListDir => "list".to_string(),
            SearchToolMode::Glob => "find".to_string(),
        };
        let path = helpers::optional_string(&invocation.input, "path")
            .or_else(|| helpers::optional_string(&invocation.input, "directory"))
            .unwrap_or_else(|| ".".to_string());

        match action.as_str() {
            "grep" => {
                self.run_grep(&invocation.id, &invocation.input, &path)
                    .await
            }
            "list" => {
                self.run_list(&invocation.id, &path, &invocation.input)
                    .await
            }
            "tree" => {
                self.run_tree(&invocation.id, &path, &invocation.input)
                    .await
            }
            "find" => {
                self.run_find(&invocation.id, &path, &invocation.input)
                    .await
            }
            "stat" => self.run_stat(&invocation.id, &path, &invocation.input),
            _ => Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: helpers::tool_error(
                    "unknown_action",
                    format!("unknown search action: {action}"),
                    true,
                    Some("Use grep, list, tree, find, or stat."),
                    None,
                ),
            }),
        }
    }
}

fn unified_search_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["grep", "list", "tree", "find", "stat"],
                "description": "Operation to perform."
            },
            "pattern": {
                "type": "string",
                "description": "Literal text to search for (for grep) or filename/path pattern (for find).",
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
                "description": "Directory or file path. Defaults to current project root."
            },
            "directory": {
                "type": "string",
                "description": "Alias for path."
            },
            "include": {
                "type": "string",
                "description": "Optional filename filter, e.g. *.rs, *.{rs,toml}, or Cargo.toml (for grep)."
            },
            "max_results": {
                "type": "integer",
                "description": "Maximum number of matches/entries to return. Defaults to 50 and is capped at 500."
            },
            "limit": {
                "type": "integer",
                "description": "Alias for max_results."
            },
            "case_sensitive": {
                "type": "boolean",
                "description": "Whether matching is case-sensitive. Defaults to true (for grep)."
            },
            "depth": {
                "type": "integer",
                "description": "Maximum directory depth for tree action. Defaults to 3."
            },
            "hidden": {
                "type": "boolean",
                "description": "Include dotfiles and hidden directories. Defaults to false (for list/tree/find)."
            }
        },
        "required": ["action"],
        "additionalProperties": false,
    })
}

fn grep_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "pattern": {
                "type": "string",
                "description": "Literal text to search for.",
                "examples": ["needle"]
            },
            "query": {
                "type": "string",
                "description": "Alias for pattern."
            },
            "search": {
                "type": "string",
                "description": "Alias for pattern."
            },
            "path": {
                "type": "string",
                "description": "Directory or file path. Defaults to current project root."
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

fn fs_browser_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["list", "tree", "find", "stat"],
                "description": "Filesystem operation to perform."
            },
            "path": {
                "type": "string",
                "description": "Directory or file path. Defaults to current project root."
            },
            "directory": {
                "type": "string",
                "description": "Alias for path."
            },
            "pattern": {
                "type": "string",
                "description": "Filename/path pattern for find or list filtering."
            },
            "max_results": {
                "type": "integer",
                "description": "Maximum number of entries to return. Defaults to 50 and is capped at 500."
            },
            "limit": {
                "type": "integer",
                "description": "Alias for max_results."
            },
            "depth": {
                "type": "integer",
                "description": "Maximum directory depth for tree action. Defaults to 3."
            },
            "hidden": {
                "type": "boolean",
                "description": "Include dotfiles and hidden directories. Defaults to false."
            }
        },
        "required": ["action"],
        "additionalProperties": false,
    })
}

fn list_dir_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Directory path. Defaults to current project root."
            },
            "directory": {
                "type": "string",
                "description": "Alias for path."
            },
            "pattern": {
                "type": "string",
                "description": "Optional filename/path filter."
            },
            "max_results": {
                "type": "integer",
                "description": "Maximum number of entries to return. Defaults to 50 and is capped at 500."
            },
            "limit": {
                "type": "integer",
                "description": "Alias for max_results."
            },
            "hidden": {
                "type": "boolean",
                "description": "Include dotfiles and hidden directories. Defaults to false."
            }
        },
        "additionalProperties": false,
    })
}

fn glob_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "pattern": {
                "type": "string",
                "description": "Filename/path glob-like pattern to find.",
                "examples": ["*.rs"]
            },
            "path": {
                "type": "string",
                "description": "Directory to search. Defaults to current project root."
            },
            "directory": {
                "type": "string",
                "description": "Alias for path."
            },
            "max_results": {
                "type": "integer",
                "description": "Maximum number of files to return. Defaults to 50 and is capped at 500."
            },
            "limit": {
                "type": "integer",
                "description": "Alias for max_results."
            },
            "hidden": {
                "type": "boolean",
                "description": "Include dotfiles and hidden directories. Defaults to false."
            }
        },
        "required": ["pattern"],
        "additionalProperties": false,
    })
}

// ── Helper: resolve relative paths against project_root ────────────────────

fn resolve_project_path(project_root: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    }
}

fn display_path(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

// ═══════════════════════════════════════════════════════════════════════════
//  Grep
// ═══════════════════════════════════════════════════════════════════════════

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

impl SearchTool {
    async fn run_grep(
        &self,
        invocation_id: &str,
        input: &Value,
        _path: &str,
    ) -> Result<ToolResult> {
        let grep_input = GrepInput::from_json(input)?;
        let project_root = self.project_root.clone();
        let result = tokio::task::spawn_blocking(move || run_grep(&project_root, grep_input))
            .await
            .map_err(|e| anyhow::anyhow!("search grep task join error: {e}"))??;
        Ok(helpers::ok(invocation_id.to_string(), result))
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

// ═══════════════════════════════════════════════════════════════════════════
//  FsBrowser: list, tree, find, stat
// ═══════════════════════════════════════════════════════════════════════════

struct CollectConfig {
    pattern: Option<String>,
    hidden: bool,
    max_results: usize,
}

impl SearchTool {
    async fn run_list(&self, invocation_id: &str, path: &str, input: &Value) -> Result<ToolResult> {
        let pattern = helpers::optional_string(input, "pattern");
        let hidden = helpers::optional_bool(input, "hidden").unwrap_or(false);
        let max_results = helpers::optional_u64(input, "max_results")
            .or_else(|| helpers::optional_u64(input, "limit"))
            .unwrap_or(DEFAULT_MAX_RESULTS as u64)
            .min(MAX_RESULTS as u64) as usize;

        let project_root = self.project_root.clone();
        let path = path.to_string();
        let path_for_closure = path.clone();
        let config = CollectConfig {
            pattern,
            hidden,
            max_results,
        };
        let max_results_val = config.max_results;

        let files = tokio::task::spawn_blocking(move || {
            let root_path = resolve_project_path(&project_root, &path_for_closure);
            let mut files = Vec::new();
            collect_files_recursive(&project_root, &root_path, &config, 0, u64::MAX, &mut files);
            files
        })
        .await
        .map_err(|e| anyhow::anyhow!("search list task join error: {e}"))?;

        let truncated = files.len() >= max_results_val;
        Ok(helpers::ok(
            invocation_id.to_string(),
            json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "path": path,
                "files": files,
                "total": files.len(),
                "truncated": truncated,
            }),
        ))
    }

    async fn run_tree(&self, invocation_id: &str, path: &str, input: &Value) -> Result<ToolResult> {
        let depth = helpers::optional_u64(input, "depth")
            .unwrap_or(3)
            .min(FS_MAX_DEPTH);
        let hidden = helpers::optional_bool(input, "hidden").unwrap_or(false);

        let project_root = self.project_root.clone();
        let path = path.to_string();
        let path_for_closure = path.clone();

        let entries = tokio::task::spawn_blocking(move || {
            let root_path = resolve_project_path(&project_root, &path_for_closure);
            build_tree(&root_path, depth, hidden, 0)
        })
        .await
        .map_err(|e| anyhow::anyhow!("search tree task join error: {e}"))?;

        Ok(helpers::ok(
            invocation_id.to_string(),
            json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "path": path,
                "entries": entries,
                "total": count_entries(&entries),
            }),
        ))
    }

    async fn run_find(&self, invocation_id: &str, path: &str, input: &Value) -> Result<ToolResult> {
        let pattern = helpers::optional_string(input, "pattern");
        let hidden = helpers::optional_bool(input, "hidden").unwrap_or(false);
        let max_results = helpers::optional_u64(input, "max_results")
            .or_else(|| helpers::optional_u64(input, "limit"))
            .unwrap_or(DEFAULT_MAX_RESULTS as u64)
            .min(MAX_RESULTS as u64) as usize;

        let project_root = self.project_root.clone();
        let path = path.to_string();
        let path_for_closure = path.clone();
        let config = CollectConfig {
            pattern,
            hidden,
            max_results,
        };
        let max_results_val = config.max_results;

        let files = tokio::task::spawn_blocking(move || {
            let root_path = resolve_project_path(&project_root, &path_for_closure);
            let mut files = Vec::new();
            collect_files_recursive(&project_root, &root_path, &config, 0, u64::MAX, &mut files);
            files
        })
        .await
        .map_err(|e| anyhow::anyhow!("search find task join error: {e}"))?;

        let truncated = files.len() >= max_results_val;
        Ok(helpers::ok(
            invocation_id.to_string(),
            json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "path": path,
                "files": files,
                "total": files.len(),
                "truncated": truncated,
            }),
        ))
    }

    fn run_stat(&self, invocation_id: &str, path: &str, _input: &Value) -> Result<ToolResult> {
        // Stat is synchronous since it's a fast operation on a single path
        let p = resolve_project_path(&self.project_root, path);
        let meta = fs::metadata(&p).with_context(|| format!("failed to stat {path}"))?;

        let file_type = if meta.is_dir() {
            "dir"
        } else if meta.is_file() {
            "file"
        } else if meta.is_symlink() {
            "symlink"
        } else {
            "other"
        };

        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        let permissions = format!("{:o}", unix_permissions(&meta));

        Ok(helpers::ok(
            invocation_id.to_string(),
            json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "path": path,
                "type": file_type,
                "size": meta.len(),
                "modified": modified,
                "permissions": permissions,
            }),
        ))
    }
}

#[cfg(unix)]
fn unix_permissions(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode()
}

#[cfg(not(unix))]
fn unix_permissions(_meta: &fs::Metadata) -> u32 {
    0
}

fn collect_files_recursive(
    project_root: &Path,
    root: &Path,
    config: &CollectConfig,
    depth: u64,
    max_depth: u64,
    files: &mut Vec<String>,
) {
    if files.len() >= config.max_results || depth > max_depth {
        return;
    }

    if !root.exists() {
        return;
    }

    if root.is_file() {
        let display = display_path(project_root, root);
        if matches_pattern(&display, config.pattern.as_deref()) {
            files.push(display);
        }
        return;
    }

    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        if files.len() >= config.max_results {
            break;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if !config.hidden && name_str.starts_with('.') {
            continue;
        }

        if should_skip_dir(&name_str) {
            continue;
        }

        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(project_root, &path, config, depth + 1, max_depth, files);
        } else {
            let display = display_path(project_root, &path);
            if matches_pattern(&display, config.pattern.as_deref()) {
                files.push(display);
            }
        }
    }
}

fn build_tree(root: &Path, max_depth: u64, hidden: bool, depth: u64) -> Vec<Value> {
    let mut entries = Vec::new();

    if !root.is_dir() || depth > max_depth {
        return entries;
    }

    let dir_entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return entries,
    };

    for entry in dir_entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy().to_string();

        if !hidden && name_str.starts_with('.') {
            continue;
        }

        if should_skip_dir(&name_str) {
            continue;
        }

        let path = entry.path();
        let meta = fs::metadata(&path).ok();

        if path.is_dir() {
            let children = if depth < max_depth {
                build_tree(&path, max_depth, hidden, depth + 1)
            } else {
                Vec::new()
            };
            entries.push(json!({
                "name": name_str,
                "type": "dir",
                "children": children.len(),
                "entries": children,
            }));
        } else {
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            entries.push(json!({
                "name": name_str,
                "type": "file",
                "size": size,
            }));
        }
    }

    entries
}

fn count_entries(entries: &[Value]) -> usize {
    let mut count = 0;
    for entry in entries {
        count += 1;
        if let Some(children) = entry.get("entries").and_then(|v| v.as_array()) {
            count += count_entries(children);
        }
    }
    count
}

fn matches_pattern(name: &str, pattern: Option<&str>) -> bool {
    match pattern {
        Some(p) => name.contains(p),
        None => true,
    }
}

fn should_skip_dir(name: &str) -> bool {
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
}

// ═══════════════════════════════════════════════════════════════════════════
//  Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_tool(temp_dir: &Path) -> (SearchTool, String) {
        let tool = SearchTool::new(temp_dir.to_path_buf());
        (tool, temp_dir.display().to_string())
    }

    #[tokio::test]
    async fn test_search_grep_finds_text() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tempdir.path().join("src")).unwrap();
        std::fs::write(
            tempdir.path().join("src/lib.rs"),
            "pub fn hello() {}\npub fn world() {}\n",
        )
        .unwrap();

        let (tool, _) = mock_tool(tempdir.path());
        let inv = ToolInvocation {
            id: "g1".into(),
            tool_name: "search".into(),
            input: json!({
                "action": "grep",
                "pattern": "hello",
                "path": "src",
            }),
        };

        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok, "{:?}", result.output);
        assert_eq!(result.output["matches"].as_array().unwrap().len(), 1);
        assert_eq!(result.output["matches"][0]["line"], 1);
        assert_eq!(result.output["matches"][0]["path"], "src/lib.rs");
    }

    #[tokio::test]
    async fn test_search_grep_case_insensitive() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::write(tempdir.path().join("hello.txt"), "Hello World\n").unwrap();

        let (tool, _) = mock_tool(tempdir.path());
        let inv = ToolInvocation {
            id: "g2".into(),
            tool_name: "search".into(),
            input: json!({
                "action": "grep",
                "pattern": "hello",
                "case_sensitive": false,
            }),
        };

        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok, "{:?}", result.output);
        assert_eq!(result.output["matches"].as_array().unwrap().len(), 1);
        assert_eq!(result.output["matches"][0]["text"], "Hello World");
    }

    #[tokio::test]
    async fn test_search_list_directory() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::write(tempdir.path().join("a.txt"), "a").unwrap();
        std::fs::write(tempdir.path().join("b.rs"), "b").unwrap();
        std::fs::create_dir(tempdir.path().join("sub")).unwrap();
        std::fs::write(tempdir.path().join("sub/c.txt"), "c").unwrap();

        let (tool, _) = mock_tool(tempdir.path());
        let inv = ToolInvocation {
            id: "l1".into(),
            tool_name: "search".into(),
            input: json!({
                "action": "list",
                "path": ".",
            }),
        };

        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok, "{:?}", result.output);
        let files = result.output["files"].as_array().unwrap();
        assert_eq!(files.len(), 3);
        assert!(files.iter().any(|f| f.as_str().unwrap().ends_with("a.txt")));
        assert!(files.iter().any(|f| f.as_str().unwrap().ends_with("b.rs")));
        assert!(
            files
                .iter()
                .any(|f| f.as_str().unwrap().ends_with("sub/c.txt"))
        );
    }

    #[tokio::test]
    async fn test_search_list_with_pattern() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::write(tempdir.path().join("readme.md"), "md").unwrap();
        std::fs::write(tempdir.path().join("code.rs"), "rs").unwrap();
        std::fs::write(tempdir.path().join("notes.md"), "md").unwrap();

        let (tool, _) = mock_tool(tempdir.path());
        let inv = ToolInvocation {
            id: "l2".into(),
            tool_name: "search".into(),
            input: json!({
                "action": "list",
                "path": ".",
                "pattern": ".md",
            }),
        };

        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok, "{:?}", result.output);
        let files = result.output["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|f| f.as_str().unwrap().ends_with(".md")));
    }

    #[tokio::test]
    async fn test_search_tree() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::write(tempdir.path().join("file.txt"), "f").unwrap();
        std::fs::create_dir(tempdir.path().join("sub")).unwrap();
        std::fs::write(tempdir.path().join("sub/child.txt"), "c").unwrap();

        let (tool, _) = mock_tool(tempdir.path());
        let inv = ToolInvocation {
            id: "t1".into(),
            tool_name: "search".into(),
            input: json!({
                "action": "tree",
                "path": ".",
            }),
        };

        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok, "{:?}", result.output);
        let entries = result.output["entries"].as_array().unwrap();
        assert!(entries.iter().any(|e| e["name"] == "file.txt"));
        assert!(entries.iter().any(|e| e["name"] == "sub"));
    }

    #[tokio::test]
    async fn test_search_stat() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::write(tempdir.path().join("test.txt"), "hello").unwrap();

        let (tool, _) = mock_tool(tempdir.path());
        let inv = ToolInvocation {
            id: "s1".into(),
            tool_name: "search".into(),
            input: json!({
                "action": "stat",
                "path": "test.txt",
            }),
        };

        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok, "{:?}", result.output);
        assert_eq!(result.output["type"], "file");
        assert_eq!(result.output["size"], 5);
        assert!(result.output["modified"].is_number());
        assert!(result.output["permissions"].is_string());
    }
}
