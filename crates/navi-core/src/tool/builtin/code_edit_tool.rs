use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use navi_vfs::code::{
    CodeSymbol, InsertPosition, SourceEdit, insert_around_symbol, rename_identifier,
    replace_symbol_definition, symbols_for_source,
};
use navi_vfs::lang::detect_language;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

use super::helpers;
use crate::security::{SecurityDecision, SecurityPolicy};
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

const DEFAULT_MAX_RESULTS: usize = 80;
const MAX_RESULTS: usize = 500;
const DEFAULT_MAX_FILES: usize = 400;
const MAX_FILES: usize = 2_000;
const MAX_FILE_BYTES: u64 = 1024 * 1024;

pub(crate) struct CodeEditTool {
    policy: SecurityPolicy,
}

impl CodeEditTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }
}

#[async_trait]
impl Tool for CodeEditTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "code_edit",
            "Unified code editing tool. Supports replacing symbol bodies, inserting text before/after symbols, and renaming identifiers across files.",
            ToolKind::Write,
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["replace", "insert-before", "insert-after", "rename"],
                        "description": "Operation: replace (symbol body), insert-before, insert-after, rename (identifier across files)."
                    },
                    "path": { "type": "string", "description": "Project-relative source file or directory." },
                    "symbol": { "type": "string", "description": "Symbol id or unique name (for replace/insert actions)." },
                    "content": { "type": "string", "description": "Replacement body or text to insert (for replace/insert actions)." },
                    "replacement": { "type": "string", "description": "Alias for content in replace mode." },
                    "expected_hash": { "type": "string", "description": "Symbol hash to reject stale edits (for replace/insert)." },
                    "old_name": { "type": "string", "description": "Identifier to rename (for rename action)." },
                    "new_name": { "type": "string", "description": "New identifier name (for rename action)." },
                    "dry_run": { "type": "boolean", "description": "Preview changes without writing (for rename action)." },
                    "max_files": { "type": "integer", "description": "Max files to scan (default 400, for rename)." },
                    "max_results": { "type": "integer", "description": "Max changes to report (default 80, for rename)." }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let input = invocation.input.clone();
        let policy = self.policy.clone();
        let output = tokio::task::spawn_blocking(move || run_code_edit(&policy, &input))
            .await
            .map_err(|e| anyhow::anyhow!("code_edit task join error: {e}"))??;
        Ok(helpers::ok(invocation.id, output))
    }
}

fn run_code_edit(policy: &SecurityPolicy, input: &Value) -> Result<Value> {
    let action = helpers::required_string(input, "action")?.to_string();

    match action.as_str() {
        "replace" => run_replace(policy, input),
        "insert-before" => run_insert(policy, input, InsertPosition::Before),
        "insert-after" => run_insert(policy, input, InsertPosition::After),
        "rename" => run_rename(policy, input),
        other => bail!(
            "unknown code_edit action: `{other}`; expected replace, insert-before, insert-after, or rename"
        ),
    }
}

fn run_replace(policy: &SecurityPolicy, input: &Value) -> Result<Value> {
    let symbol = helpers::required_string(input, "symbol")?.to_string();
    let content = helpers::optional_string(input, "content")
        .or_else(|| helpers::optional_string(input, "replacement"))
        .context("replace action requires `content` or `replacement` string")?;
    let expected_hash = helpers::optional_string(input, "expected_hash");

    let target = resolve_symbol_target(policy, helpers::optional_string(input, "path"), &symbol)?;

    let edit = replace_symbol_definition(
        &target.path,
        &target.source,
        &symbol,
        &content,
        expected_hash.as_deref(),
    )?;

    fs::write(&target.path, &edit.content)
        .with_context(|| format!("failed to write {}", target.path.display()))?;

    let path = relative_path(policy, &target.path);
    let mut output = json!({
        "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
        "action": "replace",
        "path": path,
        "edits": edit.edits,
        "start_line": edit.start_line,
        "end_line": edit.end_line,
    });
    attach_code_edit_display_diff(&mut output, &path, &target.source, &edit.content);
    Ok(output)
}

