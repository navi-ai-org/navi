use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::helpers;
use crate::security::{SecurityDecision, SecurityPolicy};
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

const MAX_FILES: usize = 20;
const MAX_LINES_PER_FILE: usize = 500;
const MAX_TOTAL_BYTES: usize = 512 * 1024;
const MAX_CANDIDATES: usize = 2_000;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const SCORE_SAMPLE_BYTES: usize = 64 * 1024;

pub(crate) struct TopFilesTool {
    policy: SecurityPolicy,
}

impl TopFilesTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }
}

#[async_trait]
impl Tool for TopFilesTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "top_files",
            "Read the most relevant project files for guided exploration, with automatic ranking and truncation.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Project-relative directory to explore. Defaults to project root."
                    },
                    "query": {
                        "type": "string",
                        "description": "Task/topic to guide ranking. Matches paths, filenames, and early file content."
                    },
                    "max_files": {
                        "type": "integer",
                        "description": "Maximum files to return. Defaults to 8 and is capped at 20."
                    },
                    "max_lines_per_file": {
                        "type": "integer",
                        "description": "Maximum leading lines per file. Defaults to 400 and is capped at 500."
                    },
                    "max_total_bytes": {
                        "type": "integer",
                        "description": "Maximum combined content bytes before truncating. Defaults to 128KiB and is capped at 512KiB."
                    },
                    "hidden": {
                        "type": "boolean",
                        "description": "Include dotfiles and hidden directories. Defaults to false."
                    }
                },
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let input = TopFilesInput::from_json(&invocation.input);
        let policy = self.policy.clone();
        let root = policy.resolve_project_path(Path::new(&input.path));
        let query = input.query.clone();

        let output =
            tokio::task::spawn_blocking(move || run_top_files(&policy, root, &query, input))
                .await
                .map_err(|e| anyhow::anyhow!("top_files task join error: {e}"))??;

        Ok(helpers::ok(invocation.id, output))
    }
}

#[derive(Clone)]
struct TopFilesInput {
    path: String,
    query: Option<String>,
    max_files: usize,
    max_lines_per_file: usize,
    max_total_bytes: usize,
    hidden: bool,
}

impl TopFilesInput {
    fn from_json(input: &Value) -> Self {
        Self {
            path: helpers::optional_string(input, "path").unwrap_or_else(|| ".".to_string()),
            query: helpers::optional_string(input, "query"),
            max_files: helpers::optional_u64(input, "max_files")
                .unwrap_or(MAX_FILES as u64)
                .min(MAX_FILES as u64) as usize,
            max_lines_per_file: helpers::optional_u64(input, "max_lines_per_file")
                .unwrap_or(400)
                .min(MAX_LINES_PER_FILE as u64) as usize,
            max_total_bytes: helpers::optional_u64(input, "max_total_bytes")
                .unwrap_or(MAX_TOTAL_BYTES as u64)
                .min(MAX_TOTAL_BYTES as u64) as usize,
            hidden: helpers::optional_bool(input, "hidden").unwrap_or(false),
        }
    }
}

