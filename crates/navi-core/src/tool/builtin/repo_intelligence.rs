use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};

use super::helpers;
use crate::repo_intelligence::{
    build_index, churn_from_git_log, dependency_edges, discover_tests, goto_symbol,
    ranked_symbol_matches, references, search_text_matches,
};
use crate::security::SecurityPolicy;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

#[derive(Clone, Copy)]
pub(crate) enum RepoIntelligenceAction {
    AstSearch,
    SymbolGoto,
    SymbolReferences,
    DependencyGraph,
    TestDiscovery,
    OwnershipChurn,
}

pub(crate) struct RepoIntelligenceTool {
    policy: SecurityPolicy,
    action: RepoIntelligenceAction,
}

impl RepoIntelligenceTool {
    pub(crate) fn new(policy: SecurityPolicy, action: RepoIntelligenceAction) -> Self {
        Self { policy, action }
    }

    fn name(&self) -> &'static str {
        match self.action {
            RepoIntelligenceAction::AstSearch => "ast_search",
            RepoIntelligenceAction::SymbolGoto => "symbol_goto",
            RepoIntelligenceAction::SymbolReferences => "symbol_references",
            RepoIntelligenceAction::DependencyGraph => "dependency_graph_query",
            RepoIntelligenceAction::TestDiscovery => "test_discovery",
            RepoIntelligenceAction::OwnershipChurn => "ownership_churn_query",
        }
    }
}

#[async_trait]
impl Tool for RepoIntelligenceTool {
    fn definition(&self) -> ToolDefinition {
        match self.action {
            RepoIntelligenceAction::AstSearch => helpers::definition(
                self.name(),
                "Search repository symbols using the structured repo index before falling back to text grep.",
                ToolKind::Read,
                json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "kind": { "type": "string" },
                        "max_results": { "type": "integer" }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }),
            ),
            RepoIntelligenceAction::SymbolGoto => helpers::definition(
                self.name(),
                "Resolve a symbol name to its defining file and line.",
                ToolKind::Read,
                json!({
                    "type": "object",
                    "properties": { "name": { "type": "string" } },
                    "required": ["name"],
                    "additionalProperties": false
                }),
            ),
            RepoIntelligenceAction::SymbolReferences => helpers::definition(
                self.name(),
                "Find identifier references using the structured repo index.",
                ToolKind::Read,
                json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "max_results": { "type": "integer" }
                    },
                    "required": ["name"],
                    "additionalProperties": false
                }),
            ),
            RepoIntelligenceAction::DependencyGraph => helpers::definition(
                self.name(),
                "Query compact import/dependency edges discovered from source files.",
                ToolKind::Read,
                json!({
                    "type": "object",
                    "properties": { "max_results": { "type": "integer" } },
                    "additionalProperties": false
                }),
            ),
            RepoIntelligenceAction::TestDiscovery => helpers::definition(
                self.name(),
                "Suggest the smallest verifier/test command for touched paths.",
                ToolKind::Read,
                json!({
                    "type": "object",
                    "properties": {
                        "paths": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    },
                    "additionalProperties": false
                }),
            ),
            RepoIntelligenceAction::OwnershipChurn => helpers::definition(
                self.name(),
                "Return files with the highest recent git churn for risk-aware planning.",
                ToolKind::Read,
                json!({
                    "type": "object",
                    "properties": { "max_results": { "type": "integer" } },
                    "additionalProperties": false
                }),
            ),
        }
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let policy = self.policy.clone();
        let action = self.action;
        let input = invocation.input.clone();
        let output = tokio::task::spawn_blocking(move || run_action(&policy, action, &input))
            .await
            .map_err(|err| anyhow::anyhow!("repo intelligence task join error: {err}"))??;
        Ok(helpers::ok(invocation.id, output))
    }
}

fn run_action(
    policy: &SecurityPolicy,
    action: RepoIntelligenceAction,
    input: &Value,
) -> Result<Value> {
    let root = policy.project_root();
    match action {
        RepoIntelligenceAction::AstSearch => {
            let query = helpers::required_string(input, "query")?;
            let kind = helpers::optional_string(input, "kind");
            let max_results = bounded(input, "max_results", 80, 500);
            let index = build_index(root)?;
            let ranked = ranked_symbol_matches(&index, query, kind.as_deref());
            let matches = ranked
                .iter()
                .take(max_results)
                .map(|ranked| ranked.symbol.clone())
                .collect::<Vec<_>>();
            let ranking = ranked
                .iter()
                .take(max_results)
                .map(|ranked| {
                    json!({
                        "name": ranked.symbol.name.clone(),
                        "kind": ranked.symbol.kind.clone(),
                        "path": ranked.symbol.path.clone(),
                        "line": ranked.symbol.line,
                        "score": ranked.score,
                        "reasons": ranked.reasons.clone(),
                    })
                })
                .collect::<Vec<_>>();
            let text_matches = search_text_matches(&index, query, max_results.clamp(5, 40));
            Ok(json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "query": query,
                "matches": matches,
                "text_matches": text_matches,
                "ranking": ranking,
                "files_indexed": index.files.len(),
            }))
        }
        RepoIntelligenceAction::SymbolGoto => {
            let name = helpers::required_string(input, "name")?;
            let index = build_index(root)?;
            Ok(json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "name": name,
                "symbol": goto_symbol(&index, name),
            }))
        }
        RepoIntelligenceAction::SymbolReferences => {
            let name = helpers::required_string(input, "name")?;
            let max_results = bounded(input, "max_results", 80, 500);
            let index = build_index(root)?;
            let mut refs = references(&index, name);
            refs.truncate(max_results);
            Ok(json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "name": name,
                "references": refs,
            }))
        }
        RepoIntelligenceAction::DependencyGraph => {
            let max_results = bounded(input, "max_results", 120, 1000);
            let index = build_index(root)?;
            let mut edges = dependency_edges(&index);
            edges.truncate(max_results);
            Ok(json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "edges": edges,
                "files_indexed": index.files.len(),
            }))
        }
        RepoIntelligenceAction::TestDiscovery => {
            let paths = input
                .get("paths")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(std::path::PathBuf::from)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Ok(json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "tests": discover_tests(root, &paths),
            }))
        }
        RepoIntelligenceAction::OwnershipChurn => {
            let max_results = bounded(input, "max_results", 20, 200);
            Ok(json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "churn": churn_from_git_log(root, max_results),
            }))
        }
    }
}

fn bounded(input: &Value, key: &str, default: usize, max: usize) -> usize {
    input
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(default)
        .clamp(1, max)
}
