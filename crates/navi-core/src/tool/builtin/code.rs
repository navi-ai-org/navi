use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use navi_vfs::code::{
    CodeReference, CodeSymbol, InsertPosition, SourceEdit, diagnostics_for_source,
    insert_around_symbol, references_for_source, rename_identifier, replace_symbol_definition,
    symbols_for_source,
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
const MAX_BODY_BYTES: usize = 32 * 1024;

pub(crate) struct SymbolsOverviewTool {
    policy: SecurityPolicy,
}

pub(crate) struct FindSymbolTool {
    policy: SecurityPolicy,
}

pub(crate) struct FindReferencesTool {
    policy: SecurityPolicy,
}

pub(crate) struct CodeDiagnosticsTool {
    policy: SecurityPolicy,
}

pub(crate) struct ReplaceSymbolBodyTool {
    policy: SecurityPolicy,
}

pub(crate) struct InsertBeforeSymbolTool {
    policy: SecurityPolicy,
}

pub(crate) struct InsertAfterSymbolTool {
    policy: SecurityPolicy,
}

pub(crate) struct RenameSymbolTool {
    policy: SecurityPolicy,
}

impl SymbolsOverviewTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }
}

impl FindSymbolTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }
}

impl FindReferencesTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }
}

impl CodeDiagnosticsTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }
}

impl ReplaceSymbolBodyTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }
}

impl InsertBeforeSymbolTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }
}

impl InsertAfterSymbolTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }
}

impl RenameSymbolTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }
}

#[async_trait]
impl Tool for SymbolsOverviewTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "symbols_overview",
            "Return compact tree-sitter symbol metadata for a file or directory. Use before broad read_file calls when navigating or refactoring code.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Project-relative file or directory. Defaults to project root." },
                    "max_results": { "type": "integer", "description": "Maximum symbols to return. Defaults to 80 and is capped at 500." },
                    "max_files": { "type": "integer", "description": "Maximum source files to scan. Defaults to 400 and is capped at 2000." }
                },
                "additionalProperties": false
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let input = ScanInput::from_json(&invocation.input);
        let policy = self.policy.clone();
        let output = tokio::task::spawn_blocking(move || run_symbols_overview(&policy, input))
            .await
            .map_err(|e| anyhow::anyhow!("symbols_overview task join error: {e}"))??;
        Ok(helpers::ok(invocation.id, output))
    }
}

#[async_trait]
impl Tool for FindSymbolTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "find_symbol",
            "Find code symbols by name/signature in a file or directory. Returns ids and hashes for precise follow-up edits.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Symbol name or signature text to search for." },
                    "name": { "type": "string", "description": "Alias for query." },
                    "path": { "type": "string", "description": "Project-relative file or directory. Defaults to project root." },
                    "kind": { "type": "string", "description": "Optional symbol kind filter, e.g. function, class, struct, enum, trait." },
                    "include_body": { "type": "boolean", "description": "When true, include truncated symbol source body. Defaults to false." },
                    "max_results": { "type": "integer", "description": "Maximum matches. Defaults to 80 and is capped at 500." },
                    "max_files": { "type": "integer", "description": "Maximum source files to scan. Defaults to 400 and is capped at 2000." }
                },
                "additionalProperties": false
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let input = FindSymbolInput::from_json(&invocation.input)?;
        let policy = self.policy.clone();
        let output = tokio::task::spawn_blocking(move || run_find_symbol(&policy, input))
            .await
            .map_err(|e| anyhow::anyhow!("find_symbol task join error: {e}"))??;
        Ok(helpers::ok(invocation.id, output))
    }
}