struct Candidate {
    path: PathBuf,
    rel_path: String,
    score: i64,
    reasons: BTreeSet<&'static str>,
    size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryIntent {
    CodeOverview,
    DocsOverview,
    TargetedSearch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileCategory {
    AgentInstructions,
    Readme,
    WorkspaceManifest,
    CrateManifest,
    Entrypoint,
    Source,
    Config,
    Other,
}

fn run_top_files(
    policy: &SecurityPolicy,
    root: PathBuf,
    query: &Option<String>,
    input: TopFilesInput,
) -> Result<Value> {
    if let SecurityDecision::Deny(reason) = policy.validate_path(&root, false) {
        anyhow::bail!(reason);
    }

    let query_terms = query_terms(query.as_deref());
    let mut candidates = Vec::new();
    let intent = classify_query_intent(&query_terms);
    collect_candidates(
        policy,
        &root,
        input.hidden,
        &query_terms,
        intent,
        &mut candidates,
    )?;
    let candidates_scanned = candidates.len();

    candidates.sort_by(compare_candidates);

    let mut files = Vec::new();
    let mut total_content_bytes = 0usize;
    let mut output_truncated = candidates_scanned > input.max_files;

    for candidate in candidates.into_iter().take(input.max_files) {
        if total_content_bytes >= input.max_total_bytes {
            output_truncated = true;
            break;
        }

        let Some(mut file) = read_candidate_file(&candidate, input.max_lines_per_file)? else {
            continue;
        };

        let content_len = file.content.len();
        let remaining = input.max_total_bytes.saturating_sub(total_content_bytes);
        if content_len > remaining {
            file.content = helpers::truncate_string(file.content, remaining);
            file.truncated = true;
            file.truncated_by_total_limit = true;
            output_truncated = true;
        }
        total_content_bytes += file.content.len();

        files.push(json!({
            "path": candidate.rel_path,
            "score": candidate.score,
            "reasons": candidate.reasons.into_iter().collect::<Vec<_>>(),
            "size": candidate.size,
            "content": file.content,
            "start_line": 1,
            "end_line": file.end_line,
            "total_lines": file.total_lines,
            "truncated": file.truncated,
            "truncated_by_total_limit": file.truncated_by_total_limit,
        }));
    }

    Ok(json!({
        "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
        "path": relative_path(policy.project_root(), &root),
        "query": query,
        "files": files,
        "candidates_scanned": candidates_scanned,
        "max_files": input.max_files,
        "max_lines_per_file": input.max_lines_per_file,
        "max_total_bytes": input.max_total_bytes,
        "truncated": output_truncated,
    }))
}

fn collect_candidates(
    policy: &SecurityPolicy,
    root: &Path,
    hidden: bool,
    query_terms: &[String],
    intent: QueryIntent,
    candidates: &mut Vec<Candidate>,
) -> Result<()> {
    if candidates.len() >= MAX_CANDIDATES || !root.exists() {
        return Ok(());
    }

    if root.is_file() {
        if let Some(candidate) = score_candidate(policy, root, query_terms, intent) {
            candidates.push(candidate);
        }
        return Ok(());
    }

    if !root.is_dir() {
        return Ok(());
    }

    let mut entries = fs::read_dir(root)?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        if candidates.len() >= MAX_CANDIDATES {
            break;
        }

        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !hidden && name.starts_with('.') {
            continue;
        }
        if should_skip_dir(&name) {
            continue;
        }

        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            collect_candidates(policy, &path, hidden, query_terms, intent, candidates)?;
        } else if file_type.is_file()
            && let Some(candidate) = score_candidate(policy, &path, query_terms, intent)
        {
            candidates.push(candidate);
        }
    }

    Ok(())
}

fn score_candidate(
    policy: &SecurityPolicy,
    path: &Path,
    query_terms: &[String],
    intent: QueryIntent,
) -> Option<Candidate> {
    if !matches!(policy.validate_path(path, false), SecurityDecision::Allow) {
        return None;
    }

    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES || should_skip_file(path) {
        return None;
    }

    let rel_path = relative_path(policy.project_root(), path);
    if !is_candidate_file(path, &rel_path) {
        return None;
    }

    let sample = read_text_prefix(path, SCORE_SAMPLE_BYTES)?;
    if sample.as_bytes().contains(&0) {
        return None;
    }

    let lower_path = rel_path.to_lowercase();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let lower_sample = sample.to_lowercase();
    let category = file_category(path, &rel_path, &file_name);
    let mut score = 0;
    let mut reasons = BTreeSet::new();

    if is_structural_file(&file_name) {
        score += 60;
        reasons.insert("structural_file");
    }
    match category {
        FileCategory::AgentInstructions => {
            score += 20;
            reasons.insert("agent_instructions");
        }
        FileCategory::Readme => {
            score += 15;
            reasons.insert("readme");
        }
        FileCategory::WorkspaceManifest => {
            score += 30;
            reasons.insert("workspace_manifest");
        }
        FileCategory::CrateManifest => {
            score += 22;
            reasons.insert("crate_manifest");
        }
        FileCategory::Entrypoint => {
            score += 18;
            reasons.insert("crate_entrypoint");
        }
        FileCategory::Source | FileCategory::Config | FileCategory::Other => {}
    }
    if is_entrypoint(&file_name) {
        score += 45;
        reasons.insert("entrypoint");
    }
    if is_source_file(path) {
        score += 25;
        reasons.insert("source_file");
    }
    if lower_path.contains("src/") || lower_path.starts_with("src") {
        score += 10;
        reasons.insert("source_directory");
    }
    if lower_path.contains("test") {
        score += 6;
        reasons.insert("test_path");
    }

    for term in query_terms {
        if lower_path.contains(term) {
            score += 35;
            reasons.insert("query_path_match");
        }
        if file_name.contains(term) {
            score += 25;
            reasons.insert("query_filename_match");
        }
        if lower_sample.contains(term) {
            score += 12;
            reasons.insert("query_content_match");
        }
    }

    apply_intent_adjustments(
        intent,
        category,
        &lower_path,
        &file_name,
        &mut score,
        &mut reasons,
    );

    if meta.len() > 256 * 1024 {
        score -= 30;
        reasons.insert("large_file_penalty");
    } else if meta.len() > 64 * 1024 {
        score -= 10;
        reasons.insert("medium_file_penalty");
    }

    if score <= 0 && !query_terms.is_empty() {
        return None;
    }

    Some(Candidate {
        path: path.to_path_buf(),
        rel_path,
        score,
        reasons,
        size: meta.len(),
    })
}

