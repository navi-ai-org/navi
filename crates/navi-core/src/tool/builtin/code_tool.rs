use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use navi_vfs::code::{
    CodeReference, CodeSymbol, diagnostics_for_source, references_for_source, symbols_for_source,
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

pub(crate) struct CodeReadTool {
    policy: SecurityPolicy,
}

impl CodeReadTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }
}

#[async_trait]
impl Tool for CodeReadTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "code",
            "Unified code analysis tool with action-based dispatch. Actions: overview (symbol tree), find (find symbol), references (find refs), diagnostics (parse errors).",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["overview", "find", "references", "diagnostics"],
                        "description": "Operation: overview (symbol tree), find (find symbol), references (find refs), diagnostics (parse errors)."
                    },
                    "path": { "type": "string", "description": "Project-relative file or directory. Defaults to project root." },
                    "query": { "type": "string", "description": "Symbol name/signature to find (for action=find)." },
                    "name": { "type": "string", "description": "Alias for query (for action=find)." },
                    "kind": { "type": "string", "description": "Symbol kind filter: function, struct, enum, etc. (for action=find)." },
                    "include_body": { "type": "boolean", "description": "Include symbol source body (for action=find)." },
                    "max_results": { "type": "integer", "description": "Max results (default 80, max 500)." },
                    "max_files": { "type": "integer", "description": "Max source files (default 400, max 2000)." }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let action = helpers::required_string(&invocation.input, "action")?.to_string();
        let policy = self.policy.clone();
        let input = invocation.input.clone();
        let id = invocation.id.clone();

        let output = tokio::task::spawn_blocking(move || match action.as_str() {
            "overview" => {
                let scan = ScanInput::from_json(&input);
                run_symbols_overview(&policy, scan)
            }
            "find" => {
                let find_input = FindSymbolInput::from_json(&input)?;
                run_find_symbol(&policy, find_input)
            }
            "references" => {
                let ref_input = FindReferencesInput::from_json(&input)?;
                run_find_references(&policy, ref_input)
            }
            "diagnostics" => {
                let scan = ScanInput::from_json(&input);
                run_code_diagnostics(&policy, scan)
            }
            _ => Ok(helpers::tool_error(
                "unknown_action",
                format!("unknown code action: {action}"),
                true,
                Some("Use overview, find, references, or diagnostics."),
                None,
            )),
        })
        .await
        .map_err(|e| anyhow::anyhow!("code tool task join error: {e}"))??;

        Ok(helpers::ok(id, output))
    }
}

// ── Input structs ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug)]
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
            .context("missing required string `query` (or alias `name`) for action=find")?;
        Ok(Self {
            scan: ScanInput::from_json(input),
            query,
            kind: helpers::optional_string(input, "kind"),
            include_body: helpers::optional_bool(input, "include_body").unwrap_or(false),
        })
    }
}

#[derive(Clone, Debug)]
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

// ── Public run functions (usable from tests) ─────────────────────────────────

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

// ── Path resolution and file collection ──────────────────────────────────────

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

// ── JSON serialization helpers ───────────────────────────────────────────────

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

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(root: &Path) -> SecurityPolicy {
        SecurityPolicy::new(
            root.to_path_buf(),
            root.parent()
                .unwrap_or(root)
                .join("navi-test-data-code-tool"),
            crate::SecurityConfig::default(),
        )
        .unwrap()
    }

    // ── Definition ─────────────────────────────────────────────────────────

    #[test]
    fn definition_has_correct_name() {
        let tool = CodeReadTool::new(policy(Path::new("/tmp")));
        let def: ToolDefinition = tool.definition();
        assert_eq!(def.name, "code");
        assert!(def.description.contains("action-based"));
        assert!(matches!(def.kind, ToolKind::Read));
    }

    #[test]
    fn definition_has_action_property() {
        let tool = CodeReadTool::new(policy(Path::new("/tmp")));
        let def = tool.definition();
        let props = def.input_schema["properties"].as_object().unwrap();
        let action = &props["action"];
        assert_eq!(action["type"], "string");
        let variants = action["enum"].as_array().unwrap();
        let names: Vec<&str> = variants.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(names, vec!["overview", "find", "references", "diagnostics"]);
        assert!(
            def.input_schema["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "action")
        );
    }

    // ── Overview ───────────────────────────────────────────────────────────

    #[test]
    fn overview_returns_symbols_for_known_file() {
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

    // ── Find ───────────────────────────────────────────────────────────────

    #[test]
    fn find_symbol_by_name() {
        let tempdir = tempfile::tempdir().unwrap();
        fs::write(
            tempdir.path().join("lib.rs"),
            "fn hello() -> i32 { 1 }\nfn world() -> i32 { 2 }\n",
        )
        .unwrap();
        let input = FindSymbolInput::from_json(&json!({ "query": "world" })).unwrap();
        let output = run_find_symbol(&policy(tempdir.path()), input).unwrap();
        let matches = output["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["name"], "world");
    }

    #[test]
    fn find_symbol_requires_query_when_action_is_find() {
        let err = FindSymbolInput::from_json(&json!({})).unwrap_err();
        assert!(err.to_string().contains("query"));
    }

    #[test]
    fn find_symbol_filters_by_kind() {
        let tempdir = tempfile::tempdir().unwrap();
        fs::write(
            tempdir.path().join("lib.rs"),
            "fn hello() -> i32 { 1 }\nstruct Good { value: i32 }\n",
        )
        .unwrap();
        let input =
            FindSymbolInput::from_json(&json!({ "query": "good", "kind": "struct" })).unwrap();
        let output = run_find_symbol(&policy(tempdir.path()), input).unwrap();
        let matches = output["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["kind"], "struct");
    }

    #[test]
    fn find_symbol_with_include_body() {
        let tempdir = tempfile::tempdir().unwrap();
        fs::write(
            tempdir.path().join("lib.rs"),
            "fn detailed(x: i32) -> i32 { x + 1 }\n",
        )
        .unwrap();
        let input =
            FindSymbolInput::from_json(&json!({ "query": "detailed", "include_body": true }))
                .unwrap();
        let output = run_find_symbol(&policy(tempdir.path()), input).unwrap();
        let matches = output["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].get("body").is_some());
    }
}