fn run_insert(policy: &SecurityPolicy, input: &Value, position: InsertPosition) -> Result<Value> {
    let symbol = helpers::required_string(input, "symbol")?.to_string();
    let content = helpers::required_string(input, "content")?.to_string();
    let expected_hash = helpers::optional_string(input, "expected_hash");

    let target = resolve_symbol_target(policy, helpers::optional_string(input, "path"), &symbol)?;

    let edit = insert_around_symbol(
        &target.path,
        &target.source,
        &symbol,
        &content,
        position,
        expected_hash.as_deref(),
    )?;

    fs::write(&target.path, &edit.content)
        .with_context(|| format!("failed to write {}", target.path.display()))?;

    let action_label = match position {
        InsertPosition::Before => "insert-before",
        InsertPosition::After => "insert-after",
    };

    let path = relative_path(policy, &target.path);
    let mut output = json!({
        "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
        "action": action_label,
        "path": path,
        "edits": edit.edits,
        "start_line": edit.start_line,
        "end_line": edit.end_line,
    });
    attach_code_edit_display_diff(&mut output, &path, &target.source, &edit.content);
    Ok(output)
}

struct SymbolTarget {
    path: PathBuf,
    source: String,
}

fn resolve_symbol_target(
    policy: &SecurityPolicy,
    path: Option<String>,
    selector: &str,
) -> Result<SymbolTarget> {
    if let Some(path) = path.filter(|path| !path.is_empty()) {
        let resolved = resolve_scan_root(policy, &path, true)?;
        if !resolved.is_file() {
            bail!(
                "code_edit path must be a source file: {}",
                resolved.display()
            );
        }
        let source = fs::read_to_string(&resolved)
            .with_context(|| format!("failed to read {}", resolved.display()))?;
        return Ok(SymbolTarget {
            path: resolved,
            source,
        });
    }

    resolve_unique_symbol_target(policy, selector)
}

fn resolve_unique_symbol_target(policy: &SecurityPolicy, selector: &str) -> Result<SymbolTarget> {
    let root = policy.project_root().to_path_buf();
    let mut files = source_files(policy, &root, DEFAULT_MAX_FILES, true)?;
    files.sort();

    let mut matches = Vec::new();
    for file in files {
        let Some(source) = read_source_file(&file)? else {
            continue;
        };
        let symbols = match symbols_for_source(&file, &source) {
            Ok(symbols) => symbols,
            Err(_) => continue,
        };
        for symbol in symbols {
            if symbol_matches_selector(&symbol, selector) {
                matches.push((file.clone(), source.clone(), symbol));
            }
        }
    }

    match matches.len() {
        0 => bail!("symbol `{selector}` not found in project; pass `path` to code_edit"),
        1 => {
            let (path, source, _) = matches.pop().expect("one match");
            Ok(SymbolTarget { path, source })
        }
        _ => {
            let labels = matches
                .iter()
                .take(8)
                .map(|(path, _, symbol)| format!("{}:{}", relative_path(policy, path), symbol.name))
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "symbol `{selector}` is ambiguous across project; pass `path` to code_edit. matches: {labels}"
            )
        }
    }
}

fn symbol_matches_selector(symbol: &CodeSymbol, selector: &str) -> bool {
    symbol.id == selector || symbol.name == selector
}

