//! Crush-style string-replace edit tools.
//!
//! - `edit`: single exact find-and-replace (create/delete helpers via empty strings)
//! - `multiedit`: sequential find-and-replace operations on one file
//!
//! These are the preferred surgical-edit tools. `apply_patch` remains available as a
//! power tool for multi-file/git-style patches.

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

use super::helpers;
use super::write_tool::{build_write_display_diff, count_diff_add_remove};
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

pub(crate) struct EditTool {
    project_root: PathBuf,
}

pub(crate) struct MultiEditTool {
    project_root: PathBuf,
}

impl EditTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }
}

impl MultiEditTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }
}

#[async_trait]
impl Tool for EditTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "edit",
            "Edit a file by exact find-and-replace. Prefer this over apply_patch for surgical edits.\n\
             Params: `path` (or `file_path`), `old_string`, `new_string`, optional `replace_all`.\n\
             - empty `old_string` creates a new file with `new_string`\n\
             - empty `new_string` deletes the matched `old_string`\n\
             - otherwise replaces exact text (must be unique unless replace_all=true)\n\
             For multiple edits to the same file pass `edits` (array of {old_string,new_string,replace_all?}). The `multiedit` tool name is a hidden alias. For large rewrites use `write_file`.",
            ToolKind::Write,
            edit_json_schema(),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let path = match path_arg(&invocation.input) {
            Some(p) => p,
            None => {
                return Ok(error_result(
                    &invocation,
                    "invalid_arguments",
                    "`path` (or `file_path`) is required",
                ));
            }
        };

        // Multi-edit mode: `edits` array (Crush multiedit absorbed into edit).
        if invocation.input.get("edits").and_then(Value::as_array).is_some() {
            return invoke_multiedit_on_path(&self.project_root, &invocation, path).await;
        }

        let old_string = string_arg(&invocation.input, &["old_string", "search"]).unwrap_or("");
        let new_string = string_arg(&invocation.input, &["new_string", "replace"]).unwrap_or("");
        let replace_all = bool_arg(&invocation.input, "replace_all").unwrap_or(false);

        // Detect missing both content fields (model forgot required args).
        if invocation.input.get("old_string").is_none()
            && invocation.input.get("search").is_none()
            && invocation.input.get("new_string").is_none()
            && invocation.input.get("replace").is_none()
        {
            return Ok(error_result(
                &invocation,
                "invalid_arguments",
                "Provide `old_string`/`new_string` (or `search`/`replace`), or `edits` for multiple replacements. Empty old_string creates a file; empty new_string deletes matched text.",
            ));
        }

        let full = match checked_project_path(&self.project_root, path) {
            Ok(p) => p,
            Err(e) => {
                return Ok(error_result(
                    &invocation,
                    "invalid_path",
                    &e.to_string(),
                ));
            }
        };

        let outcome = if old_string.is_empty() {
            create_new_file(&full, new_string)
        } else {
            let existing = match fs::read_to_string(&full) {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(error_result(
                        &invocation,
                        "file_not_found",
                        &format!("file not found: {path}. Use empty old_string to create, or write_file."),
                    ));
                }
                Err(e) => {
                    return Ok(error_result(
                        &invocation,
                        "io_error",
                        &format!("failed to read {path}: {e}"),
                    ));
                }
            };
            apply_one_edit(&existing, old_string, new_string, replace_all)
                .and_then(|new_content| write_if_changed(&full, &existing, &new_content).map(|_| {
                    EditOutcome {
                        created: false,
                        old_content: Some(existing),
                        new_content,
                    }
                }))
        };

        match outcome {
            Ok(result) => Ok(success_result(
                &invocation,
                path,
                result,
                1,
            )),
            Err(msg) => Ok(error_result(&invocation, "edit_failed", &msg)),
        }
    }
}

#[async_trait]
impl Tool for MultiEditTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "multiedit",
            "Apply multiple exact find-and-replace edits to a single file in one operation.\n\
             Hidden alias of `edit` with `edits` array. Prefer calling `edit` with `edits` for multiple replacements.\n\
             Params: `path` (or `file_path`) and `edits`: array of {old_string, new_string, replace_all?}.\n\
             Edits run sequentially; later edits see earlier results. Same exact-match rules as `edit`.\n\
             First edit may use empty old_string to create a new file.",
            ToolKind::Write,
            multiedit_json_schema(),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let path = match path_arg(&invocation.input) {
            Some(p) => p,
            None => {
                return Ok(error_result(
                    &invocation,
                    "invalid_arguments",
                    "`path` (or `file_path`) is required",
                ));
            }
        };
        invoke_multiedit_on_path(&self.project_root, &invocation, path).await
    }

}