#[async_trait]
impl Tool for FindReferencesTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "find_references",
            "Find exact identifier references in source files using tree-sitter tokens, ignoring comments/strings where the grammar exposes them.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Identifier to find exactly." },
                    "path": { "type": "string", "description": "Project-relative file or directory. Defaults to project root." },
                    "max_results": { "type": "integer", "description": "Maximum references. Defaults to 80 and is capped at 500." },
                    "max_files": { "type": "integer", "description": "Maximum source files to scan. Defaults to 400 and is capped at 2000." }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let input = FindReferencesInput::from_json(&invocation.input)?;
        let policy = self.policy.clone();
        let output = tokio::task::spawn_blocking(move || run_find_references(&policy, input))
            .await
            .map_err(|e| anyhow::anyhow!("find_references task join error: {e}"))??;
        Ok(helpers::ok(invocation.id, output))
    }
}

#[async_trait]
impl Tool for CodeDiagnosticsTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "code_diagnostics",
            "Return tree-sitter parse diagnostics for a source file or directory. Useful before and after structural edits.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Project-relative file or directory. Defaults to project root." },
                    "max_files": { "type": "integer", "description": "Maximum source files to scan. Defaults to 400 and is capped at 2000." },
                    "max_results": { "type": "integer", "description": "Maximum diagnostics. Defaults to 80 and is capped at 500." }
                },
                "additionalProperties": false
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let input = ScanInput::from_json(&invocation.input);
        let policy = self.policy.clone();
        let output = tokio::task::spawn_blocking(move || run_code_diagnostics(&policy, input))
            .await
            .map_err(|e| anyhow::anyhow!("code_diagnostics task join error: {e}"))??;
        Ok(helpers::ok(invocation.id, output))
    }
}

#[async_trait]
impl Tool for ReplaceSymbolBodyTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "replace_symbol_body",
            "Replace one full symbol definition/body by symbol id or unique name. Use expected_hash from symbols_overview/find_symbol to avoid stale edits.",
            ToolKind::Write,
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Project-relative source file to edit." },
                    "symbol": { "type": "string", "description": "Symbol id returned by symbols_overview/find_symbol, or a unique symbol name." },
                    "replacement": { "type": "string", "description": "Full replacement source for the symbol definition/body." },
                    "expected_hash": { "type": "string", "description": "Optional current symbol hash to reject stale edits." }
                },
                "required": ["path", "symbol", "replacement"],
                "additionalProperties": false
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let input = SymbolEditInput::from_json(&invocation.input, "replacement")?;
        let policy = self.policy.clone();
        let output = tokio::task::spawn_blocking(move || {
            run_symbol_edit(&policy, input, SymbolEditKind::Replace)
        })
        .await
        .map_err(|e| anyhow::anyhow!("replace_symbol_body task join error: {e}"))??;
        Ok(helpers::ok(invocation.id, output))
    }
}

#[async_trait]
impl Tool for InsertBeforeSymbolTool {
    fn definition(&self) -> ToolDefinition {
        insert_tool_definition(
            "insert_before_symbol",
            "Insert source text immediately before a symbol id or unique name. Use expected_hash to avoid stale edits.",
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let input = SymbolEditInput::from_json(&invocation.input, "content")?;
        let policy = self.policy.clone();
        let output = tokio::task::spawn_blocking(move || {
            run_symbol_edit(&policy, input, SymbolEditKind::InsertBefore)
        })
        .await
        .map_err(|e| anyhow::anyhow!("insert_before_symbol task join error: {e}"))??;
        Ok(helpers::ok(invocation.id, output))
    }
}

#[async_trait]
impl Tool for InsertAfterSymbolTool {
    fn definition(&self) -> ToolDefinition {
        insert_tool_definition(
            "insert_after_symbol",
            "Insert source text immediately after a symbol id or unique name. Use expected_hash to avoid stale edits.",
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let input = SymbolEditInput::from_json(&invocation.input, "content")?;
        let policy = self.policy.clone();
        let output = tokio::task::spawn_blocking(move || {
            run_symbol_edit(&policy, input, SymbolEditKind::InsertAfter)
        })
        .await
        .map_err(|e| anyhow::anyhow!("insert_after_symbol task join error: {e}"))??;
        Ok(helpers::ok(invocation.id, output))
    }
}

#[async_trait]
impl Tool for RenameSymbolTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "rename_symbol",
            "Rename exact identifier tokens in one source file or across a directory using tree-sitter. Prefer find_references first for review; this is token-aware, not compiler/LSP semantic rename.",
            ToolKind::Write,
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Project-relative source file or directory to edit." },
                    "old_name": { "type": "string", "description": "Identifier to rename exactly." },
                    "new_name": { "type": "string", "description": "Replacement identifier." },
                    "dry_run": { "type": "boolean", "description": "When true, return planned changes without writing. Defaults to false." },
                    "max_files": { "type": "integer", "description": "Maximum source files to scan. Defaults to 400 and is capped at 2000." },
                    "max_results": { "type": "integer", "description": "Maximum changed files to report. Defaults to 80 and is capped at 500." }
                },
                "required": ["path", "old_name", "new_name"],
                "additionalProperties": false
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let input = RenameInput::from_json(&invocation.input)?;
        let policy = self.policy.clone();
        let output = tokio::task::spawn_blocking(move || run_rename_symbol(&policy, input))
            .await
            .map_err(|e| anyhow::anyhow!("rename_symbol task join error: {e}"))??;
        Ok(helpers::ok(invocation.id, output))
    }
}