fn run_rename(policy: &SecurityPolicy, input: &Value) -> Result<Value> {
    let path = helpers::required_string(input, "path")?.to_string();
    let old_name = helpers::required_string(input, "old_name")?.to_string();
    let new_name = helpers::required_string(input, "new_name")?.to_string();
    let dry_run = helpers::optional_bool(input, "dry_run").unwrap_or(false);
    let max_files = bounded_usize(input, "max_files", DEFAULT_MAX_FILES, MAX_FILES);
    let max_results = bounded_usize(input, "max_results", DEFAULT_MAX_RESULTS, MAX_RESULTS);

    let root = resolve_scan_root(policy, &path, true)?;
    let mut files = source_files(policy, &root, max_files.saturating_add(1), true)?;
    let mut truncated = files.len() > max_files;
    files.truncate(max_files);
    let mut planned = Vec::new();
    let mut files_scanned = 0usize;
    let mut total_edits = 0usize;

    for file in files {
        if total_edits >= max_results {
            truncated = true;
            break;
        }
        let Some(content) = read_source_file(&file)? else {
            continue;
        };
        files_scanned += 1;
        match rename_identifier(&file, &content, &old_name, &new_name) {
            Ok(edit) if edit.edits > 0 => {
                if total_edits.saturating_add(edit.edits) > max_results {
                    truncated = true;
                    break;
                }
                total_edits += edit.edits;
                planned.push((file, content, edit));
            }
            Ok(_) => {}
            Err(err) if err.to_string().contains("no identifier references") => {}
            Err(err) => return Err(err),
        }
    }

    if !dry_run && truncated {
        bail!(
            "rename matched more than max_results/max_files; rerun with a higher limit or dry_run=true before applying"
        );
    }

    if !dry_run {
        write_all_with_rollback(&planned)?;
    }

    let changes = planned
        .iter()
        .map(|(file, _, edit)| {
            json!({
                "path": relative_path(policy, file),
                "edits": edit.edits,
                "start_line": edit.start_line,
                "end_line": edit.end_line,
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
        "action": "rename",
        "path": relative_path(policy, &root),
        "old_name": old_name,
        "new_name": new_name,
        "dry_run": dry_run,
        "files_scanned": files_scanned,
        "files_changed": changes.len(),
        "total_edits": total_edits,
        "changes": changes,
        "truncated": truncated,
    }))
}

// ---------------------------------------------------------------------------
// Helper functions — copied from code.rs
// ---------------------------------------------------------------------------

/// Attach a numbered display diff (real file line numbers) for TUI rendering.
fn attach_code_edit_display_diff(output: &mut Value, path: &str, old: &str, new: &str) {
    let diff = super::write_tool::build_write_display_diff(path, Some(old), new);
    let (added, removed) = super::write_tool::count_diff_add_remove(&diff);
    let Value::Object(obj) = output else {
        return;
    };
    obj.insert("lines_added".into(), json!(added));
    obj.insert("lines_removed".into(), json!(removed));
    if !diff.is_empty() {
        obj.insert("diff".into(), Value::String(diff));
    }
}

fn write_all_with_rollback(planned: &[(PathBuf, String, SourceEdit)]) -> Result<()> {
    let mut written = Vec::new();
    for (file, original, edit) in planned {
        if let Err(err) = fs::write(file, &edit.content) {
            for (written_file, written_original) in written.into_iter().rev() {
                let _ = fs::write(written_file, written_original);
            }
            return Err(err).with_context(|| format!("failed to write {}", file.display()));
        }
        written.push((file.clone(), original.clone()));
    }
    Ok(())
}

fn resolve_scan_root(policy: &SecurityPolicy, path: &str, write: bool) -> Result<PathBuf> {
    let root = policy.resolve_project_path(Path::new(path));
    match policy.validate_path(&root, write) {
        SecurityDecision::Deny(reason) => bail!(reason),
        SecurityDecision::Allow | SecurityDecision::NeedsApproval(_) => Ok(root),
    }
}

fn source_files(
    policy: &SecurityPolicy,
    root: &Path,
    max_files: usize,
    write: bool,
) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_source_files(policy, root, max_files, write, &mut files)?;
    Ok(files)
}

fn collect_source_files(
    policy: &SecurityPolicy,
    path: &Path,
    max_files: usize,
    write: bool,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if files.len() >= max_files || should_skip(path) {
        return Ok(());
    }
    if path.is_file() {
        if is_supported_source(path)
            && file_is_small_enough(path)
            && path_allowed(policy, path, write)
        {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }
    if !path.is_dir() {
        return Ok(());
    }

    let mut entries = fs::read_dir(path)
        .with_context(|| format!("failed to list {}", path.display()))?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        if files.len() >= max_files {
            break;
        }
        collect_source_files(policy, &entry.path(), max_files, write, files)?;
    }
    Ok(())
}

fn path_allowed(policy: &SecurityPolicy, path: &Path, write: bool) -> bool {
    !matches!(policy.validate_path(path, write), SecurityDecision::Deny(_))
}

fn is_supported_source(path: &Path) -> bool {
    detect_language(path).is_some()
}

