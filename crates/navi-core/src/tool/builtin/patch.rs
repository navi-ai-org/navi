use crate::file_lock::{FileLockManager, LockGuard};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

const PATCH_CONTEXT_RADIUS: usize = 20;
const MAX_PATCH_CONTEXT_WINDOWS: usize = 6;

pub(crate) struct ApplyPatchTool {
    project_root: PathBuf,
    lock_manager: Option<std::sync::Arc<FileLockManager>>,
}

impl ApplyPatchTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self {
            project_root,
            lock_manager: None,
        }
    }

    pub(crate) fn with_lock_manager(
        project_root: PathBuf,
        lock_manager: std::sync::Arc<FileLockManager>,
    ) -> Self {
        Self {
            project_root,
            lock_manager: Some(lock_manager),
        }
    }
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "apply_patch",
            "Apply one or more patches to the current project. Use exactly one input shape: {patch: string} for one patch, or {patches: string[]} for multiple patches. Prefer structured patches: *** Begin Patch, file operation headers, @@ hunks, then *** End Patch. Do not pass path/content/old_string/new_string fields and do not wrap patches in markdown fences.",
            ToolKind::Write,
            apply_patch_json_schema(),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let patches = patch_inputs(&invocation.input)?;
        let affected = patch_affected_files(&patches)?;
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
                                    "O arquivo `{file}` está bloqueado por outra instância do NAVI. Use a ferramenta `wait` com `file_path=\"{file}\"` para aguardar."
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

        if patches.iter().all(|patch| is_structured_patch(patch)) {
            return match apply_structured_patches(&self.project_root, &patches) {
                Ok(files_patched) => Ok(ToolResult {
                    invocation_id: invocation.id,
                    ok: true,
                    output: json!({
                        "method": "structured apply_patch",
                        "status": 0,
                        "patches_applied": patches.len(),
                        "files_patched": files_patched,
                    }),
                }),
                Err(err) => Ok(ToolResult {
                    invocation_id: invocation.id,
                    ok: false,
                    output: patch_failed_output(
                        "patch_failed",
                        format!("structured apply_patch failed: {err:#}"),
                        "Use one input object with either `patch` or `patches`. Rebuild the structured patch with exact context from `context_lines`: *** Begin Patch, file operation headers, @@ hunks, and *** End Patch.",
                        None,
                        patch_failure_contexts(&self.project_root, &patches),
                    ),
                }),
            };
        }

        let patch = patches.join("\n");
        for file in &affected {
            let full = self.project_root.join(file);
            if let Some(parent) = full.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
        }

        // Stage 1: git apply with --whitespace=fix.
        let git_result = run_git_apply(&self.project_root, &patch).await?;

        if git_result.status.success() {
            return Ok(ToolResult {
                invocation_id: invocation.id,
                ok: true,
                output: json!({
                    "method": "git apply",
                    "status": git_result.status.code(),
                    "patches_applied": patches.len(),
                    "stdout": String::from_utf8_lossy(&git_result.stdout),
                    "stderr": String::from_utf8_lossy(&git_result.stderr),
                    "files_patched": affected.len(),
                }),
            });
        }

        let git_stderr = String::from_utf8_lossy(&git_result.stderr).to_string();

        // Stage 2: fall back to `patch -p1`.
        let patch_result = run_patch_command(&self.project_root, &patch).await?;

        if patch_result.status.success() {
            return Ok(ToolResult {
                invocation_id: invocation.id,
                ok: true,
                output: json!({
                    "method": "patch",
                    "status": patch_result.status.code(),
                    "patches_applied": patches.len(),
                    "stdout": String::from_utf8_lossy(&patch_result.stdout),
                    "stderr": String::from_utf8_lossy(&patch_result.stderr),
                    "files_patched": affected.len(),
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
                    "git apply failed: {}\npatch -p1 failed: {}",
                    git_stderr.trim(),
                    patch_stderr.trim()
                ),
                hint,
                Some(format!(
                    "git apply stderr: {}\npatch stderr: {}",
                    git_stderr.trim(),
                    patch_stderr.trim()
                )),
                patch_failure_contexts(&self.project_root, &patches),
            ),
        })
    }
}

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

fn patch_inputs(input: &Value) -> Result<Vec<String>> {
    let has_patch = input.get("patch").is_some();
    let has_patches = input.get("patches").is_some();
    if has_patch == has_patches {
        bail!("apply_patch requires exactly one of `patch` or `patches`");
    }
    if has_patch {
        return Ok(vec![helpers::required_string(input, "patch")?.to_string()]);
    }

    let patches = input
        .get("patches")
        .and_then(Value::as_array)
        .context("`patches` must be a non-empty array of patch strings")?;
    if patches.is_empty() {
        bail!("`patches` must contain at least one patch string");
    }
    patches
        .iter()
        .enumerate()
        .map(|(index, patch)| {
            patch
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .with_context(|| format!("patches[{index}] must be a non-empty string"))
        })
        .collect()
}

