//! Unified `write` tool — replaces both `write_file` and `apply_patch`.
//!
//! Two modes (auto-detected from input):
//!
//! 1. **Direct write**: `{ path, content }` — write content to a single file.
//! 2. **Patch mode**: `{ patch }` or `{ patches: [...] }` — apply structured patches
//!    (`*** Begin Patch … *** End Patch`) or unified diffs.
//!
//! Features inherited from NAVI:
//! - Structured patch parser with full backup/rollback on failure
//! - Unified diff via `git apply --whitespace=fix` with `patch -p1` fallback
//! - File locking via `FileLockManager` (cross-instance safety)
//! - Context windows (20 lines ±) on patch failure
//! - `git_apply_error_hint` categorised error messages
//!
//! Features adopted from Codex:
//! - **Heredoc stripping**: patches wrapped in `<<'EOF'…EOF` are auto-detected
//! - **Verification phase**: prior to applying, reads target files to check context match
//! - **Delta tracking**: `AppliedPatchDelta` records every committed mutation
//! - **Fuzzy unicode matching**: EN DASH (U+2013), EM DASH (U+2014), NB-HYPHEN (U+2011)
//!   are normalised to ASCII before context matching
//! - **Environment ID preamble**: `*** Environment ID: <id>` line is parsed and echoed

use crate::file_lock::{FileLockManager, LockGuard};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use navi_vfs::code::{replace_symbol_definition, symbols_for_source};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

const PATCH_CONTEXT_RADIUS: usize = 20;
const MAX_PATCH_CONTEXT_WINDOWS: usize = 6;

pub(crate) struct WriteTool {
    project_root: PathBuf,
    lock_manager: Option<std::sync::Arc<FileLockManager>>,
    name: &'static str,
    mode: WriteToolMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteToolMode {
    Unified,
    Direct,
    Patch,
}

impl WriteTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self {
            project_root,
            lock_manager: None,
            name: "write",
            mode: WriteToolMode::Unified,
        }
    }

    pub(crate) fn with_lock_manager(
        project_root: PathBuf,
        lock_manager: std::sync::Arc<FileLockManager>,
    ) -> Self {
        Self {
            project_root,
            lock_manager: Some(lock_manager),
            name: "write",
            mode: WriteToolMode::Unified,
        }
    }

    fn alias(project_root: PathBuf, name: &'static str, mode: WriteToolMode) -> Self {
        Self {
            project_root,
            lock_manager: None,
            name,
            mode,
        }
    }

    fn alias_with_lock_manager(
        project_root: PathBuf,
        name: &'static str,
        mode: WriteToolMode,
        lock_manager: std::sync::Arc<FileLockManager>,
    ) -> Self {
        Self {
            project_root,
            lock_manager: Some(lock_manager),
            name,
            mode,
        }
    }

    pub(crate) fn write_file(project_root: PathBuf) -> Self {
        Self::alias(project_root, "write_file", WriteToolMode::Direct)
    }

    pub(crate) fn write_file_with_lock_manager(
        project_root: PathBuf,
        lock_manager: std::sync::Arc<FileLockManager>,
    ) -> Self {
        Self::alias_with_lock_manager(
            project_root,
            "write_file",
            WriteToolMode::Direct,
            lock_manager,
        )
    }

    pub(crate) fn apply_patch(project_root: PathBuf) -> Self {
        Self::alias(project_root, "apply_patch", WriteToolMode::Patch)
    }

    pub(crate) fn apply_patch_with_lock_manager(
        project_root: PathBuf,
        lock_manager: std::sync::Arc<FileLockManager>,
    ) -> Self {
        Self::alias_with_lock_manager(
            project_root,
            "apply_patch",
            WriteToolMode::Patch,
            lock_manager,
        )
    }
}

#[async_trait]
impl Tool for WriteTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            self.name,
            match self.mode {
                WriteToolMode::Unified => {
                    "Write content to files or apply patches. Two modes:\n\n\
             **Direct write** (use for creating new files or full replacements):\n\
             Pass `path` (project-relative) and `content` (full UTF-8 text).\n\n\
             **Patch mode** (use for surgical edits to existing files):\n\
             Pass `patch` (one patch string) or `patches` (array of patch strings).\n\
             The preferred format is the structured patch format:\n\
             ```\n\
             *** Begin Patch\n\
             *** Update File: path\n\
             @@\n\
              context line\n\
             -old line to remove\n\
             +new line to add\n\
             *** End Patch\n\
             ```\n\
             Also supports: `*** Add File: path`, `*** Delete File: path`, `*** Move to: target`.\n\
             Unified diff (`--- a/`, `+++ b/`, `@@` hunks) is also accepted."
                }
                WriteToolMode::Direct => {
                    "Write full UTF-8 content to a single project file, creating parent directories when needed."
                }
                WriteToolMode::Patch => {
                    "Apply one or more structured patches or unified diffs to project files."
                }
            },
            ToolKind::Write,
            match self.mode {
                WriteToolMode::Unified => write_json_schema(),
                WriteToolMode::Direct => direct_write_json_schema(),
                WriteToolMode::Patch => patch_write_json_schema(),
            },
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let input = &invocation.input;

        // --- Mode detection ---
        let has_direct = input
            .get("path")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
            && input
                .get("content")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty());

        let has_patch = input
            .get("patch")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty());
        let has_patches = input
            .get("patches")
            .and_then(Value::as_array)
            .map(|a| a.iter().any(|v| v.as_str().is_some_and(|s| !s.is_empty())))
            .unwrap_or(false);
        let has_edits = input
            .get("edits")
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false);

        match self.mode {
            WriteToolMode::Unified => {
                if has_direct {
                    return self.invoke_direct_write(invocation).await;
                }
                if has_edits {
                    return self.invoke_edits(&invocation).await;
                }
                if has_patch || has_patches {
                    return self.invoke_patch(invocation).await;
                }
            }
            WriteToolMode::Direct => {
                if has_direct {
                    return self.invoke_direct_write(invocation).await;
                }
            }
            WriteToolMode::Patch => {
                if has_edits {
                    return self.invoke_edits(&invocation).await;
                }
                if has_patch || has_patches {
                    return self.invoke_patch(invocation).await;
                }
            }
        }

        return Ok(ToolResult {
            invocation_id: invocation.id,
            ok: false,
            output: json!({
            "error_code": "invalid_arguments",
            "error": match self.mode {
                WriteToolMode::Unified => "Must provide either `path`+`content` (direct write), `patch`/`patches` (patch mode), or `edits` (search/replace).",
                WriteToolMode::Direct => "Must provide `path` and `content`.",
                WriteToolMode::Patch => "Must provide `patch`, `patches`, or `edits`.",
            }
            }),
        });
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Mode 1: Direct write
// ═══════════════════════════════════════════════════════════════════════════

impl WriteTool {
    async fn invoke_direct_write(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let raw_path = helpers::required_string(&invocation.input, "path")?.to_string();
        let path = Path::new(&raw_path);
        let full_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.project_root.join(path)
        };
        let full_path_str = full_path.to_string_lossy().to_string();
        let content = helpers::required_string(&invocation.input, "content")?.to_string();

        // Acquire file lock if configured.
        let _guard = if let Some(ref lm) = self.lock_manager {
            let lock_path = Path::new(&full_path_str);
            match lm.try_lock(lock_path) {
                Ok(Some(guard)) => Some(guard),
                Ok(None) => {
                    return Ok(ToolResult {
                        invocation_id: invocation.id,
                        ok: false,
                        output: json!({
                            "error": format!(
                                "File `{}` is locked by another NAVI instance. \
                                 Use the `wait` tool with `file_path=\"{}\"` to wait.",
                                path.display(), path.display()
                            ),
                            "error_code": "file_locked",
                            "file_path": raw_path,
                        }),
                    });
                }
                Err(e) => {
                    tracing::warn!(path = %full_path_str, error = %e, "failed to acquire file lock");
                    None
                }
            }
        } else {
            None
        };

        let _path_clone = raw_path.clone();
        let full_path_clone = full_path_str.clone();
        let content_clone = content.clone();

        let (line_counts, _existing_content) = tokio::task::spawn_blocking(move || {
            let existing = fs::read_to_string(&full_path_clone).ok();
            let lines_removed = existing.as_ref().map(|c| count_lines(c)).unwrap_or(0);

            if let Some(parent) = Path::new(&full_path_clone).parent()
                && !parent.as_os_str().is_empty()
            {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            fs::write(&full_path_clone, content_clone)
                .with_context(|| format!("failed to write {full_path_clone}"))?;

            Ok::<_, anyhow::Error>((lines_removed, existing))
        })
        .await
        .map_err(|e| anyhow::anyhow!("task join error: {}", e))??;

        let lines_added = count_lines(&content);
        let output = json!({
            "path": path,
            "bytes": content.len(),
            "lines_added": lines_added,
            "lines_removed": line_counts,
            "total_lines": lines_added,
        });

        Ok(helpers::ok(invocation.id, output))
    }
}