fn classify_query_intent(query_terms: &[String]) -> QueryIntent {
    if query_terms.is_empty() {
        return QueryIntent::CodeOverview;
    }
    let has_docs_term = query_terms.iter().any(|term| {
        matches!(
            term.as_str(),
            "agent"
                | "agents"
                | "instruction"
                | "instructions"
                | "guide"
                | "docs"
                | "doc"
                | "readme"
                | "rules"
        )
    });
    let has_code_overview_term = query_terms.iter().any(|term| {
        matches!(
            term.as_str(),
            "project"
                | "overview"
                | "structure"
                | "architecture"
                | "entrypoint"
                | "entrypoints"
                | "runtime"
                | "engine"
                | "harness"
                | "code"
                | "core"
        )
    });

    if has_docs_term && !has_code_overview_term {
        QueryIntent::DocsOverview
    } else if has_code_overview_term {
        QueryIntent::CodeOverview
    } else {
        QueryIntent::TargetedSearch
    }
}

fn file_category(path: &Path, rel_path: &str, file_name: &str) -> FileCategory {
    if matches!(file_name, "agents.md" | "claude.md") {
        return FileCategory::AgentInstructions;
    }
    if file_name == "readme.md" {
        return FileCategory::Readme;
    }
    if is_entrypoint(file_name) {
        return FileCategory::Entrypoint;
    }
    if is_workspace_manifest(rel_path, file_name) {
        return FileCategory::WorkspaceManifest;
    }
    if is_crate_manifest(rel_path, file_name) {
        return FileCategory::CrateManifest;
    }
    if is_source_file(path) {
        return FileCategory::Source;
    }
    if is_config_file(path) {
        return FileCategory::Config;
    }
    FileCategory::Other
}

fn apply_intent_adjustments(
    intent: QueryIntent,
    category: FileCategory,
    lower_path: &str,
    file_name: &str,
    score: &mut i64,
    reasons: &mut BTreeSet<&'static str>,
) {
    match intent {
        QueryIntent::CodeOverview => {
            reasons.insert("code_overview_boost");
            match category {
                FileCategory::WorkspaceManifest => *score += 55,
                FileCategory::CrateManifest => *score += 45,
                FileCategory::Entrypoint => *score += 65,
                FileCategory::Source => *score += 20,
                FileCategory::AgentInstructions | FileCategory::Readme => {
                    *score -= 80;
                    reasons.insert("doc_penalty");
                }
                FileCategory::Config | FileCategory::Other => {}
            }
            if is_domain_module(lower_path, file_name) {
                *score += 40;
                reasons.insert("domain_module");
            }
        }
        QueryIntent::DocsOverview => match category {
            FileCategory::AgentInstructions => {
                *score += 90;
                reasons.insert("docs_overview_boost");
            }
            FileCategory::Readme => {
                *score += 45;
                reasons.insert("docs_overview_boost");
            }
            _ => {}
        },
        QueryIntent::TargetedSearch => {}
    }
}

struct TopFileContent {
    content: String,
    end_line: usize,
    total_lines: usize,
    truncated: bool,
    truncated_by_total_limit: bool,
}

fn read_candidate_file(
    candidate: &Candidate,
    max_lines_per_file: usize,
) -> Result<Option<TopFileContent>> {
    let content = match fs::read_to_string(&candidate.path) {
        Ok(content) => content,
        Err(_) => return Ok(None),
    };
    let lines = content.lines().collect::<Vec<_>>();
    let total_lines = lines.len();
    let end_idx = max_lines_per_file.min(total_lines);
    let sliced_lines = &lines[..end_idx];
    let mut sliced_content = sliced_lines.join("\n");
    if !sliced_content.is_empty()
        && ((end_idx == total_lines && content.ends_with('\n')) || end_idx < total_lines)
    {
        sliced_content.push('\n');
    }

    Ok(Some(TopFileContent {
        content: sliced_content,
        end_line: end_idx,
        total_lines,
        truncated: end_idx < total_lines,
        truncated_by_total_limit: false,
    }))
}

