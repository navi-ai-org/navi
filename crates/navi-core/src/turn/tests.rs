use super::*;
use crate::model::ModelResponse;
use crate::tool::{Tool, ToolDefinition, ToolKind, ToolMetadata};
use crate::{ModelStream, SecurityConfig, SecurityPolicy, ToolInvocation, ToolResult};
use async_trait::async_trait;
use futures_util::stream;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

struct MockTool;
#[async_trait]
impl Tool for MockTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "test_tool".to_string(),
            description: "mock tool".to_string(),
            kind: ToolKind::Custom,
            input_schema: json!({}),
            metadata: Default::default(),
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

#[derive(Clone, Default)]
struct ParallelProbe {
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
}

impl ParallelProbe {
    fn enter(&self) {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        loop {
            let current = self.max_active.load(Ordering::SeqCst);
            if active <= current
                || self
                    .max_active
                    .compare_exchange(current, active, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                break;
            }
        }
    }

    fn exit(&self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }

    fn max_active(&self) -> usize {
        self.max_active.load(Ordering::SeqCst)
    }
}

struct SleepingTool {
    name: String,
    metadata: ToolMetadata,
    probe: ParallelProbe,
}

impl SleepingTool {
    fn shared(name: &str, probe: ParallelProbe) -> Self {
        Self {
            name: name.to_string(),
            metadata: ToolMetadata::reader("test", &["parallel", "read"]),
            probe,
        }
    }

    fn exclusive(name: &str, probe: ParallelProbe) -> Self {
        let mut metadata = ToolMetadata::reader("test", &["exclusive"]);
        metadata.is_read_only = false;
        metadata.is_concurrency_safe = false;
        Self {
            name: name.to_string(),
            metadata,
            probe,
        }
    }
}

#[async_trait]
impl Tool for SleepingTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.clone(),
            description: "sleeping probe tool".to_string(),
            kind: ToolKind::Read,
            input_schema: json!({}),
            metadata: self.metadata.clone(),
        }
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        self.probe.enter();
        tokio::time::sleep(Duration::from_millis(30)).await;
        self.probe.exit();
        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: true,
            output: json!({ "result": "ok" }),
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

struct MalformedToolProvider {
    calls: Mutex<usize>,
}

#[async_trait]
impl ModelProvider for MalformedToolProvider {
    fn stream(&self, _request: ModelRequest) -> ModelStream {
        let mut calls = self.calls.lock().unwrap();
        *calls += 1;
        Box::pin(stream::iter(vec![
            Ok(ModelStreamEvent::ToolCall(ToolInvocation {
                id: format!("call-{}", *calls),
                tool_name: "read".to_string(),
                input: json!({ "raw_arguments": "{\"path\": " }),
            })),
            Ok(ModelStreamEvent::Done),
        ]))
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse> {
        ModelProvider::complete(self, request).await
    }
}

struct CapturingProvider {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

#[async_trait]
impl ModelProvider for CapturingProvider {
    fn stream(&self, request: ModelRequest) -> ModelStream {
        self.requests.lock().unwrap().push(request);
        Box::pin(stream::iter(vec![
            Ok(ModelStreamEvent::TextDelta {
                text: "captured".to_string(),
            }),
            Ok(ModelStreamEvent::Done),
        ]))
    }
}

struct DegenerateOutputProvider;

#[async_trait]
impl ModelProvider for DegenerateOutputProvider {
    fn stream(&self, _request: ModelRequest) -> ModelStream {
        Box::pin(stream::iter(vec![
            Ok(ModelStreamEvent::TextDelta {
                text: "a".repeat(80),
            }),
            Ok(ModelStreamEvent::Done),
        ]))
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
        project_dir: tempdir.path().to_path_buf(),
        data_dir: tempdir.path().join("data"),
        model_name: Arc::new(std::sync::RwLock::new("gpt-4".to_string())),
        event_tx: None,
        approval_resolver: crate::runtime::ApprovalResolver::new_for_test(),
        question_resolver: crate::runtime::QuestionResolver::new_for_test(),
        compact_state: Arc::new(tokio::sync::Mutex::new(CompactState::new(128_000))),
        harness_config: crate::config::HarnessConfig::default(),
        include_tool_prompt_manifest: false,
        context_packets: Arc::new(std::sync::Mutex::new(Vec::new())),
        active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
        prompt_cache: Arc::new(crate::prompt::PromptCache::new()),
        components: crate::RuntimeComponents::default(),
        cancel_token: CancelToken::new(),
        config: Arc::new(std::sync::RwLock::new(crate::config::NaviConfig::default())),
        memory_injection: None,
        compaction_provider: None,
        compaction_model_name: None,
        session_id: "test-session".to_string(),
        allowed_tool_names: None,
    };