fn count_lines(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.lines().count().max(1)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Mode 2: Patch mode
// ═══════════════════════════════════════════════════════════════════════════

impl WriteTool {
    async fn invoke_edits(&self, invocation: &ToolInvocation) -> Result<ToolResult> {
        let Some(edits) = invocation.input.get("edits").and_then(Value::as_array) else {
            return Ok(ToolResult {
                invocation_id: invocation.id.clone(),
                ok: false,
                output: json!({
                    "error_code": "invalid_arguments",
                    "error": "`edits` must be a non-empty array of {path, search, replace} objects."
                }),
            });
        };

        if edits.is_empty() {
            return Ok(ToolResult {
                invocation_id: invocation.id.clone(),
                ok: false,
                output: json!({
                    "error_code": "invalid_arguments",
                    "error": "`edits` array must contain at least one edit."
                }),
            });
        }

        let mut parsed_edits = Vec::with_capacity(edits.len());
        for (idx, edit) in edits.iter().enumerate() {
            let path = edit
                .get("path")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("edit {idx}: missing `path`"))?;
            let search = edit
                .get("search")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("edit {idx}: missing `search`"))?;
            let replace = edit
                .get("replace")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("edit {idx}: missing `replace`"))?;
            parsed_edits.push((path.to_string(), search.to_string(), replace.to_string()));
        }

        // Acquire file locks if configured.
        let mut _lock_guards: Vec<LockGuard> = Vec::new();
        if let Some(lock_manager) = &self.lock_manager {
            for (path, _, _) in &parsed_edits {
                match lock_manager.try_lock(Path::new(path)) {
                    Ok(Some(guard)) => _lock_guards.push(guard),
                    Ok(None) => {
                        return Ok(ToolResult {
                            invocation_id: invocation.id.clone(),
                            ok: false,
                            output: json!({
                                "error": format!(
                                    "File `{path}` is locked by another NAVI instance. \
                                     Use the `wait` tool with `file_path=\"{path}\"` to wait."
                                ),
                                "error_code": "file_locked",
                                "file_path": path,
                            }),
                        });
                    }
                    Err(err) => {
                        tracing::warn!(path = %path, error = %err, "failed to acquire edit file lock");
                    }
                }
            }
        }

        let mut files_changed = Vec::new();
        let mut errors = Vec::new();

        for (path, search, replace) in &parsed_edits {
            let full = checked_project_path(&self.project_root, path)?;
            let content = match fs::read_to_string(&full) {
                Ok(c) => c,
                Err(e) => {
                    errors.push(format!("{path}: failed to read file: {e}"));
                    continue;
                }
            };
            let new_content = match apply_search_replace(&content, search, replace) {
                Some(c) => c,
                None => {
                    errors.push(format!(
                        "{path}: search block not found. Consider using `read_file` to refresh the exact content."
                    ));
                    continue;
                }
            };
            if new_content != content {
                if let Some(parent) = full.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                if let Err(e) = fs::write(&full, &new_content) {
                    errors.push(format!("{path}: failed to write file: {e}"));
                    continue;
                }
                files_changed.push(path.clone());
            }
        }

        if !errors.is_empty() && files_changed.is_empty() {
            return Ok(ToolResult {
                invocation_id: invocation.id.clone(),
                ok: false,
                output: json!({
                    "error_code": "edit_failed",
                    "error": errors.join("\n"),
                    "recoverable": true,
                    "hint": "Ensure each `search` block matches the file content exactly, including whitespace and newlines. Use `read_file` if the file may have changed."
                }),
            });
        }

        let mut output = json!({
            "method": "search_replace",
            "status": 0,
            "files_changed": files_changed,
            "edits_applied": files_changed.len(),
        });
        if !errors.is_empty() {
            if let Value::Object(ref mut obj) = output {
                obj.insert(
                    "warnings".to_string(),
                    Value::Array(errors.into_iter().map(Value::String).collect()),
                );
            }
        }
        Ok(helpers::ok(invocation.id.clone(), output))
    }
}

/// Apply a single search/replace to a file content. Returns `None` if the search block
/// is not found. Performs exact substring match first, then falls back to a normalized
/// match that ignores trailing newline differences.
fn apply_search_replace(content: &str, search: &str, replace: &str) -> Option<String> {
    if let Some(pos) = content.find(search) {
        let mut result = String::with_capacity(content.len() - search.len() + replace.len());
        result.push_str(&content[..pos]);
        result.push_str(replace);
        result.push_str(&content[pos + search.len()..]);
        return Some(result);
    }
    // Fallback: ignore trailing newline differences in the search block.
    let search_normalized = search.strip_suffix('\n').unwrap_or(search);
    let replace_normalized = if replace.ends_with('\n') || search.ends_with('\n') {
        replace.to_string()
    } else {
        replace.to_string()
    };
    let mut cursor = 0usize;
    while let Some(pos) = content[cursor..].find(search_normalized) {
        let absolute = cursor + pos;
        let after = absolute + search_normalized.len();
        if after == content.len() || content.as_bytes()[after] == b'\n' {
            let mut result = String::with_capacity(
                content.len() - search_normalized.len() + replace_normalized.len() + 1,
            );
            result.push_str(&content[..absolute]);
            result.push_str(&replace_normalized);
            if after < content.len() {
                result.push_str(&content[after..]);
            }
            return Some(result);
        }
        cursor = after;
    }
    None
}

// ═══════════════════════════════════════════════════════════════════════════
// Mode 2: Patch mode
// ═══════════════════════════════════════════════════════════════════════════

impl WriteTool {
    async fn invoke_patch(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let input = &invocation.input;
        let raw_patches = if let Some(single) = input.get("patch").and_then(Value::as_str) {
            vec![single.to_string()]
        } else if let Some(arr) = input.get("patches").and_then(Value::as_array) {
            arr.iter()
                .filter_map(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        } else {
            return Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: json!({
                    "error_code": "invalid_arguments",
                    "error": "Patch mode requires `patch` (string) or `patches` (non-empty array)."
                }),
            });
        };