async fn invoke_multiedit_on_path(
    project_root: &Path,
    invocation: &ToolInvocation,
    path: &str,
) -> Result<ToolResult> {
        let Some(edits) = invocation.input.get("edits").and_then(Value::as_array) else {
            return Ok(error_result(
                &invocation,
                "invalid_arguments",
                "`edits` must be a non-empty array of {old_string, new_string, replace_all?} objects",
            ));
        };
        if edits.is_empty() {
            return Ok(error_result(
                &invocation,
                "invalid_arguments",
                "`edits` array must contain at least one edit",
            ));
        }

        let full = match checked_project_path(project_root, path) {
            Ok(p) => p,
            Err(e) => {
                return Ok(error_result(
                    &invocation,
                    "invalid_path",
                    &e.to_string(),
                ));
            }
        };

        let mut current = match fs::read_to_string(&full) {
            Ok(c) => Some(c),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Ok(error_result(
                    &invocation,
                    "io_error",
                    &format!("failed to read {path}: {e}"),
                ));
            }
        };
        let original = current.clone();
        let mut applied = 0usize;

        for (idx, edit) in edits.iter().enumerate() {
            let old_string = string_arg(edit, &["old_string", "search"]).unwrap_or("");
            let new_string = string_arg(edit, &["new_string", "replace"]).unwrap_or("");
            let replace_all = bool_arg(edit, "replace_all").unwrap_or(false);

            if old_string.is_empty() {
                if idx != 0 {
                    return Ok(error_result(
                        &invocation,
                        "edit_failed",
                        &format!(
                            "edit {}: only the first edit can have empty old_string (for file creation)",
                            idx + 1
                        ),
                    ));
                }
                if current.is_some() {
                    return Ok(error_result(
                        &invocation,
                        "edit_failed",
                        &format!("edit {}: file already exists: {path}", idx + 1),
                    ));
                }
                current = Some(new_string.to_string());
                applied += 1;
                continue;
            }

            let Some(content) = current.as_deref() else {
                return Ok(error_result(
                    &invocation,
                    "edit_failed",
                    &format!(
                        "edit {}: file does not exist. Use empty old_string on the first edit to create it.",
                        idx + 1
                    ),
                ));
            };

            match apply_one_edit(content, old_string, new_string, replace_all) {
                Ok(next) => {
                    if next != content {
                        applied += 1;
                    }
                    current = Some(next);
                }
                Err(msg) => {
                    return Ok(error_result(
                        &invocation,
                        "edit_failed",
                        &format!("edit {}: {msg}", idx + 1),
                    ));
                }
            }
        }

        let Some(new_content) = current else {
            return Ok(error_result(
                &invocation,
                "edit_failed",
                "no content produced",
            ));
        };

        if original.as_deref() == Some(new_content.as_str()) {
            return Ok(error_result(
                &invocation,
                "edit_failed",
                "new content is the same as old content. No changes made.",
            ));
        }

        if let Some(parent) = full.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(e) = fs::write(&full, &new_content) {
            return Ok(error_result(
                &invocation,
                "io_error",
                &format!("failed to write {path}: {e}"),
            ));
        }

        Ok(success_result(
            &invocation,
            path,
            EditOutcome {
                created: original.is_none(),
                old_content: original,
                new_content,
            },
            applied,
        ))
}

struct EditOutcome {
    created: bool,
    old_content: Option<String>,
    new_content: String,
}