    let mut messages = vec![];
    let policy = crate::harness::policy_for_profile(
        &crate::config::HarnessConfig {
            observation_bytes_small: 1000,
            ..crate::config::HarnessConfig::default()
        },
        crate::config::HarnessProfile::Small,
    );

    let result = run_turn(&ctx, &mut messages, policy).await.unwrap();
    assert_eq!(result, "done");
    let tool_results: Vec<_> = messages
        .iter()
        .filter(|m| m.role == ModelRole::Tool)
        .collect();
    assert_eq!(tool_results.len(), 2);
}

#[tokio::test]
async fn shared_tool_calls_overlap_within_one_model_batch() {
    let tempdir = tempfile::tempdir().unwrap();
    let mut ctx = build_test_ctx(tempdir.path().to_path_buf());
    let probe = ParallelProbe::default();
    Arc::get_mut(&mut ctx.tool_executor)
        .unwrap()
        .register_tool(Arc::new(SleepingTool::shared(
            "shared_probe",
            probe.clone(),
        )));
    let mut messages = Vec::new();
    let mut run_state = AgentRunState::default();
    let policy = crate::harness::policy_for_profile(
        &crate::config::HarnessConfig::default(),
        crate::config::HarnessProfile::Small,
    );

    let output = ModelTurnOutput {
        text: String::new(),
        thinking: String::new(),
        tool_calls: vec![
            ToolInvocation {
                id: "call-1".to_string(),
                tool_name: "shared_probe".to_string(),
                input: json!({}),
            },
            ToolInvocation {
                id: "call-2".to_string(),
                tool_name: "shared_probe".to_string(),
                input: json!({}),
            },
        ],
        harness_stop: None,
    };

    assert!(
        handle_tool_calls(&ctx, &mut messages, &mut run_state, policy, output)
            .await
            .is_none()
    );
    assert_eq!(probe.max_active(), 2);
}

#[tokio::test]
async fn exclusive_tool_call_serializes_a_model_batch() {
    let tempdir = tempfile::tempdir().unwrap();
    let mut ctx = build_test_ctx(tempdir.path().to_path_buf());
    let probe = ParallelProbe::default();
    let executor = Arc::get_mut(&mut ctx.tool_executor).unwrap();
    executor.register_tool(Arc::new(SleepingTool::shared(
        "shared_probe",
        probe.clone(),
    )));
    executor.register_tool(Arc::new(SleepingTool::exclusive(
        "exclusive_probe",
        probe.clone(),
    )));
    let mut messages = Vec::new();
    let mut run_state = AgentRunState::default();
    let policy = crate::harness::policy_for_profile(
        &crate::config::HarnessConfig::default(),
        crate::config::HarnessProfile::Small,
    );

    let output = ModelTurnOutput {
        text: String::new(),
        thinking: String::new(),
        tool_calls: vec![
            ToolInvocation {
                id: "call-1".to_string(),
                tool_name: "shared_probe".to_string(),
                input: json!({}),
            },
            ToolInvocation {
                id: "call-2".to_string(),
                tool_name: "exclusive_probe".to_string(),
                input: json!({}),
            },
            ToolInvocation {
                id: "call-3".to_string(),
                tool_name: "shared_probe".to_string(),
                input: json!({}),
            },
        ],
        harness_stop: None,
    };

    assert!(
        handle_tool_calls(&ctx, &mut messages, &mut run_state, policy, output)
            .await
            .is_none()
    );
    assert_eq!(probe.max_active(), 1);
}

#[tokio::test]
async fn degenerate_streaming_output_stops_the_turn() {
    let tempdir = tempfile::tempdir().unwrap();
    let mut ctx = build_test_ctx(tempdir.path().to_path_buf());
    ctx.model_provider = Arc::new(std::sync::RwLock::new(Arc::new(DegenerateOutputProvider)));
    let mut messages = vec![ModelMessage::user("repeat")];
    let policy = crate::harness::policy_for_profile(
        &crate::config::HarnessConfig::default(),
        crate::config::HarnessProfile::Small,
    );

    let result = run_turn(&ctx, &mut messages, policy).await.unwrap();

    assert!(result.contains("degenerate_model_output"));
    assert!(
        messages
            .iter()
            .any(|message| message.role == ModelRole::Assistant
                && message.content.contains("degenerate_model_output"))
    );
}

#[tokio::test]
async fn system_prompt_includes_session_memory_and_rebuild_context() {
    let tempdir = tempfile::tempdir().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut ctx = build_test_ctx(tempdir.path().to_path_buf());
    ctx.model_provider = Arc::new(std::sync::RwLock::new(Arc::new(CapturingProvider {
        requests: requests.clone(),
    })));
    ctx.memory_injection = Some("Previous session context: durable session memory".to_string());
    {
        let mut state = ctx.compact_state.lock().await;
        state.rebuild_context =
            Some("=== SESSION CHECKPOINT ===\nDurable rebuild state".to_string());
    }

    let mut messages = vec![ModelMessage::user("continue")];
    let policy = crate::harness::policy_for_profile(
        &crate::config::HarnessConfig::default(),
        crate::config::HarnessProfile::Small,
    );

    let _ = run_turn(&ctx, &mut messages, policy).await.unwrap();

    let requests = requests.lock().unwrap();
    let system = requests[0]
        .messages
        .iter()
        .find(|msg| msg.role == ModelRole::System)
        .expect("system message");
    assert!(system.content.contains("durable session memory"));
    assert!(system.content.contains("Durable rebuild state"));
}

#[tokio::test]
async fn history_sync_continues_after_messages_are_shortened() {
    let tempdir = tempfile::tempdir().unwrap();
    let project_dir = tempdir.path().join("project");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&project_dir).unwrap();

