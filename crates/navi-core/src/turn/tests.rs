use super::*;
use crate::model::ModelResponse;
use crate::tool::{Tool, ToolDefinition, ToolKind};
use crate::{ModelStream, SecurityConfig, SecurityPolicy, ToolInvocation, ToolResult};
use async_trait::async_trait;
use futures_util::stream;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Mutex;

struct MockTool;
#[async_trait]
impl Tool for MockTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "test_tool".to_string(),
            description: "mock tool".to_string(),
            kind: ToolKind::Custom,
            input_schema: json!({}),
        }
    }
    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: true,
            output: json!({ "result": "mock ok" }),
        })
    }
}

struct MockProvider {
    calls: Mutex<usize>,
}

#[async_trait]
impl ModelProvider for MockProvider {
    fn stream(&self, _request: ModelRequest) -> ModelStream {
        let mut calls = self.calls.lock().unwrap();
        *calls += 1;
        let call_count = *calls;
        if call_count == 1 {
            Box::pin(stream::iter(vec![
                Ok(ModelStreamEvent::ToolCall(ToolInvocation {
                    id: "call-1".to_string(),
                    tool_name: "test_tool".to_string(),
                    input: json!({}),
                })),
                Ok(ModelStreamEvent::ToolCall(ToolInvocation {
                    id: "call-2".to_string(),
                    tool_name: "test_tool".to_string(),
                    input: json!({}),
                })),
                Ok(ModelStreamEvent::Done),
            ]))
        } else {
            Box::pin(stream::iter(vec![
                Ok(ModelStreamEvent::TextDelta {
                    text: "done".to_string(),
                }),
                Ok(ModelStreamEvent::Done),
            ]))
        }
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse> {
        ModelProvider::complete(self, request).await
    }
}

#[tokio::test]
async fn test_turn_loop_with_parallel_tools() {
    let tempdir = tempfile::tempdir().unwrap();
    let security_policy = SecurityPolicy::new(
        tempdir.path().to_path_buf(),
        tempdir.path().to_path_buf(),
        SecurityConfig::default(),
    )
    .unwrap();
    let mut executor = ToolExecutor::new(security_policy);
    executor.register_tool(Arc::new(MockTool));

    let ctx = TurnContext {
        model_provider: Arc::new(std::sync::RwLock::new(Arc::new(MockProvider {
            calls: Mutex::new(0),
        }))),
        tool_executor: Arc::new(executor),
        agent_control: AgentControl::new(),
        project_dir: tempdir.path().to_path_buf(),
        model_name: Arc::new(std::sync::RwLock::new("gpt-4".to_string())),
        event_tx: None,
        approval_resolver: crate::runtime::ApprovalResolver::new_for_test(),
        compact_state: Arc::new(tokio::sync::Mutex::new(CompactState::new(128_000))),
        harness_config: crate::config::HarnessConfig::default(),
        include_tool_prompt_manifest: false,
        agent_mode: None,
        context_packets: Arc::new(std::sync::Mutex::new(Vec::new())),
        active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
        cancel_token: CancelToken::new(),
        config: Arc::new(std::sync::RwLock::new(crate::config::NaviConfig::default())),
    };

    let mut messages = vec![];
    let policy = HarnessPolicy {
        profile: crate::config::HarnessProfile::Small,
        observation_max_bytes: 1000,
    };

    let result = run_turn(&ctx, &mut messages, policy).await.unwrap();
    assert_eq!(result, "done");
    let tool_results: Vec<_> = messages
        .iter()
        .filter(|m| m.role == ModelRole::Tool)
        .collect();
    assert_eq!(tool_results.len(), 2);
}

/// Helper to build a TurnContext pointing at a given project directory.
fn build_test_ctx(project_dir: PathBuf) -> TurnContext {
    let security_policy = SecurityPolicy::new(
        project_dir.clone(),
        project_dir.clone(),
        SecurityConfig::default(),
    )
    .unwrap();
    let mut executor = ToolExecutor::new(security_policy);
    executor.register_tool(Arc::new(MockTool));

    TurnContext {
        model_provider: Arc::new(std::sync::RwLock::new(Arc::new(MockProvider {
            calls: Mutex::new(0),
        }))),
        tool_executor: Arc::new(executor),
        agent_control: AgentControl::new(),
        project_dir,
        model_name: Arc::new(std::sync::RwLock::new("gpt-4".to_string())),
        event_tx: None,
        approval_resolver: crate::runtime::ApprovalResolver::new_for_test(),
        compact_state: Arc::new(tokio::sync::Mutex::new(CompactState::new(128_000))),
        harness_config: crate::config::HarnessConfig::default(),
        include_tool_prompt_manifest: false,
        agent_mode: None,
        context_packets: Arc::new(std::sync::Mutex::new(Vec::new())),
        active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
        cancel_token: CancelToken::new(),
        config: Arc::new(std::sync::RwLock::new(crate::config::NaviConfig::default())),
    }
}

#[tokio::test]
async fn test_ensure_system_prompt_reads_agents_md() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(
        tempdir.path().join("AGENTS.md"),
        "Custom project instructions",
    )
    .unwrap();
    let ctx = build_test_ctx(tempdir.path().to_path_buf());