fn apply_one_edit(
    content: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> std::result::Result<String, String> {
    if old_string.is_empty() {
        return Err("old_string cannot be empty for content replacement".into());
    }

    let matches = count_non_overlapping(content, old_string);
    if matches == 0 {
        // Trailing-newline tolerant fallback (same spirit as write_tool search/replace).
        let old_norm = old_string.strip_suffix('\n').unwrap_or(old_string);
        if old_norm != old_string {
            let matches_norm = count_non_overlapping(content, old_norm);
            if matches_norm == 1 || (replace_all && matches_norm > 0) {
                let next = if replace_all {
                    content.replace(old_norm, new_string)
                } else {
                    content.replacen(old_norm, new_string, 1)
                };
                if next == content {
                    return Err(
                        "new content is the same as old content. No changes made.".into(),
                    );
                }
                return Ok(next);
            }
            if matches_norm > 1 && !replace_all {
                return Err(
                    "old_string appears multiple times in the file. Provide more context for a unique match, or set replace_all to true."
                        .into(),
                );
            }
        }
        return Err(
            "old_string not found in file. Make sure it matches exactly, including whitespace and line breaks. Use read_file to refresh content."
                .into(),
        );
    }
    if matches > 1 && !replace_all {
        return Err(
            "old_string appears multiple times in the file. Provide more context for a unique match, or set replace_all to true."
                .into(),
        );
    }

    let next = if replace_all {
        content.replace(old_string, new_string)
    } else {
        content.replacen(old_string, new_string, 1)
    };
    if next == content {
        return Err("new content is the same as old content. No changes made.".into());
    }
    Ok(next)
}

fn count_non_overlapping(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0usize;
    let mut start = 0usize;
    while let Some(pos) = haystack[start..].find(needle) {
        count += 1;
        start += pos + needle.len();
    }
    count
}

fn create_new_file(full: &Path, content: &str) -> std::result::Result<EditOutcome, String> {
    if full.exists() {
        if full.is_dir() {
            return Err(format!("path is a directory, not a file: {}", full.display()));
        }
        return Err(format!("file already exists: {}", full.display()));
    }
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create parent dirs: {e}"))?;
    }
    fs::write(full, content).map_err(|e| format!("failed to write file: {e}"))?;
    Ok(EditOutcome {
        created: true,
        old_content: None,
        new_content: content.to_string(),
    })
}

fn write_if_changed(
    full: &Path,
    old: &str,
    new: &str,
) -> std::result::Result<(), String> {
    if old == new {
        return Err("new content is the same as old content. No changes made.".into());
    }
    if let Some(parent) = full.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(full, new).map_err(|e| format!("failed to write file: {e}"))
}

fn success_result(
    invocation: &ToolInvocation,
    path: &str,
    outcome: EditOutcome,
    edits_applied: usize,
) -> ToolResult {
    let old = outcome.old_content.as_deref();
    let diff = build_write_display_diff(path, old, &outcome.new_content);
    let (lines_added, lines_removed) = count_diff_add_remove(&diff);
    let mut output = json!({
        "method": "search_replace",
        "status": 0,
        "path": path,
        "files_changed": [path],
        "edits_applied": edits_applied,
        "lines_added": lines_added,
        "lines_removed": lines_removed,
        "created": outcome.created,
    });
    if !diff.is_empty() {
        if let Value::Object(ref mut obj) = output {
            obj.insert("diff".to_string(), Value::String(diff));
        }
    }
    helpers::ok(invocation.id.clone(), output)
}

fn error_result(invocation: &ToolInvocation, code: &str, message: &str) -> ToolResult {
    ToolResult {
        invocation_id: invocation.id.clone(),
        ok: false,
        output: json!({
            "error_code": code,
            "error": message,
            "recoverable": true,
            "hint": "Ensure old_string matches the file exactly (whitespace/newlines). Use read_file if the file may have changed. Prefer multiedit for multiple changes in one file."
        }),
    }
}

fn path_arg(input: &Value) -> Option<&str> {
    string_arg(input, &["path", "file_path", "file"]).filter(|s| !s.is_empty())
}

fn string_arg<'a>(input: &'a Value, keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some(v) = input.get(*key).and_then(Value::as_str) {
            return Some(v);
        }
    }
    None
}

fn bool_arg(input: &Value, key: &str) -> Option<bool> {
    input.get(key).and_then(|v| {
        v.as_bool()
            .or_else(|| v.as_str().and_then(|s| match s {
                "true" | "1" | "yes" => Some(true),
                "false" | "0" | "no" => Some(false),
                _ => None,
            }))
    })
}

fn checked_project_path(project_root: &Path, path: &str) -> Result<PathBuf> {
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        bail!("path must stay inside the project: {path}");
    }
    Ok(project_root.join(relative))
}

