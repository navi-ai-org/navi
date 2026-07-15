//! Repository exploration tool — deterministic BM25 + symbol search.
//!
//! Fast, non-LLM search over the project index. Combines:
//! - structured symbol ranking (`ranked_symbol_matches`)
//! - BM25 text matches over docs/signatures/snippets (`search_text_matches`)
//!
//! Returns compact locations (path + line range + snippet + score) so the
//! parent agent can `read_file` only what matters. No nested model turn.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

use super::helpers;
use crate::repo_intelligence::{
    RankedSymbolRecord, TextMatchRecord, build_index, ranked_symbol_matches, search_text_matches,
};
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

const DEFAULT_MAX_RESULTS: usize = 10;
const MAX_RESULTS_CAP: usize = 40;
/// Soft weight so top symbols and BM25 text compete on a shared ranking.
const SYMBOL_SCORE_SCALE: f64 = 1.0;
const TEXT_SCORE_SCALE: f64 = 2.5;

pub struct RepoExploreTool {
    project_dir: PathBuf,
}

impl RepoExploreTool {
    pub fn new(project_dir: PathBuf) -> Self {
        Self { project_dir }
    }
}

#[async_trait]
impl Tool for RepoExploreTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "repo_explore",
            "Fast contextual search over the repository (BM25 + symbol index). \
             Returns ranked file locations with line ranges and short snippets. \
             Use this before reading files to find the right places. \
             Does NOT spawn a subagent or call the model. \
             Prefer for: \"where is X\", architecture concepts, symbol names, error strings.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What to find: symbols, paths, concepts, error text, architectural terms."
                    },
                    "context": {
                        "type": "string",
                        "description": "Optional extra terms to bias ranking (why you need this / related words)."
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum locations to return (default 10, max 40)."
                    },
                    "kind": {
                        "type": "string",
                        "description": "Optional symbol kind filter (function, struct, trait, …). Applied to symbol hits only."
                    }
                },
                "required": ["query"],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let query = helpers::required_string(&invocation.input, "query")?.to_string();
        let context = helpers::optional_string(&invocation.input, "context");
        let kind = helpers::optional_string(&invocation.input, "kind");
        let max_results = helpers::optional_u64(&invocation.input, "max_results")
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .clamp(1, MAX_RESULTS_CAP);

        let search_query = match context.as_deref() {
            Some(ctx) if !ctx.trim().is_empty() => format!("{query} {ctx}"),
            _ => query.clone(),
        };

        let project_dir = self.project_dir.clone();
        let kind_filter = kind.clone();
        let started = Instant::now();

        // Index + search are CPU-bound; keep the runtime free.
        let result = tokio::task::spawn_blocking(move || {
            explore_repo(
                &project_dir,
                &search_query,
                kind_filter.as_deref(),
                max_results,
            )
        })
        .await;

        let elapsed_ms = started.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(report)) => Ok(helpers::ok(
                invocation.id,
                json!({
                    "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                    "query": query,
                    "context": context,
                    "locations": report.locations,
                    "files_indexed": report.files_indexed,
                    "symbols_considered": report.symbols_considered,
                    "text_hits": report.text_hits,
                    "elapsed_ms": elapsed_ms,
                    "engine": "bm25+symbols",
                }),
            )),
            Ok(Err(err)) => Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: json!({
                    "error": format!("repo_explore failed: {err:#}"),
                    "elapsed_ms": elapsed_ms,
                }),
            }),
            Err(err) => Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: json!({
                    "error": format!("repo_explore task join error: {err}"),
                    "elapsed_ms": elapsed_ms,
                }),
            }),
        }
    }
}

struct ExploreReport {
    locations: Vec<serde_json::Value>,
    files_indexed: usize,
    symbols_considered: usize,
    text_hits: usize,
}

#[derive(Debug, Clone)]
struct LocationHit {
    path: PathBuf,
    start_line: usize,
    end_line: usize,
    kind: String,
    name: Option<String>,
    snippet: String,
    score: f64,
    reasons: Vec<String>,
}

fn explore_repo(
    project_dir: &Path,
    query: &str,
    kind: Option<&str>,
    max_results: usize,
) -> Result<ExploreReport> {
    let index = build_index(project_dir)?;
    let symbols = ranked_symbol_matches(&index, query, kind);
    // Pull a wider BM25 pool so merge can re-rank against symbols.
    let text_pool = (max_results * 3).clamp(15, 80);
    let text_matches = search_text_matches(&index, query, text_pool);

    let locations = merge_locations(&symbols, &text_matches, max_results);

    Ok(ExploreReport {
        locations: locations
            .into_iter()
            .map(|hit| {
                json!({
                    "path": path_display(&hit.path),
                    "start_line": hit.start_line,
                    "end_line": hit.end_line,
                    "kind": hit.kind,
                    "name": hit.name,
                    "snippet": hit.snippet,
                    "score": hit.score,
                    "reasons": hit.reasons,
                    "why": why_summary(&hit),
                })
            })
            .collect(),
        files_indexed: index.files.len(),
        symbols_considered: symbols.len(),
        text_hits: text_matches.len(),
    })
}