fn file_is_small_enough(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.len() <= MAX_FILE_BYTES)
        .unwrap_or(false)
}

fn read_source_file(path: &Path) -> Result<Option<String>> {
    if !file_is_small_enough(path) {
        return Ok(None);
    }
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(err) if err.kind() == std::io::ErrorKind::InvalidData => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn relative_path(policy: &SecurityPolicy, path: &Path) -> String {
    path.strip_prefix(policy.project_root())
        .unwrap_or(path)
        .display()
        .to_string()
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

fn bounded_usize(input: &Value, key: &str, default: usize, max: usize) -> usize {
    helpers::optional_u64(input, key)
        .unwrap_or(default as u64)
        .min(max as u64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolKind;
    use crate::{SecurityConfig, SecurityPolicy};

    fn policy(root: &Path) -> SecurityPolicy {
        SecurityPolicy::new(
            root.to_path_buf(),
            root.parent()
                .unwrap_or(root)
                .join("navi-test-data-code-edit"),
            SecurityConfig::default(),
        )
        .unwrap()
    }

    #[test]
    fn definition_has_correct_name() {
        let policy = SecurityPolicy::new(
            PathBuf::from("/tmp"),
            PathBuf::from("/tmp/navi-test-data-code-edit"),
            SecurityConfig::default(),
        )
        .unwrap();
        let tool = CodeEditTool::new(policy);
        let def = tool.definition();
        assert_eq!(def.name, "code_edit");
        assert_eq!(def.kind, ToolKind::Write);
    }

    #[test]
    fn definition_has_action_property() {
        let policy = SecurityPolicy::new(
            PathBuf::from("/tmp"),
            PathBuf::from("/tmp/navi-test-data-code-edit"),
            SecurityConfig::default(),
        )
        .unwrap();
        let tool = CodeEditTool::new(policy);
        let def = tool.definition();
        let schema = &def.input_schema;
        let props = schema["properties"].as_object().unwrap();
        assert!(
            props.contains_key("action"),
            "schema must have action property"
        );
        let action = &props["action"];
        assert_eq!(action["type"], "string");
        let variants = action["enum"].as_array().unwrap();
        let names: Vec<&str> = variants.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(
            names,
            vec!["replace", "insert-before", "insert-after", "rename"]
        );
        assert_eq!(schema["required"].as_array().unwrap(), &[json!("action")]);
    }

    #[test]
    fn replace_symbol_body_writes_valid_source() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("lib.rs");
        fs::write(&path, "pub fn target() -> i32 { 1 }\n").unwrap();
        let policy = policy(tempdir.path());

        // Use symbols_overview-like logic via run_code_edit by calling
        // replace_symbol_definition directly via the replace action.
        let source = fs::read_to_string(&path).unwrap();
        let symbols = navi_vfs::code::symbols_for_source(&path, &source).unwrap();
        let symbol = &symbols[0];

        let input = json!({
            "action": "replace",
            "path": "lib.rs",
            "symbol": symbol.id,
            "content": "pub fn target() -> i32 { 2 }",
            "expected_hash": symbol.hash,
        });
        run_code_edit(&policy, &input).unwrap();
        let result = fs::read_to_string(&path).unwrap();
        assert!(
            result.contains("{ 2 }"),
            "expected `{{ 2 }}` in result, got: {result:?}"
        );
    }

    #[test]
    fn insert_before_symbol_works() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("lib.rs");
        fs::write(&path, "pub fn target() -> i32 { 1 }\n").unwrap();
        let policy = policy(tempdir.path());

        let source = fs::read_to_string(&path).unwrap();
        let symbols = navi_vfs::code::symbols_for_source(&path, &source).unwrap();
        let symbol = &symbols[0];

        let input = json!({
            "action": "insert-before",
            "path": "lib.rs",
            "symbol": symbol.id,
            "content": "// preamble\n",
            "expected_hash": symbol.hash,
        });
        run_code_edit(&policy, &input).unwrap();
        let result = fs::read_to_string(&path).unwrap();
        assert!(
            result.starts_with("// preamble"),
            "expected preamble prefix, got: {result:?}"
        );
    }

    #[test]
    fn insert_after_symbol_works() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("lib.rs");
        fs::write(&path, "pub fn target() -> i32 { 1 }\n").unwrap();
        let policy = policy(tempdir.path());

        let source = fs::read_to_string(&path).unwrap();
        let symbols = navi_vfs::code::symbols_for_source(&path, &source).unwrap();
        let symbol = &symbols[0];

        let input = json!({
            "action": "insert-after",
            "path": "lib.rs",
            "symbol": symbol.id,
            "content": "\n// postamble\n",
            "expected_hash": symbol.hash,
        });
        run_code_edit(&policy, &input).unwrap();
        let result = fs::read_to_string(&path).unwrap();
        assert!(
            result.contains("// postamble"),
            "expected postamble in result, got: {result:?}"
        );
    }

    #[test]
    fn rename_symbol_dry_run_reports_changes_without_writing() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("lib.rs");
        fs::write(&path, "fn target(value: i32) -> i32 { value + 1 }\n").unwrap();

        let input = json!({
            "action": "rename",
            "path": "lib.rs",
            "old_name": "value",
            "new_name": "amount",
            "dry_run": true,
        });
        let output = run_code_edit(&policy(tempdir.path()), &input).unwrap();
        assert_eq!(output["total_edits"], 2);
        // File should be unchanged
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("value + 1"));
    }

    #[test]
    fn rename_symbol_does_not_write_partial_when_truncated() {
        let tempdir = tempfile::tempdir().unwrap();
        fs::write(
            tempdir.path().join("a.rs"),
            "fn a(value: i32) -> i32 { value + 1 }\n",
        )
        .unwrap();
        fs::write(
            tempdir.path().join("b.rs"),
            "fn b(value: i32) -> i32 { value + 2 }\n",
        )
        .unwrap();

        let input = json!({
            "action": "rename",
            "path": ".",
            "old_name": "value",
            "new_name": "amount",
            "max_results": 2,
        });
        let err = run_code_edit(&policy(tempdir.path()), &input).unwrap_err();

        assert!(err.to_string().contains("matched more than max_results"));
        assert!(
            fs::read_to_string(tempdir.path().join("a.rs"))
                .unwrap()
                .contains("value + 1")
        );
        assert!(
            fs::read_to_string(tempdir.path().join("b.rs"))
                .unwrap()
                .contains("value + 2")
        );
    }

    #[test]
    fn unknown_action_returns_error() {
        let policy = SecurityPolicy::new(
            PathBuf::from("/tmp"),
            PathBuf::from("/tmp/navi-test-data-code-edit"),
            SecurityConfig::default(),
        )
        .unwrap();
        let input = json!({ "action": "nope" });
        let err = run_code_edit(&policy, &input).unwrap_err();
        assert!(
            err.to_string().contains("unknown code_edit action"),
            "expected unknown action error, got: {err}"
        );
    }

    #[test]
    fn replace_resolves_unique_symbol_without_path() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("lib.rs");
        fs::write(&path, "pub fn target() -> i32 { 1 }\n").unwrap();

        let input = json!({
            "action": "replace",
            "symbol": "target",
            "content": "pub fn target() -> i32 { 2 }",
        });

        let output = run_code_edit(&policy(tempdir.path()), &input).unwrap();
        assert_eq!(output["path"], "lib.rs");
        let result = fs::read_to_string(path).unwrap();
        assert!(result.contains("{ 2 }"));
    }

    #[test]
    fn replace_without_path_reports_ambiguous_symbol() {
        let tempdir = tempfile::tempdir().unwrap();
        fs::write(
            tempdir.path().join("a.rs"),
            "pub fn target() -> i32 { 1 }\n",
        )
        .unwrap();
        fs::write(
            tempdir.path().join("b.rs"),
            "pub fn target() -> i32 { 2 }\n",
        )
        .unwrap();

        let input = json!({
            "action": "replace",
            "symbol": "target",
            "content": "pub fn target() -> i32 { 3 }",
        });

        let err = run_code_edit(&policy(tempdir.path()), &input).unwrap_err();
        assert!(
            err.to_string().contains("ambiguous across project"),
            "expected ambiguous symbol error, got: {err}"
        );
    }
}