fn edit_json_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Project-relative file path to edit (preferred)."
            },
            "file_path": {
                "type": "string",
                "description": "Alias for path (Crush/Claude-compatible)."
            },
            "old_string": {
                "type": "string",
                "description": "Exact text to find. Empty string creates a new file with new_string."
            },
            "new_string": {
                "type": "string",
                "description": "Replacement text. Empty string deletes the matched old_string."
            },
            "search": {
                "type": "string",
                "description": "Alias for old_string."
            },
            "replace": {
                "type": "string",
                "description": "Alias for new_string."
            },
            "replace_all": {
                "type": "boolean",
                "description": "Replace all occurrences of old_string (default false; unique match required)."
            },
            "edits": {
                "type": "array",
                "description": "Multiple sequential find-and-replace ops on this file (preferred over calling multiedit). Each item: old_string, new_string, replace_all?.",
                "minItems": 1,
                "items": {
                    "type": "object",
                    "properties": {
                        "old_string": { "type": "string" },
                        "new_string": { "type": "string" },
                        "search": { "type": "string", "description": "Alias for old_string." },
                        "replace": { "type": "string", "description": "Alias for new_string." },
                        "replace_all": { "type": "boolean" }
                    },
                    "additionalProperties": false
                }
            }
        },
        "additionalProperties": false,
        "examples": [
            {
                "path": "src/lib.rs",
                "old_string": "fn greet() {\n    println!(\"hi\");\n}",
                "new_string": "fn greet() {\n    println!(\"hello\");\n}"
            }
        ]
    })
}