fn insert_tool_definition(name: &str, description: &str) -> ToolDefinition {
    helpers::definition(
        name,
        description,
        ToolKind::Write,
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Project-relative source file to edit." },
                "symbol": { "type": "string", "description": "Symbol id returned by symbols_overview/find_symbol, or a unique symbol name." },
                "content": { "type": "string", "description": "Source text to insert." },
                "expected_hash": { "type": "string", "description": "Optional current symbol hash to reject stale edits." }
            },
            "required": ["path", "symbol", "content"],
            "additionalProperties": false
        }),
    )
}

#[derive(Clone)]
struct ScanInput {
    path: String,
    max_results: usize,
    max_files: usize,
}

impl ScanInput {
    fn from_json(input: &Value) -> Self {
        Self {
            path: helpers::optional_string(input, "path").unwrap_or_else(|| ".".to_string()),
            max_results: bounded_usize(input, "max_results", DEFAULT_MAX_RESULTS, MAX_RESULTS),
            max_files: bounded_usize(input, "max_files", DEFAULT_MAX_FILES, MAX_FILES),
        }
    }
}

#[derive(Clone)]
struct FindSymbolInput {
    scan: ScanInput,
    query: String,
    kind: Option<String>,
    include_body: bool,
}

impl FindSymbolInput {
    fn from_json(input: &Value) -> Result<Self> {
        let query = helpers::optional_string(input, "query")
            .or_else(|| helpers::optional_string(input, "name"))
            .filter(|value| !value.trim().is_empty())
            .context("missing required string `query` (or alias `name`)")?;
        Ok(Self {
            scan: ScanInput::from_json(input),
            query,
            kind: helpers::optional_string(input, "kind"),
            include_body: helpers::optional_bool(input, "include_body").unwrap_or(false),
        })
    }
}

#[derive(Clone)]
struct FindReferencesInput {
    scan: ScanInput,
    name: String,
}

impl FindReferencesInput {
    fn from_json(input: &Value) -> Result<Self> {
        Ok(Self {
            scan: ScanInput::from_json(input),
            name: helpers::required_string(input, "name")?.to_string(),
        })
    }
}

#[derive(Clone)]
struct SymbolEditInput {
    path: String,
    symbol: String,
    content: String,
    expected_hash: Option<String>,
}

impl SymbolEditInput {
    fn from_json(input: &Value, content_key: &str) -> Result<Self> {
        Ok(Self {
            path: helpers::required_string(input, "path")?.to_string(),
            symbol: helpers::required_string(input, "symbol")?.to_string(),
            content: helpers::required_string(input, content_key)?.to_string(),
            expected_hash: helpers::optional_string(input, "expected_hash"),
        })
    }
}

#[derive(Clone)]
struct RenameInput {
    path: String,
    old_name: String,
    new_name: String,
    dry_run: bool,
    max_files: usize,
    max_results: usize,
}