        if raw_patches.is_empty() {
            return Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: json!({
                    "error_code": "invalid_arguments",
                    "error": "`patch` string or `patches` array must contain at least one non-empty patch."
                }),
            });
        }

        // Lenient heredoc stripping (Codex feature): if a patch body is wrapped in
        // <<[EOF|'EOF'|"EOF"]...EOF, strip the markers.
        let patches: Vec<String> = raw_patches
            .iter()
            .map(|p| {
                let patch = strip_heredoc(p).unwrap_or_else(|| p.to_string());
                normalize_structured_patch_hunk_prefixes(&patch)
            })
            .collect();

        let affected = patch_affected_files(&patches)?;

        // Acquire file locks if configured.
        let mut _lock_guards: Vec<LockGuard> = Vec::new();
        if let Some(lock_manager) = &self.lock_manager {
            for file in &affected {
                match lock_manager.try_lock(Path::new(file)) {
                    Ok(Some(guard)) => _lock_guards.push(guard),
                    Ok(None) => {
                        return Ok(ToolResult {
                            invocation_id: invocation.id,
                            ok: false,
                            output: json!({
                                "error": format!(
                                    "File `{file}` is locked by another NAVI instance. \
                                     Use the `wait` tool with `file_path=\"{file}\"` to wait."
                                ),
                                "error_code": "file_locked",
                                "file_path": file,
                            }),
                        });
                    }
                    Err(err) => {
                        tracing::warn!(path = %file, error = %err, "failed to acquire patch file lock");
                    }
                }
            }
        }

        // Phase 1: Parse and verify (Codex verification phase).
        // For structured patches, read target files and attempt context match BEFORE applying.
        let mut verification_errors: Vec<String> = Vec::new();
        let mut patch_is_structured = false;
        for patch in &patches {
            if is_structured_patch(patch) {
                patch_is_structured = true;
                if let Err(err) = verify_structured_patch(&self.project_root, patch) {
                    verification_errors.push(err.to_string());
                }
            }
        }
        if !verification_errors.is_empty() {
            match apply_structured_symbol_replacement_fallback(&self.project_root, &patches) {
                Ok(Some(files_patched)) => {
                    return Ok(ToolResult {
                        invocation_id: invocation.id,
                        ok: true,
                        output: json!({
                            "method": "structured_symbol_replacement_fallback",
                            "status": 0,
                            "patches_applied": patches.len(),
                            "files_patched": files_patched,
                            "recovered_from": "verification_failed",
                            "warnings": verification_errors,
                            "affected_paths": affected,
                        }),
                    });
                }
                Ok(None) => {}
                Err(err) => {
                    verification_errors
                        .push(format!("symbol replacement fallback failed: {err:#}"));
                }
            }
            let output = json!({
                "error_code": "verification_failed",
                "error": format!("Patch verification failed:\n{}", verification_errors.join("\n")),
                "recoverable": true,
                "hint": "Re-read the affected files and regenerate the patch with exact context.",
                "context_lines": patch_failure_contexts(&self.project_root, &patches),
                "context_note": "Each context window includes up to 20 lines before and 20 lines after the nearest relevant patch hunk location.",
            });
            return Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output,
            });
        }

        // Phase 2: Apply.
        if patch_is_structured || patches.iter().all(|p| is_structured_patch(p)) {
            return match apply_structured_patches(&self.project_root, &patches) {
                Ok(files_patched) => Ok(ToolResult {
                    invocation_id: invocation.id,
                    ok: true,
                    output: json!({
                        "method": "structured",
                        "status": 0,
                        "patches_applied": patches.len(),
                        "files_patched": files_patched,
                        "affected_paths": affected,
                    }),
                }),
                Err(err) => Ok(ToolResult {
                    invocation_id: invocation.id,
                    ok: false,
                    output: patch_failed_output(
                        "patch_failed",
                        format!("structured patch failed: {err:#}"),
                        "Rebuild the structured patch with exact context from `context_lines`: \
                         *** Begin Patch, file operation headers, @@ hunks, and *** End Patch.",
                        None,
                        patch_failure_contexts(&self.project_root, &patches),
                    ),
                }),
            };
        }

        // Unified diff path: git apply + patch fallback.
        let patch = patches.join("\n");
        for file in &affected {
            let full = self.project_root.join(file);
            if let Some(parent) = full.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
        }

        let git_result = run_git_apply(&self.project_root, &patch).await?;
        if git_result.status.success() {
            return Ok(ToolResult {
                invocation_id: invocation.id,
                ok: true,
                output: json!({
                    "method": "git_apply",
                    "status": git_result.status.code(),
                    "patches_applied": patches.len(),
                    "stdout": String::from_utf8_lossy(&git_result.stdout),
                    "stderr": String::from_utf8_lossy(&git_result.stderr),
                    "files_patched": affected.len(),
                    "affected_paths": affected,
                }),
            });
        }

        let git_stderr = String::from_utf8_lossy(&git_result.stderr).to_string();

        // Try again with relaxed whitespace rules if the first git apply failed.
        let relaxed_git_result = run_git_apply_relaxed(&self.project_root, &patch).await?;
        if relaxed_git_result.status.success() {
            return Ok(ToolResult {
                invocation_id: invocation.id,
                ok: true,
                output: json!({
                    "method": "git_apply_relaxed",
                    "status": relaxed_git_result.status.code(),
                    "patches_applied": patches.len(),
                    "stdout": String::from_utf8_lossy(&relaxed_git_result.stdout),
                    "stderr": String::from_utf8_lossy(&relaxed_git_result.stderr),
                    "files_patched": affected.len(),
                    "affected_paths": affected,
                }),
            });
        }

        let patch_result = run_patch_command(&self.project_root, &patch).await?;
        if patch_result.status.success() {
            return Ok(ToolResult {
                invocation_id: invocation.id,
                ok: true,
                output: json!({
                    "method": "patch_fallback",
                    "status": patch_result.status.code(),
                    "patches_applied": patches.len(),
                    "stdout": String::from_utf8_lossy(&patch_result.stdout),
                    "stderr": String::from_utf8_lossy(&patch_result.stderr),
                    "files_patched": affected.len(),
                    "affected_paths": affected,
                }),
            });
        }

        let patch_stderr = String::from_utf8_lossy(&patch_result.stderr).to_string();
        let hint = git_apply_error_hint(&git_stderr);

        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: false,
            output: patch_failed_output(
                "patch_failed",
                format!(
                    "git apply failed:\n{}\npatch -p1 failed:\n{}",
                    git_stderr.trim(),
                    patch_stderr.trim()
                ),
                hint,
                Some(format!(
                    "git apply stderr:\n{}\npatch stderr:\n{}",
                    git_stderr.trim(),
                    patch_stderr.trim()
                )),
                patch_failure_contexts(&self.project_root, &patches),
            ),
        })
    }
}

// ── Lenient heredoc stripping (Codex feature) ────────────────────────────

/// If `patch` is wrapped in `<<[EOF|'EOF'|"EOF"]...EOF`, strip the markers.
/// Returns `Some(inner)` with the patch body if heredoc detected, else `None`.
fn strip_heredoc(patch: &str) -> Option<String> {
    let trimmed = patch.trim_start();
    let skip_len = if trimmed.starts_with("<<'EOF'") {
        Some(7) // <<'EOF'
    } else if trimmed.starts_with("<<\"EOF\"") {
        Some(7) // <<"EOF"
    } else if trimmed.starts_with("<<EOF") {
        Some(5) // <<EOF
    } else {
        None
    }?;
    let after_marker = &trimmed[skip_len..];
    // Find the closing EOF on its own line.
    let eof_pos = after_marker.rfind("\nEOF")?;
    let body = after_marker[..eof_pos].trim_end().trim_start();
    if body.starts_with("*** Begin Patch") || body.starts_with("--- ") {
        Some(body.to_string())
    } else {
        // Not a real patch inside — some other heredoc.
        None
    }
}

fn normalize_structured_patch_hunk_prefixes(patch: &str) -> String {
    if !is_structured_patch(patch) {
        return patch.to_string();
    }

    let mut normalized = Vec::new();
    let mut in_hunk = false;
    for line in patch.lines() {
        if line.starts_with("*** ") {
            in_hunk = false;
            normalized.push(line.to_string());
            continue;
        }
        if line.starts_with("@@") {
            in_hunk = true;
            normalized.push(line.to_string());
            continue;
        }
        if in_hunk
            && !line.is_empty()
            && !line.starts_with(' ')
            && !line.starts_with('-')
            && !line.starts_with('+')
        {
            normalized.push(format!(" {line}"));
        } else {
            normalized.push(line.to_string());
        }
    }

    let mut patch = normalized.join("\n");
    if patch.ends_with("*** End Patch") || patch.ends_with("*** End of File") {
        patch.push('\n');
    }
    patch
}

// ── Fuzzy unicode normalisation (Codex feature) ─────────────────────────

/// Normalise punctuation characters that differ between what the model emits
/// and what the file contains (e.g. EN DASH → ASCII hyphen).
fn normalise_line(s: &str) -> String {
    s.replace('\u{2013}', "-") // EN DASH
        .replace('\u{2014}', "--") // EM DASH
        .replace('\u{2011}', "-") // NON-BREAKING HYPHEN
        .replace('\u{2018}', "'") // LEFT SINGLE QUOTATION MARK
        .replace('\u{2019}', "'") // RIGHT SINGLE QUOTATION MARK
        .replace('\u{201c}', "\"") // LEFT DOUBLE QUOTATION MARK
        .replace('\u{201d}', "\"") // RIGHT DOUBLE QUOTATION MARK
}

// ── Verification phase (Codex feature) ───────────────────────────────────

fn verify_structured_patch(project_root: &Path, patch: &str) -> Result<()> {
    let ops = parse_structured_patch(patch)?;
    for op in &ops {
        match op {
            StructuredOp::Update { path, hunks, .. } => {
                let full = checked_project_path(project_root, path)?;
                let content = match fs::read_to_string(&full) {
                    Ok(c) => c,
                    Err(e) => {
                        bail!("Cannot read {path} for verification: {e}");
                    }
                };
                let old_lines: Vec<String> = content.lines().map(str::to_string).collect();
                let mut cursor = 0usize;
                for (hunk_idx, hunk) in hunks.iter().enumerate() {
                    let pos = find_hunk_position(&old_lines, cursor, hunk)
                        .or_else(|| {
                            // Try fuzzy match with normalised lines.
                            let normalised: Vec<String> =
                                old_lines.iter().map(|l| normalise_line(l)).collect();
                            find_hunk_position(&normalised, cursor, hunk)
                        })
                        .with_context(|| {
                            format!(
                                "Hunk {} of {}: context lines do not match the file on disk. \
                                 Re-read the file and regenerate the patch with exact context.",
                                hunk_idx + 1,
                                path
                            )
                        })?;
                    cursor = pos;
                    // Advance cursor past context/remove lines.
                    for line in hunk {
                        match line {
                            HunkLine::Context(_) | HunkLine::Remove(_) => cursor += 1,
                            HunkLine::Add(_) => {}
                        }
                    }
                }
            }
            StructuredOp::Delete { path } => {
                let full = checked_project_path(project_root, path)?;
                if !full.exists() {
                    bail!("Cannot delete {path}: file does not exist");
                }
            }
            StructuredOp::Add { .. } => {
                // Adding files does not need verification against existing content.
            }
        }
    }
    Ok(())
}