fn multiedit_json_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Project-relative file path to edit (preferred)."
            },
            "file_path": {
                "type": "string",
                "description": "Alias for path (Crush/Claude-compatible)."
            },
            "edits": {
                "type": "array",
                "description": "Sequential find-and-replace operations on this file.",
                "minItems": 1,
                "items": {
                    "type": "object",
                    "properties": {
                        "old_string": {
                            "type": "string",
                            "description": "Exact text to find. Empty only allowed on first edit to create the file."
                        },
                        "new_string": {
                            "type": "string",
                            "description": "Replacement text. Empty deletes the matched old_string."
                        },
                        "search": {
                            "type": "string",
                            "description": "Alias for old_string."
                        },
                        "replace": {
                            "type": "string",
                            "description": "Alias for new_string."
                        },
                        "replace_all": {
                            "type": "boolean",
                            "description": "Replace all occurrences (default false)."
                        }
                    },
                    "additionalProperties": false
                }
            }
        },
        "required": ["edits"],
        "additionalProperties": false,
        "examples": [
            {
                "path": "src/lib.rs",
                "edits": [
                    { "old_string": "foo", "new_string": "bar" },
                    { "old_string": "baz", "new_string": "qux", "replace_all": true }
                ]
            }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolInvocation;
    use serde_json::json;
    use tempfile::tempdir;

    fn inv(tool: &str, input: Value) -> ToolInvocation {
        ToolInvocation {
            id: "t".into(),
            tool_name: tool.into(),
            input,
        }
    }

    #[tokio::test]
    async fn edit_replaces_unique_string() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("a.rs"), "fn main() {\n    hi\n}\n").unwrap();
        let tool = EditTool::new(root.clone());
        let result = tool
            .invoke(inv(
                "edit",
                json!({
                    "path": "a.rs",
                    "old_string": "    hi\n",
                    "new_string": "    hello\n"
                }),
            ))
            .await
            .unwrap();
        assert!(result.ok, "{:?}", result.output);
        assert_eq!(
            fs::read_to_string(root.join("a.rs")).unwrap(),
            "fn main() {\n    hello\n}\n"
        );
        assert_eq!(result.output["edits_applied"], 1);
        assert!(result.output.get("diff").is_some());
    }

    #[tokio::test]
    async fn edit_rejects_ambiguous_match_without_replace_all() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("a.rs"), "x = 1\nx = 2\n").unwrap();
        let tool = EditTool::new(root);
        let result = tool
            .invoke(inv(
                "edit",
                json!({
                    "path": "a.rs",
                    "old_string": "x = ",
                    "new_string": "y = "
                }),
            ))
            .await
            .unwrap();
        assert!(!result.ok);
        assert!(
            result.output["error"]
                .as_str()
                .unwrap_or("")
                .contains("multiple times"),
            "{:?}",
            result.output
        );
    }

    #[tokio::test]
    async fn edit_replace_all() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("a.rs"), "x = 1\nx = 2\n").unwrap();
        let tool = EditTool::new(root.clone());
        let result = tool
            .invoke(inv(
                "edit",
                json!({
                    "path": "a.rs",
                    "old_string": "x = ",
                    "new_string": "y = ",
                    "replace_all": true
                }),
            ))
            .await
            .unwrap();
        assert!(result.ok, "{:?}", result.output);
        assert_eq!(fs::read_to_string(root.join("a.rs")).unwrap(), "y = 1\ny = 2\n");
    }

    #[tokio::test]
    async fn edit_creates_file_with_empty_old_string() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let tool = EditTool::new(root.clone());
        let result = tool
            .invoke(inv(
                "edit",
                json!({
                    "path": "new.rs",
                    "old_string": "",
                    "new_string": "pub fn x() {}\n"
                }),
            ))
            .await
            .unwrap();
        assert!(result.ok, "{:?}", result.output);
        assert_eq!(
            fs::read_to_string(root.join("new.rs")).unwrap(),
            "pub fn x() {}\n"
        );
        assert_eq!(result.output["created"], true);
    }

    #[tokio::test]
    async fn edit_accepts_crush_file_path_and_search_aliases() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("a.txt"), "hello\n").unwrap();
        let tool = EditTool::new(root.clone());
        let result = tool
            .invoke(inv(
                "edit",
                json!({
                    "file_path": "a.txt",
                    "search": "hello",
                    "replace": "hola"
                }),
            ))
            .await
            .unwrap();
        assert!(result.ok, "{:?}", result.output);
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "hola\n");
    }

    #[tokio::test]
    async fn multiedit_applies_sequential_edits() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("a.rs"), "alpha beta\n").unwrap();
        let tool = MultiEditTool::new(root.clone());
        let result = tool
            .invoke(inv(
                "multiedit",
                json!({
                    "path": "a.rs",
                    "edits": [
                        { "old_string": "alpha", "new_string": "ALPHA" },
                        { "old_string": "beta", "new_string": "BETA" }
                    ]
                }),
            ))
            .await
            .unwrap();
        assert!(result.ok, "{:?}", result.output);
        assert_eq!(
            fs::read_to_string(root.join("a.rs")).unwrap(),
            "ALPHA BETA\n"
        );
        assert_eq!(result.output["edits_applied"], 2);
    }

    #[tokio::test]
    async fn multiedit_fails_clearly_on_missing_block() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("a.rs"), "only this\n").unwrap();
        let tool = MultiEditTool::new(root);
        let result = tool
            .invoke(inv(
                "multiedit",
                json!({
                    "path": "a.rs",
                    "edits": [
                        { "old_string": "missing", "new_string": "x" }
                    ]
                }),
            ))
            .await
            .unwrap();
        assert!(!result.ok);
        assert!(
            result.output["error"]
                .as_str()
                .unwrap_or("")
                .contains("not found"),
            "{:?}",
            result.output
        );
    }

    
    #[tokio::test]
    async fn edit_accepts_edits_array_for_multiedit() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("a.rs"), "alpha beta\n").unwrap();
        let tool = EditTool::new(root.clone());
        let result = tool
            .invoke(inv(
                "edit",
                json!({
                    "path": "a.rs",
                    "edits": [
                        { "old_string": "alpha", "new_string": "ALPHA" },
                        { "old_string": "beta", "new_string": "BETA" }
                    ]
                }),
            ))
            .await
            .unwrap();
        assert!(result.ok, "{:?}", result.output);
        assert_eq!(
            fs::read_to_string(root.join("a.rs")).unwrap(),
            "ALPHA BETA\n"
        );
        assert_eq!(result.output["edits_applied"], 2);
    }

#[tokio::test]
    async fn edit_rejects_path_escape() {
        let dir = tempdir().unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(inv(
                "edit",
                json!({
                    "path": "../outside.txt",
                    "old_string": "a",
                    "new_string": "b"
                }),
            ))
            .await
            .unwrap();
        assert!(!result.ok);
    }
}