    let mut ctx = build_test_ctx(project_dir.clone());
    ctx.data_dir = data_dir.clone();
    ctx.session_id = "history-shortened".to_string();

    let mut messages = vec![
        ModelMessage::system("base prompt"),
        ModelMessage::user("first user"),
        ModelMessage::assistant("first assistant"),
    ];
    sync_messages_to_history(&ctx, &messages).await.unwrap();

    let manager = crate::memory::MemoryManager::new(
        project_dir.clone(),
        data_dir.clone(),
        &ctx.active_config().memory,
    )
    .unwrap();
    assert_eq!(
        manager
            .history
            .get_event_count(&ctx.session_id, "message")
            .unwrap(),
        3
    );

    messages.clear();
    messages.push(ModelMessage::system("rebuilt prompt"));
    messages.push(ModelMessage::user("post rebuild user"));
    sync_messages_to_history(&ctx, &messages).await.unwrap();

    assert_eq!(
        manager
            .history
            .get_event_count(&ctx.session_id, "message")
            .unwrap(),
        5
    );
}

#[tokio::test]
async fn malformed_tool_arguments_stop_the_turn() {
    let tempdir = tempfile::tempdir().unwrap();
    let security_policy = SecurityPolicy::new(
        tempdir.path().to_path_buf(),
        tempdir.path().to_path_buf(),
        SecurityConfig::default(),
    )
    .unwrap();
    let executor = ToolExecutor::new(security_policy);
    let provider = Arc::new(MalformedToolProvider {
        calls: Mutex::new(0),
    });

    let ctx = TurnContext {
        model_provider: Arc::new(std::sync::RwLock::new(provider.clone())),
        tool_executor: Arc::new(executor),
        project_dir: tempdir.path().to_path_buf(),
        data_dir: tempdir.path().join("data"),
        model_name: Arc::new(std::sync::RwLock::new("gpt-4".to_string())),
        event_tx: None,
        approval_resolver: crate::runtime::ApprovalResolver::new_for_test(),
        question_resolver: crate::runtime::QuestionResolver::new_for_test(),
        compact_state: Arc::new(tokio::sync::Mutex::new(CompactState::new(128_000))),
        harness_config: crate::config::HarnessConfig::default(),
        include_tool_prompt_manifest: false,
        context_packets: Arc::new(std::sync::Mutex::new(Vec::new())),
        active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
        prompt_cache: Arc::new(crate::prompt::PromptCache::new()),
        components: crate::RuntimeComponents::default(),
        cancel_token: CancelToken::new(),
        config: Arc::new(std::sync::RwLock::new(crate::config::NaviConfig::default())),
        memory_injection: None,
        compaction_provider: None,
        compaction_model_name: None,
        session_id: "test-session".to_string(),
        allowed_tool_names: None,
    };
    let policy = crate::harness::policy_for_profile(
        &crate::config::HarnessConfig {
            max_consecutive_malformed_arguments: 2,
            ..crate::config::HarnessConfig::default()
        },
        crate::config::HarnessProfile::Small,
    );
    let mut messages = vec![];

    let result = run_turn(&ctx, &mut messages, policy).await.unwrap();

    assert!(result.contains("consecutive_malformed_arguments"));
    assert_eq!(*provider.calls.lock().unwrap(), 2);
}

#[test]
fn think_tag_splitter_moves_inline_thinking_out_of_text() {
    let mut splitter = ThinkTagSplitter::default();

    assert_eq!(
        splitter.push("hello <think>hidden</think> world"),
        vec![
            SplitTextPart::Text("hello ".to_string()),
            SplitTextPart::Thinking("hidden".to_string()),
            SplitTextPart::Text(" world".to_string()),
        ]
    );
}

#[test]
fn think_tag_splitter_handles_tags_split_across_chunks() {
    let mut splitter = ThinkTagSplitter::default();
    let mut parts = Vec::new();

    parts.extend(splitter.push("<thi"));
    parts.extend(splitter.push("nk>hidden</thi"));
    parts.extend(splitter.push("nk>visible"));

    assert_eq!(
        parts,
        vec![
            SplitTextPart::Thinking("hidden".to_string()),
            SplitTextPart::Text("visible".to_string()),
        ]
    );
}

#[test]
fn think_tag_splitter_drops_partial_open_tag_on_drain() {
    let mut splitter = ThinkTagSplitter::default();

    assert!(splitter.push("<thi").is_empty());
    assert!(splitter.drain_pending().is_empty());
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
        data_dir: project_dir.join("data"),
        project_dir,
        model_name: Arc::new(std::sync::RwLock::new("gpt-4".to_string())),
        event_tx: None,
        approval_resolver: crate::runtime::ApprovalResolver::new_for_test(),
        question_resolver: crate::runtime::QuestionResolver::new_for_test(),
        compact_state: Arc::new(tokio::sync::Mutex::new(CompactState::new(128_000))),
        harness_config: crate::config::HarnessConfig::default(),
        include_tool_prompt_manifest: false,
        context_packets: Arc::new(std::sync::Mutex::new(Vec::new())),
        active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
        prompt_cache: Arc::new(crate::prompt::PromptCache::new()),
        components: crate::RuntimeComponents::default(),
        cancel_token: CancelToken::new(),
        config: Arc::new(std::sync::RwLock::new(crate::config::NaviConfig::default())),
        memory_injection: None,
        compaction_provider: None,
        compaction_model_name: None,
        session_id: "test-session".to_string(),
        allowed_tool_names: None,
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
