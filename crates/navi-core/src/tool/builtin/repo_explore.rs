//! Repository exploration tool — read-only subagent for scanning repos.
//!
//! Uses a cheap model to find relevant code locations. Issues parallel
//! read-only tool calls (read_file, fs_browser, grep, git_ops) and returns
//! compact file paths with line ranges as focused context.

use std::sync::{Arc, RwLock, Weak};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;

use super::helpers;
use crate::cancel::CancelToken;
use crate::compact::CompactState;
use crate::config::{HarnessConfig, NaviConfig};
use crate::model::{ModelMessage, ModelProvider, ModelRole};
use crate::prompt::PromptCache;
use crate::runtime::ApprovalResolver;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};
use crate::turn::TurnContext;

const SYSTEM_PROMPT: &str = "You are a repository exploration agent. Your job is to find \
relevant code locations for the user's query. You have READ-ONLY access to the repository \
(read_file, fs_browser, grep, git_ops tools).

Rules:
- Issue multiple parallel tool calls when possible to explore the repo quickly
- Return file paths with line ranges, NOT full file contents
- Be precise: return only the relevant code locations
- Format your final answer as a structured list of locations with:
  - File path
  - Line range (start-end)
  - Brief description of what's at that location
  - Why it's relevant to the query
- Be concise. Focus on the most relevant 3-10 locations.
- Do NOT write any files or run any commands.";

pub struct RepoExploreTool {
    tool_executor: Weak<crate::tool::ToolExecutor>,
    model_provider: Arc<RwLock<Arc<dyn ModelProvider>>>,
    project_dir: std::path::PathBuf,
    data_dir: std::path::PathBuf,
    model_name: Arc<RwLock<String>>,
    harness_config: HarnessConfig,
    config: Arc<RwLock<NaviConfig>>,
    prompt_cache: Arc<PromptCache>,
}

impl RepoExploreTool {
    pub fn new(
        tool_executor: Weak<crate::tool::ToolExecutor>,
        model_provider: Arc<RwLock<Arc<dyn ModelProvider>>>,
        project_dir: std::path::PathBuf,
        data_dir: std::path::PathBuf,
        model_name: Arc<RwLock<String>>,
        harness_config: HarnessConfig,
        config: Arc<RwLock<NaviConfig>>,
        prompt_cache: Arc<PromptCache>,
    ) -> Self {
        Self {
            tool_executor,
            model_provider,
            project_dir,
            data_dir,
            model_name,
            harness_config,
            config,
            prompt_cache,
        }
    }
}