impl RenameInput {
    fn from_json(input: &Value) -> Result<Self> {
        Ok(Self {
            path: helpers::required_string(input, "path")?.to_string(),
            old_name: helpers::required_string(input, "old_name")?.to_string(),
            new_name: helpers::required_string(input, "new_name")?.to_string(),
            dry_run: helpers::optional_bool(input, "dry_run").unwrap_or(false),
            max_files: bounded_usize(input, "max_files", DEFAULT_MAX_FILES, MAX_FILES),
            max_results: bounded_usize(input, "max_results", DEFAULT_MAX_RESULTS, MAX_RESULTS),
        })
    }
}

enum SymbolEditKind {
    Replace,
    InsertBefore,
    InsertAfter,
}

fn run_symbols_overview(policy: &SecurityPolicy, input: ScanInput) -> Result<Value> {
    let root = resolve_scan_root(policy, &input.path, false)?;
    let files = source_files(policy, &root, input.max_files, false)?;
    let mut symbols = Vec::new();
    let mut files_scanned = 0usize;
    let mut truncated = files.len() >= input.max_files;

    for file in files {
        if symbols.len() >= input.max_results {
            truncated = true;
            break;
        }
        let Some(content) = read_source_file(&file)? else {
            continue;
        };
        files_scanned += 1;
        for symbol in symbols_for_source(&file, &content).unwrap_or_default() {
            if symbols.len() >= input.max_results {
                truncated = true;
                break;
            }
            symbols.push(symbol_json(policy, &file, &symbol));
        }
    }

    Ok(json!({
        "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
        "path": relative_path(policy, &root),
        "symbols": symbols,
        "files_scanned": files_scanned,
        "max_results": input.max_results,
        "truncated": truncated,
    }))
}

fn run_find_symbol(policy: &SecurityPolicy, input: FindSymbolInput) -> Result<Value> {
    let root = resolve_scan_root(policy, &input.scan.path, false)?;
    let files = source_files(policy, &root, input.scan.max_files, false)?;
    let needle = input.query.to_lowercase();
    let kind_filter = input.kind.as_ref().map(|kind| kind.to_lowercase());
    let mut matches = Vec::new();
    let mut files_scanned = 0usize;
    let mut truncated = files.len() >= input.scan.max_files;

    for file in files {
        if matches.len() >= input.scan.max_results {
            truncated = true;
            break;
        }
        let Some(content) = read_source_file(&file)? else {
            continue;
        };
        files_scanned += 1;
        for symbol in symbols_for_source(&file, &content).unwrap_or_default() {
            if matches.len() >= input.scan.max_results {
                truncated = true;
                break;
            }
            if let Some(kind) = &kind_filter
                && symbol.kind.to_lowercase() != *kind
            {
                continue;
            }
            let haystack =
                format!("{} {} {}", symbol.name, symbol.kind, symbol.signature).to_lowercase();
            if !haystack.contains(&needle) {
                continue;
            }
            let mut value = symbol_json(policy, &file, &symbol);
            if input.include_body
                && let Some(body) = content.get(symbol.start_byte..symbol.end_byte)
                && let Value::Object(ref mut object) = value
            {
                object.insert(
                    "body".to_string(),
                    json!(helpers::truncate_string(body.to_string(), MAX_BODY_BYTES)),
                );
            }
            matches.push(value);
        }
    }

    Ok(json!({
        "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
        "query": input.query,
        "path": relative_path(policy, &root),
        "matches": matches,
        "files_scanned": files_scanned,
        "truncated": truncated,
    }))
}

fn run_find_references(policy: &SecurityPolicy, input: FindReferencesInput) -> Result<Value> {
    let root = resolve_scan_root(policy, &input.scan.path, false)?;
    let files = source_files(policy, &root, input.scan.max_files, false)?;
    let mut references = Vec::new();
    let mut files_scanned = 0usize;
    let mut truncated = files.len() >= input.scan.max_files;

    for file in files {
        if references.len() >= input.scan.max_results {
            truncated = true;
            break;
        }
        let Some(content) = read_source_file(&file)? else {
            continue;
        };
        files_scanned += 1;
        for reference in references_for_source(&file, &content, &input.name).unwrap_or_default() {
            if references.len() >= input.scan.max_results {
                truncated = true;
                break;
            }
            references.push(reference_json(policy, &file, &reference));
        }
    }

    Ok(json!({
        "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
        "name": input.name,
        "path": relative_path(policy, &root),
        "references": references,
        "files_scanned": files_scanned,
        "truncated": truncated,
    }))
}