fn apply_structured_symbol_replacement_fallback(
    project_root: &Path,
    patches: &[String],
) -> Result<Option<usize>> {
    let mut candidates = Vec::new();
    for patch in patches {
        if !is_structured_patch(patch) {
            continue;
        }
        candidates.extend(structured_symbol_replacement_candidates(
            project_root,
            patch,
        )?);
    }

    match candidates.len() {
        0 => Ok(None),
        1 => {
            let candidate = candidates.pop().expect("one candidate");
            fs::write(&candidate.path, candidate.content)
                .with_context(|| format!("failed to write {}", candidate.path.display()))?;
            Ok(Some(1))
        }
        _ => bail!(
            "ambiguous malformed patch recovery: {} symbol replacement candidates",
            candidates.len()
        ),
    }
}

struct SymbolReplacementCandidate {
    path: PathBuf,
    content: String,
}

fn structured_symbol_replacement_candidates(
    project_root: &Path,
    patch: &str,
) -> Result<Vec<SymbolReplacementCandidate>> {
    let mut candidates = Vec::new();
    let mut current_path: Option<String> = None;
    let mut hunk_lines = Vec::new();

    for line in patch.lines().chain(std::iter::once("*** End Patch")) {
        if let Some(path) = line.strip_prefix("*** Update File: ") {
            if let Some(path) = current_path.take() {
                candidates.extend(symbol_replacements_from_hunks(
                    project_root,
                    &path,
                    &hunk_lines,
                )?);
                hunk_lines.clear();
            }
            current_path = Some(path.to_string());
            continue;
        }
        if line.starts_with("*** ") {
            if let Some(path) = current_path.take() {
                candidates.extend(symbol_replacements_from_hunks(
                    project_root,
                    &path,
                    &hunk_lines,
                )?);
                hunk_lines.clear();
            }
            continue;
        }
        if current_path.is_some() {
            hunk_lines.push(line.to_string());
        }
    }

    Ok(candidates)
}

fn symbol_replacements_from_hunks(
    project_root: &Path,
    relative_path: &str,
    hunk_lines: &[String],
) -> Result<Vec<SymbolReplacementCandidate>> {
    let path = checked_project_path(project_root, relative_path)?;
    let source =
        fs::read_to_string(&path).with_context(|| format!("failed to read {relative_path}"))?;
    let mut candidates = Vec::new();

    for replacement in extract_complete_function_blocks(hunk_lines) {
        let Ok(symbols) = symbols_for_source(&path, &replacement) else {
            continue;
        };
        if symbols.len() != 1 {
            continue;
        }
        let symbol = &symbols[0];
        let Ok(edit) = replace_symbol_definition(&path, &source, &symbol.name, &replacement, None)
        else {
            continue;
        };
        if edit.content != source {
            candidates.push(SymbolReplacementCandidate {
                path: path.clone(),
                content: edit.content,
            });
        }
    }

    Ok(candidates)
}

fn extract_complete_function_blocks(hunk_lines: &[String]) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < hunk_lines.len() {
        let Some(content) = replacement_line_content(&hunk_lines[index]) else {
            index += 1;
            continue;
        };
        if !looks_like_function_start(content) {
            index += 1;
            continue;
        }

        let mut block = Vec::new();
        let mut brace_balance = 0isize;
        let mut saw_open_brace = false;
        let mut cursor = index;
        while cursor < hunk_lines.len() {
            let Some(content) = replacement_line_content(&hunk_lines[cursor]) else {
                break;
            };
            block.push(content.to_string());
            for ch in content.chars() {
                match ch {
                    '{' => {
                        saw_open_brace = true;
                        brace_balance += 1;
                    }
                    '}' => brace_balance -= 1,
                    _ => {}
                }
            }
            cursor += 1;
            if saw_open_brace && brace_balance == 0 {
                blocks.push(block.join("\n"));
                break;
            }
        }

        index = cursor.max(index + 1);
    }
    blocks
}

fn replacement_line_content(line: &str) -> Option<&str> {
    if line.starts_with("@@") || line.starts_with('-') {
        None
    } else if let Some(content) = line.strip_prefix('+') {
        Some(content)
    } else if let Some(content) = line.strip_prefix(' ') {
        Some(content)
    } else {
        Some(line)
    }
}

fn looks_like_function_start(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("fn ")
        || trimmed.starts_with("pub fn ")
        || trimmed.starts_with("pub(crate) fn ")
        || trimmed.starts_with("pub(super) fn ")
}

// ═══════════════════════════════════════════════════════════════════════════
// Shared utilities for patch mode
// ═══════════════════════════════════════════════════════════════════════════

fn patch_affected_files(patches: &[String]) -> Result<Vec<String>> {
    let mut files = Vec::new();
    for patch in patches {
        if is_structured_patch(patch) {
            for op in parse_structured_patch(patch)? {
                match op {
                    StructuredOp::Add { path, .. } | StructuredOp::Delete { path } => {
                        push_unique_string(&mut files, path);
                    }
                    StructuredOp::Update { path, move_to, .. } => {
                        push_unique_string(&mut files, path);
                        if let Some(target) = move_to {
                            push_unique_string(&mut files, target);
                        }
                    }
                }
            }
        } else {
            for path in extract_patched_files(patch) {
                push_unique_string(&mut files, path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn push_unique_string(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn write_json_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Project-relative file path (required for direct write mode, in combination with `content`)."
            },
            "content": {
                "type": "string",
                "description": "Full UTF-8 file content to write (required for direct write mode, in combination with `path`)."
            },
            "patch": {
                "type": "string",
                "description": "A single complete patch string (structured format: *** Begin Patch, hunks, *** End Patch; or unified diff; or JSON search/replace array)."
            },
            "patches": {
                "type": "array",
                "description": "Multiple patch strings to apply in one call. Each element is a complete patch string (structured, unified diff, or JSON search/replace array).",
                "items": { "type": "string" },
                "minItems": 1
            },
            "edits": {
                "type": "array",
                "description": "Simple search/replace edits. Each object must include `path`, `search` and `replace`. This is the easiest format for surgical edits when the exact file content is known. `search` must match a contiguous block in the file exactly; use `replace` with the desired content.",
                "items": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Project-relative file path to edit." },
                        "search": { "type": "string", "description": "Exact contiguous text to search for in the file." },
                        "replace": { "type": "string", "description": "Text to replace the matched block with." }
                    },
                    "required": ["path", "search", "replace"],
                    "additionalProperties": false
                },
                "minItems": 1
            }
        },
        "anyOf": [
            { "required": ["path", "content"] },
            { "required": ["patch"] },
            { "required": ["patches"] },
            { "required": ["edits"] }
        ],
        "maxProperties": 2,
        "additionalProperties": false,
        "examples": [
            { "path": "src/main.rs", "content": "fn main() { println!(\"hello\"); }" },
            { "patch": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch" },
            { "edits": [{ "path": "src/lib.rs", "search": "old\n", "replace": "new\n" }] }
        ]
    })
}

fn direct_write_json_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Project-relative file path to write."
            },
            "content": {
                "type": "string",
                "description": "Full UTF-8 file content to write."
            }
        },
        "required": ["path", "content"],
        "additionalProperties": false,
        "examples": [
            { "path": "src/main.rs", "content": "fn main() { println!(\"hello\"); }\n" }
        ]
    })
}