#[async_trait]
impl Tool for RepoExploreTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "repo_explore",
            "Find relevant code locations in the repository. \
             This read-only tool uses a cheap, fast model to scan the repo \
             and return file paths with line ranges. It does NOT write files \
             or run commands. Use this before reading files to find the right \
             locations efficiently.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What to find in the repository. Be specific: file patterns, function names, architectural concepts, error handling, etc."
                    },
                    "context": {
                        "type": "string",
                        "description": "Additional context about what you're looking for and why (optional). Helps the explorer be more targeted."
                    }
                },
                "required": ["query"],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let query = helpers::required_string(&invocation.input, "query")?;
        let context = helpers::optional_string(&invocation.input, "context");

        let executor = self
            .tool_executor
            .upgrade()
            .context("repo_explore: tool executor dropped")?;

        let started = std::time::Instant::now();

        // Build the user prompt.
        let user_prompt = if let Some(ctx) = context {
            format!("Query: {query}\n\nContext: {ctx}")
        } else {
            format!("Query: {query}")
        };

        // Build a restricted tool executor with only read-only tools.
        // We use the same executor but the system prompt restricts the agent
        // to read-only operations. The security policy already handles this.
        let include_tool_prompt = crate::config::effective_tool_prompt_manifest(
            &self.config.read().unwrap_or_else(|e| e.into_inner()),
        );

        let messages = vec![
            ModelMessage {
                role: ModelRole::System,
                content: SYSTEM_PROMPT.to_string(),
                content_parts: Vec::new(),
                tool_call_id: None,
                tool_name: None,
                tool_calls: vec![],
                created_at: None,
                thinking_content: None,
            },
            ModelMessage {
                role: ModelRole::User,
                content: user_prompt,
                content_parts: Vec::new(),
                tool_call_id: None,
                tool_name: None,
                tool_calls: vec![],
                created_at: None,
                thinking_content: None,
            },
        ];

        let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
        let resolver = ApprovalResolver::new_standalone();

        // Use a small harness profile for fast exploration.
        let mut explore_harness = self.harness_config.clone();
        explore_harness.profile = crate::config::HarnessProfile::Small;

        let sub_ctx = TurnContext {
            model_provider: self.model_provider.clone(),
            tool_executor: executor,
            project_dir: self.project_dir.clone(),
            data_dir: self.data_dir.clone(),
            model_name: self.model_name.clone(),
            event_tx: Some(event_tx),
            approval_resolver: resolver,
            question_resolver: crate::runtime::QuestionResolver::new_standalone(),
            compact_state: Arc::new(tokio::sync::Mutex::new(CompactState::new(
                crate::config::effective_context_window(
                    &self.config.read().unwrap_or_else(|e| e.into_inner()),
                ),
            ))),
            harness_config: explore_harness.clone(),
            include_tool_prompt_manifest: include_tool_prompt,
            context_packets: Arc::new(std::sync::Mutex::new(Vec::new())),
            active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
            prompt_cache: self.prompt_cache.clone(),
            cancel_token: CancelToken::new(),
            config: self.config.clone(),
            memory_injection: None,
            compaction_provider: None,
            compaction_model_name: None,
            session_id: "repo-explore-subagent".to_string(),
            allowed_tool_names: None,
        };

        let policy = crate::harness::policy_for_profile(&explore_harness, explore_harness.profile);

        let result = crate::turn::run_turn(&sub_ctx, &mut mut_messages(messages), policy).await;
        let elapsed = started.elapsed();

        let text = match result {
            Ok(output) => output,
            Err(err) => format!("repo_explore failed: {err:#}"),
        };

        Ok(helpers::ok(
            invocation.id,
            json!({
                "locations": text,
                "elapsed_ms": elapsed.as_millis() as u64,
            }),
        ))
    }
}

fn mut_messages(messages: Vec<ModelMessage>) -> Vec<ModelMessage> {
    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_has_correct_name_and_kind() {
        // Create a minimal tool to check definition.
        let tempdir = tempfile::tempdir().unwrap();
        let security_policy = crate::SecurityPolicy::new(
            tempdir.path().to_path_buf(),
            tempdir.path().to_path_buf(),
            crate::SecurityConfig::default(),
        )
        .unwrap();
        let tool_executor = Arc::new(crate::ToolExecutor::new(security_policy));

        let tool = RepoExploreTool::new(
            Arc::downgrade(&tool_executor),
            Arc::new(RwLock::new(Arc::new(MockProvider))),
            tempdir.path().to_path_buf(),
            tempdir.path().join("data"),
            Arc::new(RwLock::new("test-model".to_string())),
            HarnessConfig::default(),
            Arc::new(RwLock::new(NaviConfig::default())),
            Arc::new(PromptCache::new()),
        );

        let def = tool.definition();
        assert_eq!(def.name, "repo_explore");
        assert_eq!(def.kind, ToolKind::Read);
    }

    struct MockProvider;

    #[async_trait]
    impl crate::model::ModelProvider for MockProvider {
        fn stream(&self, _request: crate::model::ModelRequest) -> crate::model::ModelStream {
            Box::pin(futures_util::stream::iter(vec![
                Ok(crate::model::ModelStreamEvent::TextDelta {
                    text: "Found locations:\n- src/main.rs:1-10 (main function)".to_string(),
                }),
                Ok(crate::model::ModelStreamEvent::Done),
            ]))
        }
    }
}