fn run_code_diagnostics(policy: &SecurityPolicy, input: ScanInput) -> Result<Value> {
    let root = resolve_scan_root(policy, &input.path, false)?;
    let files = source_files(policy, &root, input.max_files, false)?;
    let mut diagnostics = Vec::new();
    let mut files_scanned = 0usize;
    let mut truncated = files.len() >= input.max_files;

    for file in files {
        if diagnostics.len() >= input.max_results {
            truncated = true;
            break;
        }
        let Some(content) = read_source_file(&file)? else {
            continue;
        };
        files_scanned += 1;
        for diagnostic in diagnostics_for_source(&file, &content).unwrap_or_default() {
            if diagnostics.len() >= input.max_results {
                truncated = true;
                break;
            }
            diagnostics.push(json!({
                "path": relative_path(policy, &file),
                "kind": diagnostic.kind,
                "message": diagnostic.message,
                "start_line": diagnostic.start_line,
                "start_column": diagnostic.start_column,
                "end_line": diagnostic.end_line,
                "end_column": diagnostic.end_column,
            }));
        }
    }

    Ok(json!({
        "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
        "path": relative_path(policy, &root),
        "diagnostics": diagnostics,
        "files_scanned": files_scanned,
        "truncated": truncated,
    }))
}

fn run_symbol_edit(
    policy: &SecurityPolicy,
    input: SymbolEditInput,
    kind: SymbolEditKind,
) -> Result<Value> {
    let path = resolve_scan_root(policy, &input.path, true)?;
    if !path.is_file() {
        bail!("symbol edit path must be a source file: {}", path.display());
    }
    let content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let expected_hash = input.expected_hash.as_deref();
    let edit = match kind {
        SymbolEditKind::Replace => replace_symbol_definition(
            &path,
            &content,
            &input.symbol,
            &input.content,
            expected_hash,
        )?,
        SymbolEditKind::InsertBefore => insert_around_symbol(
            &path,
            &content,
            &input.symbol,
            &input.content,
            InsertPosition::Before,
            expected_hash,
        )?,
        SymbolEditKind::InsertAfter => insert_around_symbol(
            &path,
            &content,
            &input.symbol,
            &input.content,
            InsertPosition::After,
            expected_hash,
        )?,
    };
    fs::write(&path, &edit.content)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(json!({
        "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
        "path": relative_path(policy, &path),
        "edits": edit.edits,
        "start_line": edit.start_line,
        "end_line": edit.end_line,
    }))
}