fn merge_locations(
    symbols: &[RankedSymbolRecord],
    text_matches: &[TextMatchRecord],
    max_results: usize,
) -> Vec<LocationHit> {
    // Key: (path, start_line) — keep best score per anchor.
    let mut by_anchor: HashMap<(String, usize), LocationHit> = HashMap::new();

    for ranked in symbols {
        let path = ranked.symbol.path.clone();
        let line = ranked.symbol.line.max(1);
        let key = (path_display(&path), line);
        let mut reasons = ranked.reasons.clone();
        reasons.push("symbol".to_string());
        let hit = LocationHit {
            path,
            start_line: line,
            // Small window so the agent can read a compact range.
            end_line: line.saturating_add(12),
            kind: ranked.symbol.kind.clone(),
            name: Some(ranked.symbol.name.clone()),
            snippet: ranked.symbol.signature.clone(),
            score: ranked.score * SYMBOL_SCORE_SCALE,
            reasons,
        };
        insert_best(&mut by_anchor, key, hit);
    }

    for text in text_matches {
        let path = text.path.clone();
        let line = text.line.max(1);
        let key = (path_display(&path), line);
        let hit = LocationHit {
            path,
            start_line: line,
            end_line: line.saturating_add(8),
            kind: text.kind.clone(),
            name: None,
            snippet: text.text.clone(),
            score: text.score * TEXT_SCORE_SCALE,
            reasons: vec!["bm25".to_string(), text.kind.clone()],
        };
        insert_best(&mut by_anchor, key, hit);
    }

    let mut hits: Vec<LocationHit> = by_anchor.into_values().collect();
    hits.sort_by(|a, b| {
        score_cmp(b.score, a.score)
            .then_with(|| path_display(&a.path).cmp(&path_display(&b.path)))
            .then_with(|| a.start_line.cmp(&b.start_line))
    });
    hits.truncate(max_results);
    hits
}

fn insert_best(
    map: &mut HashMap<(String, usize), LocationHit>,
    key: (String, usize),
    hit: LocationHit,
) {
    match map.get(&key) {
        Some(existing) if existing.score >= hit.score => {
            // Keep existing; maybe merge reasons for transparency.
        }
        Some(existing) => {
            let mut merged = hit;
            for reason in &existing.reasons {
                if !merged.reasons.iter().any(|r| r == reason) {
                    merged.reasons.push(reason.clone());
                }
            }
            // Prefer a named symbol if the winner was text-only.
            if merged.name.is_none() {
                merged.name = existing.name.clone();
            }
            if merged.snippet.is_empty() {
                merged.snippet = existing.snippet.clone();
            }
            map.insert(key, merged);
        }
        None => {
            map.insert(key, hit);
        }
    }
}

fn score_cmp(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

fn path_display(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn why_summary(hit: &LocationHit) -> String {
    let mut parts = Vec::new();
    if let Some(name) = &hit.name {
        parts.push(format!("{kind} `{name}`", kind = hit.kind));
    } else {
        parts.push(hit.kind.clone());
    }
    if !hit.reasons.is_empty() {
        parts.push(format!("matched via {}", hit.reasons.join(", ")));
    }
    parts.join(" — ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_src(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn definition_has_correct_name_and_kind() {
        let tool = RepoExploreTool::new(PathBuf::from("/tmp"));
        let def = tool.definition();
        assert_eq!(def.name, "repo_explore");
        assert_eq!(def.kind, ToolKind::Read);
        assert!(def.description.to_lowercase().contains("bm25"));
        assert!(
            def.description.to_lowercase().contains("does not spawn"),
            "should clarify no nested agent turn"
        );
    }

    #[test]
    fn explore_finds_symbol_and_doc_hits() {
        let dir = tempfile::tempdir().unwrap();
        write_src(
            dir.path(),
            "src/lib.rs",
            "/// Handles tool approval for guarded commands.\n\
             pub fn validate_tool_approval() {}\n\
             fn other() { validate_tool_approval(); }\n",
        );
        write_src(
            dir.path(),
            "src/security.rs",
            "pub struct SecurityPolicy;\nimpl SecurityPolicy {\n  pub fn is_guarded_command() {}\n}\n",
        );

        let report = explore_repo(dir.path(), "tool approval guarded", None, 10).unwrap();
        assert!(
            report.files_indexed >= 2,
            "indexed: {}",
            report.files_indexed
        );
        assert!(!report.locations.is_empty(), "expected locations, got none");

        let blob = serde_json::to_string(&report.locations).unwrap();
        assert!(
            blob.contains("validate_tool_approval")
                || blob.contains("approval")
                || blob.contains("is_guarded_command")
                || blob.contains("SecurityPolicy"),
            "unexpected locations: {blob}"
        );
    }

    #[test]
    fn explore_respects_max_results() {
        let dir = tempfile::tempdir().unwrap();
        write_src(
            dir.path(),
            "src/a.rs",
            "pub fn alpha() {}\npub fn alphabet() {}\npub fn alpine() {}\n",
        );
        let report = explore_repo(dir.path(), "alp", None, 2).unwrap();
        assert!(report.locations.len() <= 2);
    }

    #[tokio::test]
    async fn invoke_returns_structured_locations() {
        let dir = tempfile::tempdir().unwrap();
        write_src(
            dir.path(),
            "src/main.rs",
            "fn main() { println!(\"hello repo explore\"); }\n",
        );
        let tool = RepoExploreTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "t1".into(),
                tool_name: "repo_explore".into(),
                input: json!({ "query": "repo explore hello" }),
            })
            .await
            .unwrap();
        assert!(result.ok, "{:?}", result.output);
        assert_eq!(
            result.output.get("engine").and_then(|v| v.as_str()),
            Some("bm25+symbols")
        );
        assert!(result.output.get("locations").is_some());
        assert!(result.output.get("elapsed_ms").is_some());
    }
}