fn apply_patch_json_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "patch": {
                "type": "string",
                "description": "A single complete patch string. Prefer structured format: *** Begin Patch\n*** Update File: path\n@@\n context line\n-old line\n+new line\n*** End Patch. Add-file content lines must start with +. Unified diff is also accepted.",
                "examples": ["*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch\n"]
            },
            "patches": {
                "type": "array",
                "description": "Multiple complete patch strings to apply as one tool call. Use this instead of making repeated apply_patch calls when you already know several independent edits.",
                "items": { "type": "string" },
                "minItems": 1,
                "examples": [["*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch\n", "*** Begin Patch\n*** Add File: src/new.rs\n+pub fn new() {}\n*** End Patch\n"]]
            }
        },
        "minProperties": 1,
        "maxProperties": 1,
        "additionalProperties": false,
        "examples": [{
            "patch": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch\n"
        }]
    })
}

fn is_structured_patch(patch: &str) -> bool {
    patch.trim_start().starts_with("*** Begin Patch")
}

// ── Structured patch: parse → plan → commit with rollback ────────────────

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
    let mut lines = patch.lines().peekable();
    let Some(first) = lines.next() else {
        bail!("empty patch");
    };
    if first.trim() != "*** Begin Patch" {
        bail!("missing *** Begin Patch header");
    }

    let mut ops = Vec::new();
    while let Some(line) = lines.next() {
        if line.trim() == "*** End Patch" {
            return Ok(ops);
        }
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let mut new_lines = Vec::new();
            while let Some(next) = lines.peek().copied() {
                if next.starts_with("*** ") {
                    break;
                }
                let line = lines.next().expect("peeked line exists");
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
            while let Some(next) = lines.peek().copied() {
                if next.starts_with("*** Update File: ")
                    || next.starts_with("*** Add File: ")
                    || next.starts_with("*** Delete File: ")
                    || next.trim() == "*** End Patch"
                {
                    break;
                }
                let line = lines.next().expect("peeked line exists");
                if let Some(target) = line.strip_prefix("*** Move to: ") {
                    move_to = Some(target.to_string());
                    continue;
                }
                if line.starts_with("@@") {
                    let mut hunk = Vec::new();
                    while let Some(hunk_line) = lines.peek().copied() {
                        if hunk_line.starts_with("@@") || hunk_line.starts_with("*** ") {
                            break;
                        }
                        hunk.push(parse_hunk_line(lines.next().expect("peeked line exists"))?);
                    }
                    hunks.push(hunk);
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
    let old_lines = content.lines().map(str::to_string).collect::<Vec<_>>();
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
            .with_context(|| "hunk context did not match target file")?;
        result.extend_from_slice(&old_lines[cursor..pos]);
        cursor = pos;
        for line in hunk {
            match line {
                HunkLine::Context(content) => {
                    if old_lines.get(cursor) != Some(content) {
                        bail!("context mismatch at line {}", cursor + 1);
                    }
                    result.push(content.clone());
                    cursor += 1;
                }
                HunkLine::Remove(content) => {
                    if old_lines.get(cursor) != Some(content) {
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
    let expected = hunk
        .iter()
        .filter_map(|line| match line {
            HunkLine::Context(content) | HunkLine::Remove(content) => Some(content),
            HunkLine::Add(_) => None,
        })
        .collect::<Vec<_>>();
    if expected.is_empty() {
        return Some(start);
    }
    (start..=old_lines.len().saturating_sub(expected.len())).find(|&pos| {
        expected
            .iter()
            .enumerate()
            .all(|(offset, line)| old_lines.get(pos + offset) == Some(line))
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

// ── Unified diff: git apply + patch fallback ─────────────────────────────

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
                "Each context window includes up to 20 lines before and 20 lines after the nearest relevant patch hunk location.".to_string(),
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
                for hunk in hunks {
                    let preferred_line = preferred_structured_hunk_line(project_root, &path, &hunk);
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
            push_unique_path(&mut paths, PathBuf::from(path));
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            push_unique_path(&mut paths, PathBuf::from(path));
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
    let lines = content.lines().map(str::to_string).collect::<Vec<_>>();
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
                .map(|pos| pos + 1)
        })
        .unwrap_or(1)
}

fn unified_patch_contexts(project_root: &Path, patch: &str) -> Vec<Value> {
    let mut contexts = Vec::new();
    let mut old_path: Option<String> = None;
    let mut current_path: Option<String> = None;

    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("--- ") {
            old_path = clean_unified_path(path);
        } else if let Some(path) = line.strip_prefix("+++ ") {
            current_path = clean_unified_path(path).or_else(|| old_path.clone());
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
    let lines = content.lines().collect::<Vec<_>>();
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
         --- a/path, +++ b/path, @@ hunk headers with line counts, and context lines (starting with space) \
         that exactly match the file on disk."
    } else if lower.contains("patch does not apply") || lower.contains("does not apply") {
        "The patch context lines don't match the file on disk. Re-read the file with read_file and \
         regenerate the diff against the content you see. Ensure the @@ hunk line numbers and counts \
         are correct for the target file."
    } else if lower.contains("no such file or directory") {
        "The target file doesn't exist. For new files, use --- /dev/null and +++ b/newfile/path. \
         For renames, ensure both old and new paths are correct."
    } else if lower.contains("already exists") {
        "The file already exists. To modify an existing file, use --- a/path and +++ b/path. \
         For new files, the file must not already exist."
    } else if lower.contains("permission denied") {
        "Permission denied. Check file permissions on the target file or directory."
    } else {
        "Check that the patch uses unified diff format with correct --- a/ and +++ b/ headers, \
         @@ hunk headers with accurate line numbers, and context lines that match the file content. \
         Re-read the file before regenerating the patch."
    }
}