fn read_text_prefix(path: &Path, max_bytes: usize) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(max_bytes as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    while !bytes.is_empty() && std::str::from_utf8(&bytes).is_err() {
        bytes.pop();
    }
    String::from_utf8(bytes).ok()
}

fn query_terms(query: Option<&str>) -> Vec<String> {
    query
        .unwrap_or_default()
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(str::trim)
        .filter(|term| term.len() > 1)
        .map(str::to_lowercase)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn compare_candidates(left: &Candidate, right: &Candidate) -> Ordering {
    right
        .score
        .cmp(&left.score)
        .then_with(|| left.rel_path.cmp(&right.rel_path))
}

fn relative_path(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn is_workspace_manifest(rel_path: &str, file_name: &str) -> bool {
    matches!(
        file_name,
        "cargo.toml" | "package.json" | "pyproject.toml" | "go.mod" | "pom.xml"
    ) && !rel_path.contains('/')
}

fn is_crate_manifest(rel_path: &str, file_name: &str) -> bool {
    matches!(
        file_name,
        "cargo.toml" | "package.json" | "pyproject.toml" | "go.mod" | "pom.xml"
    ) && rel_path.contains('/')
}

fn is_domain_module(lower_path: &str, file_name: &str) -> bool {
    let stem = file_name.split('.').next().unwrap_or_default();
    matches!(
        stem,
        "runtime"
            | "engine"
            | "harness"
            | "tool"
            | "tools"
            | "session"
            | "provider"
            | "providers"
            | "config"
            | "dispatch"
            | "event_loop"
            | "lib"
            | "main"
    ) || lower_path.contains("/runtime/")
        || lower_path.contains("/tool/")
        || lower_path.contains("/provider")
        || lower_path.contains("/session")
}

fn is_candidate_file(path: &Path, rel_path: &str) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_lowercase();
    is_structural_file(&file_name)
        || is_source_file(path)
        || is_config_file(path)
        || rel_path.ends_with("AGENTS.md")
        || rel_path.ends_with("CLAUDE.md")
}

fn is_source_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some(
            "rs" | "go"
                | "c"
                | "h"
                | "cpp"
                | "cc"
                | "hpp"
                | "js"
                | "jsx"
                | "ts"
                | "tsx"
                | "py"
                | "java"
                | "rb"
                | "php"
                | "sh"
                | "bash"
                | "html"
                | "css"
                | "json"
                | "toml"
                | "yaml"
                | "yml"
                | "md"
                | "cs"
        )
    )
}

fn is_config_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("toml" | "json" | "jsonc" | "yaml" | "yml" | "ini" | "conf")
    )
}

fn is_structural_file(file_name: &str) -> bool {
    matches!(
        file_name,
        "readme.md"
            | "agents.md"
            | "claude.md"
            | "cargo.toml"
            | "package.json"
            | "pyproject.toml"
            | "go.mod"
            | "pom.xml"
            | "build.gradle"
            | "settings.gradle"
            | "makefile"
            | "justfile"
            | "dockerfile"
    )
}

fn is_entrypoint(file_name: &str) -> bool {
    matches!(
        file_name,
        "main.rs"
            | "lib.rs"
            | "mod.rs"
            | "main.go"
            | "index.js"
            | "index.ts"
            | "app.js"
            | "app.ts"
            | "app.tsx"
            | "main.py"
            | "__init__.py"
            | "main.java"
    )
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
            | "coverage"
            | ".nyc_output"
            | "htmlcov"
            | ".idea"
            | ".vscode"
    )
}

fn should_skip_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_lowercase();
    if matches!(
        name.as_str(),
        "cargo.lock" | "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml" | "bun.lockb"
    ) {
        return true;
    }
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some(
            "lock"
                | "log"
                | "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "pdf"
                | "zip"
                | "gz"
                | "tar"
                | "wasm"
                | "so"
                | "dylib"
                | "dll"
                | "rlib"
                | "bin"
                | "snap"
        )
    ) || path
        .components()
        .any(|component| component.as_os_str() == "snapshots")
}
