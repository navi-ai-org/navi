use super::*;
use crate::model::ModelResponse;
use crate::tool::{Tool, ToolDefinition, ToolKind};
use crate::{ModelStream, SecurityConfig, SecurityPolicy, ToolInvocation, ToolResult};
use async_trait::async_trait;
use futures_util::stream;
use serde_json::json;
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
        model_provider: Arc::new(MockProvider {
            calls: Mutex::new(0),
        }),
        tool_executor: Arc::new(executor),
        agent_control: AgentControl::new(),
        project_dir: tempdir.path().to_path_buf(),
        model_name: "gpt-4".to_string(),
        event_tx: None,
        approval_resolver: crate::runtime::ApprovalResolver::new_for_test(),
        compact_state: Arc::new(tokio::sync::Mutex::new(CompactState::new(128_000))),
        harness_config: crate::config::HarnessConfig::default(),
        include_tool_prompt_manifest: false,
        agent_mode: None,
        context_packets: Vec::new(),
        active_skills: Vec::new(),
        cancel_token: CancelToken::new(),
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