fn run_rename_symbol(policy: &SecurityPolicy, input: RenameInput) -> Result<Value> {
    let root = resolve_scan_root(policy, &input.path, true)?;
    let mut files = source_files(policy, &root, input.max_files.saturating_add(1), true)?;
    let mut truncated = files.len() > input.max_files;
    files.truncate(input.max_files);
    let mut planned = Vec::new();
    let mut files_scanned = 0usize;
    let mut total_edits = 0usize;

    for file in files {
        if total_edits >= input.max_results {
            truncated = true;
            break;
        }
        let Some(content) = read_source_file(&file)? else {
            continue;
        };
        files_scanned += 1;
        match rename_identifier(&file, &content, &input.old_name, &input.new_name) {
            Ok(edit) if edit.edits > 0 => {
                if total_edits.saturating_add(edit.edits) > input.max_results {
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

    if !input.dry_run && truncated {
        bail!(
            "rename_symbol matched more than max_results/max_files; rerun with a higher limit or dry_run=true before applying"
        );
    }

    if !input.dry_run {
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
        "path": relative_path(policy, &root),
        "old_name": input.old_name,
        "new_name": input.new_name,
        "dry_run": input.dry_run,
        "files_scanned": files_scanned,
        "files_changed": changes.len(),
        "total_edits": total_edits,
        "changes": changes,
        "truncated": truncated,
    }))
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

fn symbol_json(policy: &SecurityPolicy, file: &Path, symbol: &CodeSymbol) -> Value {
    json!({
        "path": relative_path(policy, file),
        "id": symbol.id,
        "name": symbol.name,
        "kind": symbol.kind,
        "language": symbol.language,
        "start_line": symbol.start_line,
        "end_line": symbol.end_line,
        "start_column": symbol.start_column,
        "end_column": symbol.end_column,
        "start_byte": symbol.start_byte,
        "end_byte": symbol.end_byte,
        "name_start_byte": symbol.name_start_byte,
        "name_end_byte": symbol.name_end_byte,
        "parent_id": symbol.parent_id,
        "signature": symbol.signature,
        "hash": symbol.hash,
    })
}

fn reference_json(policy: &SecurityPolicy, file: &Path, reference: &CodeReference) -> Value {
    json!({
        "path": relative_path(policy, file),
        "name": reference.name,
        "kind": reference.kind,
        "line": reference.line,
        "column": reference.column,
        "start_byte": reference.start_byte,
        "end_byte": reference.end_byte,
        "snippet": reference.snippet,
    })
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

    fn policy(root: &Path) -> SecurityPolicy {
        SecurityPolicy::new(
            root.to_path_buf(),
            root.join(".navi-data"),
            crate::SecurityConfig::default(),
        )
        .unwrap()
    }

    #[test]
    fn symbols_overview_returns_hash_guarded_symbols() {
        let tempdir = tempfile::tempdir().unwrap();
        let src = tempdir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("lib.rs"), "pub fn target() -> i32 { 1 }\n").unwrap();

        let output =
            run_symbols_overview(&policy(tempdir.path()), ScanInput::from_json(&json!({})))
                .unwrap();
        let symbols = output["symbols"].as_array().unwrap();
        assert_eq!(symbols[0]["name"], "target");
        assert!(symbols[0]["hash"].as_str().unwrap().len() >= 8);
    }

    #[test]
    fn replace_symbol_body_writes_valid_source() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("lib.rs");
        fs::write(&path, "pub fn target() -> i32 { 1 }\n").unwrap();
        let policy = policy(tempdir.path());
        let overview =
            run_symbols_overview(&policy, ScanInput::from_json(&json!({ "path": "lib.rs" })))
                .unwrap();
        let symbol = &overview["symbols"].as_array().unwrap()[0];
        run_symbol_edit(
            &policy,
            SymbolEditInput {
                path: "lib.rs".to_string(),
                symbol: symbol["id"].as_str().unwrap().to_string(),
                content: "pub fn target() -> i32 { 2 }".to_string(),
                expected_hash: Some(symbol["hash"].as_str().unwrap().to_string()),
            },
            SymbolEditKind::Replace,
        )
        .unwrap();
        assert!(fs::read_to_string(path).unwrap().contains("{ 2 }"));
    }

    #[test]
    fn rename_symbol_dry_run_reports_changes_without_writing() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("lib.rs");
        fs::write(&path, "fn target(value: i32) -> i32 { value + 1 }\n").unwrap();
        let output = run_rename_symbol(
            &policy(tempdir.path()),
            RenameInput::from_json(&json!({
                "path": "lib.rs",
                "old_name": "value",
                "new_name": "amount",
                "dry_run": true
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(output["total_edits"], 2);
        assert!(fs::read_to_string(path).unwrap().contains("value + 1"));
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

        let err = run_rename_symbol(
            &policy(tempdir.path()),
            RenameInput::from_json(&json!({
                "path": ".",
                "old_name": "value",
                "new_name": "amount",
                "max_results": 2
            }))
            .unwrap(),
        )
        .unwrap_err();

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
}