    let mut messages = vec![];
    ensure_system_prompt(&ctx, &mut messages).await;

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, ModelRole::System);
    assert!(
        messages[0].content.contains("Custom project instructions"),
        "system prompt should include AGENTS.md content"
    );
    assert!(
        messages[0]
            .content
            .contains("AGENTS.md / Project Instructions"),
        "system prompt should have the AGENTS.md section header"
    );
}

#[tokio::test]
async fn test_ensure_system_prompt_falls_back_without_agents_md() {
    let tempdir = tempfile::tempdir().unwrap();
    // No AGENTS.md written — should use the default fallback.
    let ctx = build_test_ctx(tempdir.path().to_path_buf());

    let mut messages = vec![];
    ensure_system_prompt(&ctx, &mut messages).await;

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, ModelRole::System);
    assert!(
        messages[0]
            .content
            .contains("Default NAVI base instructions"),
        "system prompt should fall back to default instructions when AGENTS.md is absent"
    );
}

#[tokio::test]
async fn test_ensure_system_prompt_updates_existing_system_message() {
    let tempdir = tempfile::tempdir().unwrap();
    let ctx = build_test_ctx(tempdir.path().to_path_buf());

    let mut messages = vec![ModelMessage::system("stale prompt".to_string())];
    ensure_system_prompt(&ctx, &mut messages).await;

    // Should replace the existing system message, not add a second one.
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, ModelRole::System);
    assert!(
        messages[0]
            .content
            .contains("Default NAVI base instructions"),
        "existing system message should be replaced"
    );
    assert!(
        !messages[0].content.contains("stale prompt"),
        "old system message content should be gone"
    );
}

#[tokio::test]
async fn test_ensure_system_prompt_inserts_before_non_system_message() {
    let tempdir = tempfile::tempdir().unwrap();
    let ctx = build_test_ctx(tempdir.path().to_path_buf());

    let mut messages = vec![ModelMessage::user("hello".to_string())];
    ensure_system_prompt(&ctx, &mut messages).await;

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, ModelRole::System);
    assert_eq!(messages[1].role, ModelRole::User);
    assert_eq!(messages[1].content, "hello");
}

#[tokio::test]
async fn test_ensure_system_prompt_includes_agent_mode() {
    let tempdir = tempfile::tempdir().unwrap();
    let mut ctx = build_test_ctx(tempdir.path().to_path_buf());
    ctx.agent_mode = Some(crate::agent::AgentMode::Plan);

    let mut messages = vec![];
    ensure_system_prompt(&ctx, &mut messages).await;

    assert!(messages[0].content.contains("Agent Mode"));
    assert!(
        messages[0].content.contains("Plan"),
        "system prompt should include the active agent mode instructions"
    );
}

#[tokio::test]
async fn test_ensure_system_prompt_uses_loaded_config_profile() {
    let tempdir = tempfile::tempdir().unwrap();
    let ctx = build_test_ctx(tempdir.path().to_path_buf());
    let mut config = crate::config::NaviConfig::default();
    config.harness.profile = crate::config::HarnessProfile::Small;
    *ctx.config.write().unwrap() = config;

    let mut messages = vec![];
    ensure_system_prompt(&ctx, &mut messages).await;

    assert!(
        messages[0].content.contains("Harness profile: small"),
        "system prompt should reflect the loaded_config harness profile, got: {}",
        messages[0].content
    );
}

#[test]
fn test_resolve_approval_delegates_to_resolver() {
    let tempdir = tempfile::tempdir().unwrap();
    let ctx = build_test_ctx(tempdir.path().to_path_buf());

    let mut rx = ctx.approval_resolver.register("approval-1".to_string());

    let decision = crate::event::ApprovalDecision::Approved {
        id: "approval-1".to_string(),
    };
    let resolved = ctx.resolve_approval(decision);
    assert!(
        resolved,
        "resolve_approval should return true for a registered id"
    );

    let received = rx.try_recv().expect("receiver should have the decision");
    assert!(
        matches!(received, crate::event::ApprovalDecision::Approved { id } if id == "approval-1"),
        "receiver should get the approved decision"
    );
}

#[test]
fn test_resolve_approval_returns_false_for_unknown_id() {
    let tempdir = tempfile::tempdir().unwrap();
    let ctx = build_test_ctx(tempdir.path().to_path_buf());

    let decision = crate::event::ApprovalDecision::Approved {
        id: "unknown-id".to_string(),
    };
    let resolved = ctx.resolve_approval(decision);
    assert!(
        !resolved,
        "resolve_approval should return false when no pending approval matches"
    );
}

#[test]
fn test_resolve_approval_denied_delivers_denial() {
    let tempdir = tempfile::tempdir().unwrap();
    let ctx = build_test_ctx(tempdir.path().to_path_buf());

    let mut rx = ctx.approval_resolver.register("approval-2".to_string());

    let decision = crate::event::ApprovalDecision::Denied {
        id: "approval-2".to_string(),
    };
    let resolved = ctx.resolve_approval(decision);
    assert!(resolved);

    let received = rx.try_recv().expect("receiver should have the decision");
    assert!(
        matches!(received, crate::event::ApprovalDecision::Denied { id } if id == "approval-2"),
        "receiver should get the denied decision"
    );
}

#[test]
fn test_cancellation_token_not_requested_by_default() {
    let token = CancelToken::new();
    assert!(!token.is_requested());
}

#[test]
fn test_cancellation_token_reflects_cancel() {
    let token = CancelToken::new();
    token.cancel();
    assert!(token.is_requested());
}