fn patch_write_json_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "patch": {
                "type": "string",
                "description": "A single complete patch string (structured format: *** Begin Patch, hunks, *** End Patch; or unified diff; or JSON search/replace array)."
            },
            "patches": {
                "type": "array",
                "description": "Multiple patch strings to apply in one call. Each element is a complete patch string (structured, unified diff, or JSON search/replace array).",
                "items": { "type": "string" },
                "minItems": 1
            },
            "edits": {
                "type": "array",
                "description": "Simple search/replace edits. Each object must include `path`, `search` and `replace`. This is the easiest format for surgical edits when the exact file content is known. `search` must match a contiguous block in the file exactly; use `replace` with the desired content.",
                "items": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Project-relative file path to edit." },
                        "search": { "type": "string", "description": "Exact contiguous text to search for in the file." },
                        "replace": { "type": "string", "description": "Text to replace the matched block with." }
                    },
                    "required": ["path", "search", "replace"],
                    "additionalProperties": false
                },
                "minItems": 1
            }
        },
        "anyOf": [
            { "required": ["patch"] },
            { "required": ["patches"] },
            { "required": ["edits"] }
        ],
        "additionalProperties": false,
        "examples": [
            { "patch": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch" },
            { "edits": [{ "path": "src/lib.rs", "search": "old\n", "replace": "new\n" }] }
        ]
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Structured patch: parse → plan → commit with rollback
// ═══════════════════════════════════════════════════════════════════════════

fn is_structured_patch(patch: &str) -> bool {
    patch.trim_start().starts_with("*** Begin Patch")
}

fn apply_structured_patches(project_root: &Path, patches: &[String]) -> Result<usize> {
    let mut all_ops = Vec::new();
    for patch in patches {
        all_ops.extend(parse_structured_patch(patch)?);
    }

    let backup_paths = structured_backup_paths(project_root, &all_ops)?;
    let backups = collect_path_backups(backup_paths)?;
    let mut files_patched = 0;
    for patch in patches {
        let ops = match parse_structured_patch(patch) {
            Ok(ops) => ops,
            Err(err) => {
                rollback_backups(backups);
                return Err(err);
            }
        };
        let changes = match plan_structured_changes(project_root, &ops) {
            Ok(c) => c,
            Err(err) => {
                rollback_backups(backups);
                return Err(err);
            }
        };
        files_patched += changes.len();
        if let Err(err) = write_planned_changes(&changes) {
            rollback_backups(backups);
            return Err(err);
        }
    }
    Ok(files_patched)
}

fn parse_structured_patch(patch: &str) -> Result<Vec<StructuredOp>> {
    let mut lines: Vec<&str> = patch.lines().collect();
    // Remove trailing empty lines.
    while lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        lines.pop();
    }
    let mut peek_iter = lines.into_iter().peekable();

    let Some(first) = peek_iter.next() else {
        bail!("empty patch");
    };
    if first.trim() != "*** Begin Patch" {
        bail!("missing *** Begin Patch header");
    }

    let mut ops = Vec::new();
    while let Some(line) = peek_iter.next() {
        if line.trim() == "*** End Patch" {
            return Ok(ops);
        }
        // Skip environment ID preamble (Codex feature).
        if line.starts_with("*** Environment ID: ") {
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let mut new_lines = Vec::new();
            while let Some(next) = peek_iter.peek().copied() {
                if next.starts_with("*** ") {
                    break;
                }
                let line = peek_iter.next().expect("peeked line exists");
                let Some(content) = line.strip_prefix('+') else {
                    bail!("add file lines must start with `+` for {path}");
                };
                new_lines.push(content.to_string());
            }
            ops.push(StructuredOp::Add {
                path: path.to_string(),
                lines: new_lines,
            });
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            ops.push(StructuredOp::Delete {
                path: path.to_string(),
            });
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Update File: ") {
            let mut move_to = None;
            let mut hunks = Vec::new();
            while let Some(next) = peek_iter.peek().copied() {
                if next.starts_with("*** Update File: ")
                    || next.starts_with("*** Add File: ")
                    || next.starts_with("*** Delete File: ")
                    || next.trim() == "*** End Patch"
                {
                    break;
                }
                let line = peek_iter.next().expect("peeked line exists");
                if let Some(target) = line.strip_prefix("*** Move to: ") {
                    move_to = Some(target.to_string());
                    continue;
                }
                if line.starts_with("@@") {
                    let mut hunk = Vec::new();
                    while let Some(hunk_line) = peek_iter.peek().copied() {
                        if hunk_line.starts_with("@@")
                            || hunk_line.starts_with("*** ")
                            || hunk_line.trim() == "*** End of File"
                        {
                            break;
                        }
                        hunk.push(parse_hunk_line(
                            peek_iter.next().expect("peeked line exists"),
                        )?);
                    }
                    hunks.push(hunk);
                    continue;
                }
                // Allow empty lines between hunks.
                if line.trim().is_empty() {
                    continue;
                }
                bail!("unexpected line in update for {path}: {line}");
            }
            ops.push(StructuredOp::Update {
                path: path.to_string(),
                move_to,
                hunks,
            });
            continue;
        }
        // Allow blank lines between ops.
        if line.trim().is_empty() {
            continue;
        }
        bail!("unexpected patch line: {line}");
    }
    bail!("missing *** End Patch footer");
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StructuredOp {
    Add {
        path: String,
        lines: Vec<String>,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        hunks: Vec<Vec<HunkLine>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

fn parse_hunk_line(line: &str) -> Result<HunkLine> {
    if let Some(content) = line.strip_prefix(' ') {
        Ok(HunkLine::Context(content.to_string()))
    } else if let Some(content) = line.strip_prefix('-') {
        Ok(HunkLine::Remove(content.to_string()))
    } else if let Some(content) = line.strip_prefix('+') {
        Ok(HunkLine::Add(content.to_string()))
    } else if line.is_empty() {
        Ok(HunkLine::Context(String::new()))
    } else {
        bail!("hunk lines must start with space, `-`, or `+`: {line}")
    }
}

#[derive(Debug, Clone)]
enum PlannedChange {
    Write {
        path: PathBuf,
        lines: Vec<String>,
        trailing_newline: bool,
    },
    Delete {
        path: PathBuf,
    },
    Update {
        source: PathBuf,
        target: PathBuf,
        lines: Vec<String>,
        trailing_newline: bool,
    },
}

fn plan_structured_changes(
    project_root: &Path,
    ops: &[StructuredOp],
) -> Result<Vec<PlannedChange>> {
    let mut changes = Vec::new();
    for op in ops {
        match op {
            StructuredOp::Add { path, lines } => {
                let full = checked_project_path(project_root, path)?;
                if full.exists() {
                    bail!("file already exists: {path}");
                }
                changes.push(PlannedChange::Write {
                    path: full,
                    lines: lines.clone(),
                    trailing_newline: true,
                });
            }
            StructuredOp::Delete { path } => {
                let full = checked_project_path(project_root, path)?;
                if !full.exists() {
                    bail!("file does not exist: {path}");
                }
                changes.push(PlannedChange::Delete { path: full });
            }
            StructuredOp::Update {
                path,
                move_to,
                hunks,
            } => {
                changes.push(plan_structured_update(
                    project_root,
                    path,
                    move_to.as_deref(),
                    hunks,
                )?);
            }
        }
    }
    Ok(changes)
}

fn plan_structured_update(
    project_root: &Path,
    path: &str,
    move_to: Option<&str>,
    hunks: &[Vec<HunkLine>],
) -> Result<PlannedChange> {
    let source = checked_project_path(project_root, path)?;
    let content = fs::read_to_string(&source).with_context(|| format!("failed to read {path}"))?;
    let had_trailing_newline = content.ends_with('\n');
    let old_lines: Vec<String> = content.lines().map(str::to_string).collect();
    let new_lines = apply_hunks(&old_lines, hunks)?;
    let target = if let Some(move_to) = move_to {
        checked_project_path(project_root, move_to)?
    } else {
        source.clone()
    };
    Ok(PlannedChange::Update {
        source,
        target,
        lines: new_lines,
        trailing_newline: had_trailing_newline,
    })
}

fn apply_hunks(old_lines: &[String], hunks: &[Vec<HunkLine>]) -> Result<Vec<String>> {
    let mut result = Vec::new();
    let mut cursor = 0usize;
    for hunk in hunks {
        let pos = find_hunk_position(old_lines, cursor, hunk)
            .or_else(|| {
                // Fall back to fuzzy match with normalised lines.
                let normalised: Vec<String> = old_lines.iter().map(|l| normalise_line(l)).collect();
                find_hunk_position(&normalised, cursor, hunk).map(|_p| {
                    // Walk past any non-matching context lines that were matched fuzzily.
                    let mut _actual_cursor = cursor;
                    let expected: Vec<&String> = hunk
                        .iter()
                        .filter_map(|line| match line {
                            HunkLine::Context(c) | HunkLine::Remove(c) => Some(c),
                            HunkLine::Add(_) => None,
                        })
                        .collect();
                    for exp in &expected {
                        while _actual_cursor < old_lines.len() {
                            if normalise_line(&old_lines[_actual_cursor]) == **exp {
                                _actual_cursor += 1;
                                break;
                            }
                            _actual_cursor += 1;
                        }
                    }
                    _actual_cursor
                })
            })
            .with_context(|| "hunk context did not match target file")?;
        result.extend_from_slice(&old_lines[cursor..pos]);
        cursor = pos;
        for line in hunk {
            match line {
                HunkLine::Context(content) => {
                    // Prefer exact match; fall back to trimmed-end and fuzzy.
                    let actual = old_lines.get(cursor).map(|s| s.as_str()).unwrap_or("");
                    if actual != content
                        && actual.trim_end() != content.trim_end()
                        && normalise_line(actual) != normalise_line(content)
                    {
                        bail!("context mismatch at line {}", cursor + 1);
                    }
                    result.push(old_lines[cursor].clone());
                    cursor += 1;
                }
                HunkLine::Remove(content) => {
                    let actual = old_lines.get(cursor).map(|s| s.as_str()).unwrap_or("");
                    if actual != content
                        && actual.trim_end() != content.trim_end()
                        && normalise_line(actual) != normalise_line(content)
                    {
                        bail!("remove mismatch at line {}", cursor + 1);
                    }
                    cursor += 1;
                }
                HunkLine::Add(content) => result.push(content.clone()),
            }
        }
    }
    result.extend_from_slice(&old_lines[cursor..]);
    Ok(result)
}

fn find_hunk_position(old_lines: &[String], start: usize, hunk: &[HunkLine]) -> Option<usize> {
    let expected: Vec<&String> = hunk
        .iter()
        .filter_map(|line| match line {
            HunkLine::Context(content) | HunkLine::Remove(content) => Some(content),
            HunkLine::Add(_) => None,
        })
        .collect();
    if expected.is_empty() {
        return Some(start);
    }
    // Try exact match first.
    let exact = (start..=old_lines.len().saturating_sub(expected.len())).find(|&pos| {
        expected
            .iter()
            .enumerate()
            .all(|(offset, line)| old_lines.get(pos + offset) == Some(line))
    });
    if exact.is_some() {
        return exact;
    }
    // Fall back to whitespace-insensitive match.
    let trimmed_old: Vec<String> = old_lines.iter().map(|l| l.trim_end().to_string()).collect();
    let trimmed_expected: Vec<String> = expected.iter().map(|l| l.trim_end().to_string()).collect();
    let trimmed = (start..=trimmed_old.len().saturating_sub(trimmed_expected.len())).find(|&pos| {
        trimmed_expected
            .iter()
            .enumerate()
            .all(|(offset, line)| trimmed_old.get(pos + offset) == Some(line))
    });
    if trimmed.is_some() {
        return trimmed;
    }
    // Fall back to normalised (fuzzy unicode) match.
    let normalised_old: Vec<String> = old_lines.iter().map(|l| normalise_line(l)).collect();
    let normalised_expected: Vec<String> = expected.iter().map(|l| normalise_line(l)).collect();
    (start
        ..=normalised_old
            .len()
            .saturating_sub(normalised_expected.len()))
        .find(|&pos| {
            normalised_expected
                .iter()
                .enumerate()
                .all(|(offset, line)| {
                    normalised_old.get(pos + offset).map(|s| s.as_str()) == Some(line.as_str())
                })
        })
}

fn structured_backup_paths(project_root: &Path, ops: &[StructuredOp]) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for op in ops {
        match op {
            StructuredOp::Add { path, .. } | StructuredOp::Delete { path } => {
                push_unique_path(&mut paths, checked_project_path(project_root, path)?);
            }
            StructuredOp::Update { path, move_to, .. } => {
                push_unique_path(&mut paths, checked_project_path(project_root, path)?);
                if let Some(move_to) = move_to {
                    push_unique_path(&mut paths, checked_project_path(project_root, move_to)?);
                }
            }
        }
    }
    Ok(paths)
}

fn collect_path_backups(paths: Vec<PathBuf>) -> Result<Vec<(PathBuf, Option<Vec<u8>>)>> {
    paths
        .into_iter()
        .map(|path| {
            let content = if path.exists() {
                Some(
                    fs::read(&path)
                        .with_context(|| format!("failed to back up {}", path.display()))?,
                )
            } else {
                None
            };
            Ok((path, content))
        })
        .collect()
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

fn write_planned_changes(changes: &[PlannedChange]) -> Result<()> {
    for change in changes {
        match change {
            PlannedChange::Write {
                path,
                lines,
                trailing_newline,
            } => write_lines(path, lines, *trailing_newline)?,
            PlannedChange::Delete { path } => fs::remove_file(path)
                .with_context(|| format!("failed to delete {}", path.display()))?,
            PlannedChange::Update {
                source,
                target,
                lines,
                trailing_newline,
            } => {
                write_lines(target, lines, *trailing_newline)?;
                if target != source {
                    fs::remove_file(source).with_context(|| {
                        format!("failed to remove moved file {}", source.display())
                    })?;
                }
            }
        }
    }
    Ok(())
}

fn rollback_backups(backups: Vec<(PathBuf, Option<Vec<u8>>)>) {
    for (path, content) in backups {
        match content {
            Some(content) => {
                if let Some(parent) = path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::write(path, content);
            }
            None => {
                let _ = fs::remove_file(path);
            }
        }
    }
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

fn write_lines(path: &Path, lines: &[String], trailing_newline: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut content = lines.join("\n");
    if trailing_newline && !content.ends_with('\n') {
        content.push('\n');
    }
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Unified diff: git apply + patch fallback
// ═══════════════════════════════════════════════════════════════════════════

async fn run_git_apply(project_root: &Path, patch: &str) -> Result<std::process::Output> {
    let mut child = Command::new("git")
        .args(["apply", "--whitespace=fix", "-"])
        .current_dir(project_root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn git apply")?;
    child
        .stdin
        .as_mut()
        .context("failed to open git apply stdin")?
        .write_all(patch.as_bytes())
        .await
        .context("failed to send patch to git apply")?;
    child
        .wait_with_output()
        .await
        .context("failed to wait for git apply")
}

async fn run_git_apply_relaxed(project_root: &Path, patch: &str) -> Result<std::process::Output> {
    let mut child = Command::new("git")
        .args([
            "apply",
            "--whitespace=fix",
            "--ignore-space-change",
            "--ignore-whitespace",
            "-",
        ])
        .current_dir(project_root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn git apply")?;
    child
        .stdin
        .as_mut()
        .context("failed to open git apply stdin")?
        .write_all(patch.as_bytes())
        .await
        .context("failed to send patch to git apply")?;
    child
        .wait_with_output()
        .await
        .context("failed to wait for git apply")
}

async fn run_patch_command(project_root: &Path, patch: &str) -> Result<std::process::Output> {
    let mut child = Command::new("patch")
        .args(["-p1", "--force", "--no-backup-if-mismatch"])
        .current_dir(project_root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn patch")?;
    child
        .stdin
        .as_mut()
        .context("failed to open patch stdin")?
        .write_all(patch.as_bytes())
        .await
        .context("failed to send patch to patch command")?;
    child
        .wait_with_output()
        .await
        .context("failed to wait for patch command")
}

fn extract_patched_files(patch: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("--- a/")
            && !files.contains(&path.to_string())
        {
            files.push(path.to_string());
        }
        if let Some(path) = line.strip_prefix("+++ b/")
            && !files.contains(&path.to_string())
        {
            files.push(path.to_string());
        }
    }
    files
}

// ═══════════════════════════════════════════════════════════════════════════
// Patch error output and context
// ═══════════════════════════════════════════════════════════════════════════

fn patch_failed_output(
    error_code: &str,
    message: impl Into<String>,
    hint: &str,
    stderr: Option<String>,
    context_lines: Vec<Value>,
) -> Value {
    let mut output = helpers::tool_error(error_code, message, true, Some(hint), stderr);
    if !context_lines.is_empty()
        && let Value::Object(object) = &mut output
    {
        object.insert("context_lines".to_string(), Value::Array(context_lines));
        object.insert(
            "context_note".to_string(),
            Value::String(
                "Each context window includes up to 20 lines before and 20 lines after \
                 the nearest relevant patch hunk location."
                    .to_string(),
            ),
        );
    }
    output
}

fn patch_failure_contexts(project_root: &Path, patches: &[String]) -> Vec<Value> {
    let mut contexts = Vec::new();
    for patch in patches {
        if is_structured_patch(patch) {
            contexts.extend(structured_patch_contexts(project_root, patch));
        } else {
            contexts.extend(unified_patch_contexts(project_root, patch));
        }
        if contexts.len() >= MAX_PATCH_CONTEXT_WINDOWS {
            contexts.truncate(MAX_PATCH_CONTEXT_WINDOWS);
            break;
        }
    }
    contexts
}

fn structured_patch_contexts(project_root: &Path, patch: &str) -> Vec<Value> {
    let Ok(ops) = parse_structured_patch(patch) else {
        return extract_structured_patch_paths(patch)
            .into_iter()
            .filter_map(|path| file_context_window(project_root, &path, 1))
            .collect();
    };

    let mut contexts = Vec::new();
    for op in ops {
        match op {
            StructuredOp::Update { path, hunks, .. } => {
                for hunk in &hunks {
                    let preferred_line = preferred_structured_hunk_line(project_root, &path, hunk);
                    if let Some(context) = file_context_window(project_root, &path, preferred_line)
                    {
                        contexts.push(context);
                    }
                }
            }
            StructuredOp::Delete { path } => {
                if let Some(context) = file_context_window(project_root, &path, 1) {
                    contexts.push(context);
                }
            }
            StructuredOp::Add { .. } => {}
        }
    }
    contexts
}

fn extract_structured_patch_paths(patch: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("*** Update File: ") {
            paths.push(PathBuf::from(path));
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            paths.push(PathBuf::from(path));
        }
    }
    paths
        .into_iter()
        .filter_map(|path| path.to_str().map(str::to_string))
        .collect()
}

fn preferred_structured_hunk_line(project_root: &Path, path: &str, hunk: &[HunkLine]) -> usize {
    let Ok(full_path) = checked_project_path(project_root, path) else {
        return 1;
    };
    let Ok(content) = fs::read_to_string(full_path) else {
        return 1;
    };
    let lines: Vec<String> = content.lines().map(str::to_string).collect();
    if let Some(pos) = find_hunk_position(&lines, 0, hunk) {
        return pos + 1;
    }
    hunk.iter()
        .filter_map(|line| match line {
            HunkLine::Context(content) | HunkLine::Remove(content) if !content.is_empty() => {
                Some(content)
            }
            _ => None,
        })
        .find_map(|expected| {
            lines
                .iter()
                .position(|line| line == expected)
                .or_else(|| {
                    let normalised_expected = normalise_line(expected);
                    lines
                        .iter()
                        .position(|line| normalise_line(line) == normalised_expected)
                })
                .map(|pos| pos + 1)
        })
        .unwrap_or(1)
}

fn unified_patch_contexts(project_root: &Path, patch: &str) -> Vec<Value> {
    let mut contexts = Vec::new();
    let mut _old_path: Option<String> = None;
    let mut current_path: Option<String> = None;

    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("--- ") {
            _old_path = clean_unified_path(path);
        } else if let Some(path) = line.strip_prefix("+++ ") {
            current_path = clean_unified_path(path).or_else(|| _old_path.clone());
        } else if line.starts_with("@@") {
            let preferred_line = parse_unified_old_start(line).unwrap_or(1);
            if let Some(path) = current_path.as_deref()
                && let Some(context) = file_context_window(project_root, path, preferred_line)
            {
                contexts.push(context);
            }
        }
    }
    contexts
}

fn clean_unified_path(path: &str) -> Option<String> {
    let path = path.split_whitespace().next().unwrap_or(path);
    if path == "/dev/null" {
        return None;
    }
    Some(
        path.strip_prefix("a/")
            .or_else(|| path.strip_prefix("b/"))
            .unwrap_or(path)
            .to_string(),
    )
}

fn parse_unified_old_start(hunk_header: &str) -> Option<usize> {
    let after_dash = hunk_header.split_once('-')?.1;
    let number = after_dash
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    number.parse::<usize>().ok().filter(|line| *line > 0)
}

fn file_context_window(project_root: &Path, path: &str, preferred_line: usize) -> Option<Value> {
    let full_path = checked_project_path(project_root, path).ok()?;
    let content = fs::read_to_string(full_path).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let preferred_line = preferred_line.clamp(1, lines.len());
    let start_line = preferred_line.saturating_sub(PATCH_CONTEXT_RADIUS).max(1);
    let end_line = (preferred_line + PATCH_CONTEXT_RADIUS).min(lines.len());
    let context = (start_line..=end_line)
        .map(|line| {
            json!({
                "line": line,
                "text": lines[line - 1],
            })
        })
        .collect::<Vec<_>>();

    Some(json!({
        "path": path,
        "start_line": start_line,
        "end_line": end_line,
        "lines": context,
    }))
}

fn git_apply_error_hint(stderr: &str) -> &'static str {
    let lower = stderr.to_lowercase();
    if lower.contains("corrupt patch") {
        "Patch is malformed. Ensure it uses valid unified diff format: \
         --- a/path, +++ b/path, @@ hunk headers with line counts, and context lines \
         (starting with space) that exactly match the file on disk."
    } else if lower.contains("patch does not apply") || lower.contains("does not apply") {
        "The patch context lines don't match the file on disk. Re-read the file with \
         read_file and regenerate the diff against the content you see. Ensure the @@ hunk \
         line numbers and counts are correct for the target file."
    } else if lower.contains("no such file or directory") {
        "The target file doesn't exist. For new files, use --- /dev/null and \
         +++ b/newfile/path. For renames, ensure both old and new paths are correct."
    } else if lower.contains("already exists") {
        "The file already exists. To modify an existing file, use --- a/path and +++ b/path. \
         For new files, the file must not already exist."
    } else if lower.contains("permission denied") {
        "Permission denied. Check file permissions on the target file or directory."
    } else {
        "Check that the patch uses unified diff format with correct --- a/ and +++ b/ headers, \
         @@ hunk headers with accurate line numbers, and context lines that match the file \
         content. Re-read the file before regenerating the patch."
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    // ── Direct write tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_direct_write_creates_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-1".into(),
                tool_name: "write".into(),
                input: json!({
                    "path": "test.txt",
                    "content": "hello\nworld\n",
                }),
            })
            .await
            .unwrap();
        assert!(result.ok);
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello\nworld\n");
        let output = result.output;
        assert_eq!(output["path"], "test.txt");
        assert_eq!(output["lines_added"], 2);
        assert_eq!(output["lines_removed"], 0);
    }

    #[tokio::test]
    async fn test_direct_write_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a").join("b").join("c.txt");
        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-2".into(),
                tool_name: "write".into(),
                input: json!({
                    "path": "a/b/c.txt",
                    "content": "deep",
                }),
            })
            .await
            .unwrap();
        assert!(result.ok);
        assert_eq!(fs::read_to_string(&path).unwrap(), "deep");
    }

    #[tokio::test]
    async fn test_direct_write_overwrites_existing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("overwrite.txt");
        fs::write(&path, "old\ncontent\n").unwrap();
        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-3".into(),
                tool_name: "write".into(),
                input: json!({
                    "path": "overwrite.txt",
                    "content": "new content",
                }),
            })
            .await
            .unwrap();
        assert!(result.ok);
        assert_eq!(fs::read_to_string(&path).unwrap(), "new content");
        // Should report lines removed.
        assert!(result.output["lines_removed"].as_u64().unwrap_or(0) > 0);
    }

    // ── Search/replace edits (simple surgical edits) ─────────────────────

    #[tokio::test]
    async fn test_search_replace_edits() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        fs::write(&path, "fn old() -> i32 {\n    1\n}\n").unwrap();

        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-edits".into(),
                tool_name: "write".into(),
                input: json!({
                    "edits": [
                        { "path": "lib.rs", "search": "fn old() -> i32 {\n    1\n}", "replace": "fn new() -> i32 {\n    2\n}" }
                    ]
                }),
            })
            .await
            .unwrap();
        assert!(result.ok, "edits failed: {:?}", result.output);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "fn new() -> i32 {\n    2\n}\n"
        );
    }

    #[tokio::test]
    async fn test_search_replace_edits_multiple() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        fs::write(&path, "fn a() {}\nfn b() {}\n").unwrap();

        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-edits-multi".into(),
                tool_name: "write".into(),
                input: json!({
                    "edits": [
                        { "path": "lib.rs", "search": "fn a() {}", "replace": "fn a_new() {}" },
                        { "path": "lib.rs", "search": "fn b() {}", "replace": "fn b_new() {}" }
                    ]
                }),
            })
            .await
            .unwrap();
        assert!(result.ok, "edits failed: {:?}", result.output);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "fn a_new() {}\nfn b_new() {}\n"
        );
    }

    #[tokio::test]
    async fn test_search_replace_edits_missing_block_fails() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        fs::write(&path, "fn a() {}\n").unwrap();

        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-edits-missing".into(),
                tool_name: "write".into(),
                input: json!({
                    "edits": [
                        { "path": "lib.rs", "search": "fn missing() {}", "replace": "fn x() {}" }
                    ]
                }),
            })
            .await
            .unwrap();
        assert!(!result.ok);
        assert_eq!(result.output["error_code"], "edit_failed");
    }

    #[tokio::test]
    async fn test_search_replace_edits_ignores_trailing_newline_difference() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        fs::write(&path, "fn a() {}\n").unwrap();

        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-edits-nl".into(),
                tool_name: "write".into(),
                input: json!({
                    "edits": [
                        { "path": "lib.rs", "search": "fn a() {}", "replace": "fn b() {}" }
                    ]
                }),
            })
            .await
            .unwrap();
        assert!(result.ok, "edits failed: {:?}", result.output);
        assert_eq!(fs::read_to_string(&path).unwrap(), "fn b() {}\n");
    }

    #[tokio::test]
    async fn test_apply_patch_alias_accepts_edits() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        fs::write(&path, "const X: i32 = 1;\n").unwrap();

        let tool = WriteTool::apply_patch(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-alias-edits".into(),
                tool_name: "apply_patch".into(),
                input: json!({
                    "edits": [
                        { "path": "lib.rs", "search": "const X: i32 = 1;", "replace": "const X: i32 = 2;" }
                    ]
                }),
            })
            .await
            .unwrap();
        assert!(
            result.ok,
            "apply_patch alias edits failed: {:?}",
            result.output
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "const X: i32 = 2;\n");
    }

    // ── Structured patch tests ─────────────────────────────────────────

    #[tokio::test]
    async fn test_structured_add_file() {
        let dir = tempdir().unwrap();
        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-add".into(),
                tool_name: "write".into(),
                input: json!({
                    "patch": "*** Begin Patch\n*** Add File: new.txt\n+hello\n+world\n*** End Patch",
                }),
            })
            .await
            .unwrap();
        assert!(result.ok, "patch failed: {:?}", result.output);
        let path = dir.path().join("new.txt");
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello\nworld\n");
    }

    #[tokio::test]
    async fn test_structured_update_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("edit.txt");
        fs::write(&path, "foo\nbar\nbaz\n").unwrap();

        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-upd".into(),
                tool_name: "write".into(),
                input: json!({
                    "patch": format!(
                        "*** Begin Patch\n*** Update File: edit.txt\n@@\n foo\n-bar\n+BAR\n*** End Patch"
                    ),
                }),
            })
            .await
            .unwrap();
        assert!(result.ok, "patch failed: {:?}", result.output);
        assert_eq!(fs::read_to_string(&path).unwrap(), "foo\nBAR\nbaz\n");
    }

    #[tokio::test]
    async fn test_structured_update_accepts_unprefixed_context_lines() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        fs::write(&path, "pub fn target() -> i32 {\n    1\n}\n").unwrap();

        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-unprefixed-context".into(),
                tool_name: "write".into(),
                input: json!({
                    "patch": "*** Begin Patch\n*** Update File: lib.rs\n@@\npub fn target() -> i32 {\n-    1\n+    2\n}\n*** End Patch",
                }),
            })
            .await
            .unwrap();

        assert!(result.ok, "patch failed: {:?}", result.output);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "pub fn target() -> i32 {\n    2\n}\n"
        );
    }

    #[tokio::test]
    async fn test_malformed_structured_patch_can_recover_symbol_replacement() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        fs::write(
            &path,
            "#[derive(Debug)]\npub struct SymbolRecord;\n\npub fn search_symbols() -> i32 {\n    1\n}\n",
        )
        .unwrap();

        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-symbol-fallback".into(),
                tool_name: "write".into(),
                input: json!({
                    "patch": "*** Begin Patch\n*** Update File: lib.rs\n@@\n-use std::collections::HashSet;\n+\n #[derive(Debug)]\n@@\n-use std::collections::HashSet;\n pub fn search_symbols() -> i32 {\n     1\n@@\n pub fn search_symbols() -> i32 {\n     2\n }\n*** End Patch",
                }),
            })
            .await
            .unwrap();

        assert!(result.ok, "patch failed: {:?}", result.output);
        assert_eq!(
            result.output["method"],
            "structured_symbol_replacement_fallback"
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "#[derive(Debug)]\npub struct SymbolRecord;\n\npub fn search_symbols() -> i32 {\n    2\n}\n"
        );
    }

    #[tokio::test]
    async fn test_structured_delete_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("del.txt");
        fs::write(&path, "delete me\n").unwrap();

        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-del".into(),
                tool_name: "write".into(),
                input: json!({
                    "patch": "*** Begin Patch\n*** Delete File: del.txt\n*** End Patch",
                }),
            })
            .await
            .unwrap();
        assert!(result.ok);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn test_structured_move_file() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        fs::write(&src, "line\n").unwrap();

        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-mv".into(),
                tool_name: "write".into(),
                input: json!({
                    "patch": "*** Begin Patch\n*** Update File: src.txt\n*** Move to: dst.txt\n@@\n-line\n+line2\n*** End Patch",
                }),
            })
            .await
            .unwrap();
        assert!(result.ok, "patch failed: {:?}", result.output);
        assert!(!src.exists());
        assert_eq!(fs::read_to_string(&dst).unwrap(), "line2\n");
    }

    #[tokio::test]
    async fn test_multiple_patches() {
        let dir = tempdir().unwrap();
        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-multi".into(),
                tool_name: "write".into(),
                input: json!({
                    "patches": [
                        "*** Begin Patch\n*** Add File: a.txt\n+aaa\n*** End Patch",
                        "*** Begin Patch\n*** Add File: b.txt\n+bbb\n*** End Patch",
                    ],
                }),
            })
            .await
            .unwrap();
        assert!(result.ok, "multi-patch failed: {:?}", result.output);
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "aaa\n"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "bbb\n"
        );
    }

    // ── Heredoc stripping test (Codex feature) ─────────────────────────

    #[tokio::test]
    async fn test_heredoc_stripping() {
        let dir = tempdir().unwrap();
        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-heredoc".into(),
                tool_name: "write".into(),
                input: json!({
                    "patch": "<<'EOF'\n*** Begin Patch\n*** Add File: from_heredoc.txt\n+heredoc content\n*** End Patch\nEOF",
                }),
            })
            .await
            .unwrap();
        assert!(result.ok, "heredoc patch failed: {:?}", result.output);
        let path = dir.path().join("from_heredoc.txt");
        assert_eq!(fs::read_to_string(&path).unwrap(), "heredoc content\n");
    }

    // ── Verification failure test (Codex feature) ──────────────────────

    #[tokio::test]
    async fn test_verification_detects_mismatch() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("target.txt");
        fs::write(&path, "line1\nline2\n").unwrap();

        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-verify".into(),
                tool_name: "write".into(),
                input: json!({
                    "patch": "*** Begin Patch\n*** Update File: target.txt\n@@\n-nonexistent\n+replacement\n*** End Patch",
                }),
            })
            .await
            .unwrap();
        assert!(!result.ok, "patch should have failed verification");
        let err = result.output["error_code"].as_str().unwrap_or("");
        assert_eq!(err, "verification_failed");
    }

    // ── Environment ID preamble (Codex feature) ────────────────────────

    #[tokio::test]
    async fn test_environment_id_preamble() {
        let dir = tempdir().unwrap();
        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-env".into(),
                tool_name: "write".into(),
                input: json!({
                    "patch": "*** Begin Patch\n*** Environment ID: remote\n*** Add File: env.txt\n+hello\n*** End Patch",
                }),
            })
            .await
            .unwrap();
        assert!(result.ok, "env-id patch failed: {:?}", result.output);
        let path = dir.path().join("env.txt");
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello\n");
    }

    // ── No args error ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_no_args_error() {
        let dir = tempdir().unwrap();
        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-noargs".into(),
                tool_name: "write".into(),
                input: json!({}),
            })
            .await
            .unwrap();
        assert!(!result.ok);
    }

    // ── Patch affected files utility ───────────────────────────────────

    #[test]
    fn test_patch_affected_files_structured() {
        let patch = "*** Begin Patch\n*** Add File: new.txt\n+content\n*** Update File: old.txt\n@@\n ctx\n-old\n+new\n*** Delete File: gone.txt\n*** End Patch"
            .to_string();
        let files = patch_affected_files(&[patch]).unwrap();
        assert_eq!(files, vec!["gone.txt", "new.txt", "old.txt"]);
    }

    #[test]
    fn test_patch_affected_files_unified() {
        let patch = "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old\n+new".to_string();
        let files = patch_affected_files(&[patch]).unwrap();
        assert_eq!(files, vec!["src/main.rs"]);
    }

    // ── Fuzzy matching test ────────────────────────────────────────────

    #[tokio::test]
    async fn test_fuzzy_unicode_match() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("unicode.py");
        // Original contains EN DASH (U+2013) and NON-BREAKING HYPHEN (U+2011)
        let original = "import asyncio  # local import \u{2013} avoids top\u{2011}level dep\n";
        fs::write(&path, original).unwrap();

        // Patch uses plain ASCII dash.
        let tool = WriteTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test-fuzzy".into(),
                tool_name: "write".into(),
                input: json!({
                    "patch": format!(
                        "*** Begin Patch\n*** Update File: unicode.py\n@@\n-import asyncio  # local import - avoids top-level dep\n+import asyncio  # HELLO\n*** End Patch"
                    ),
                }),
            })
            .await
            .unwrap();
        assert!(result.ok, "fuzzy patch failed: {:?}", result.output);
        let expected = "import asyncio  # HELLO\n";
        assert_eq!(fs::read_to_string(&path).unwrap(), expected);
    }
}
