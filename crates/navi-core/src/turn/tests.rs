use super::*;
use crate::model::ModelResponse;
use crate::tool::{Tool, ToolDefinition, ToolKind, ToolMetadata};
use crate::{
    ModelStream, PermissionMode, SecurityConfig, SecurityPolicy, ToolInvocation, ToolResult,
};
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
        plan_review_resolver: crate::runtime::PlanReviewResolver::new_for_test(),
        sudo_password_resolver: crate::runtime::SudoPasswordResolver::new_for_test(),
        compact_state: Arc::new(tokio::sync::Mutex::new(CompactState::new(128_000))),
        harness_config: crate::config::HarnessConfig::default(),
        include_tool_prompt_manifest: false,
        context_packets: Arc::new(std::sync::Mutex::new(Vec::new())),
        available_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
        skill_pools: Arc::new(std::sync::Mutex::new(Vec::new())),
        active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
        prompt_cache: Arc::new(crate::prompt::PromptCache::new()),
        instructions: std::sync::Arc::new(std::sync::RwLock::new(None)),
        prompt_prefix: std::sync::Arc::new(std::sync::Mutex::new(None)),
        components: crate::RuntimeComponents::default(),
        cancel_token: CancelToken::new(),
        config: Arc::new(std::sync::RwLock::new(crate::config::NaviConfig::default())),
        memory_injection: None,
        compaction_provider: None,
        agent_mode: crate::plan_mode::AgentMode::Default,
        compaction_model_name: None,
        session_id: "test-session".to_string(),
        allowed_tool_names: None,
        is_subagent: false,
        memory_manager: Arc::new(std::sync::Mutex::new(None)),
        harness_card: None,
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
    // Memory injection is now in a developer message, not the system message.
    let memory_msg = requests[0]
        .messages
        .iter()
        .find(|msg| msg.role == ModelRole::Developer && msg.content.contains("session memory"))
        .expect("developer message with session memory");
    assert!(memory_msg.content.contains("durable session memory"));
    assert!(memory_msg.content.contains("Durable rebuild state"));
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
        plan_review_resolver: crate::runtime::PlanReviewResolver::new_for_test(),
        sudo_password_resolver: crate::runtime::SudoPasswordResolver::new_for_test(),
        compact_state: Arc::new(tokio::sync::Mutex::new(CompactState::new(128_000))),
        harness_config: crate::config::HarnessConfig::default(),
        include_tool_prompt_manifest: false,
        context_packets: Arc::new(std::sync::Mutex::new(Vec::new())),
        available_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
        skill_pools: Arc::new(std::sync::Mutex::new(Vec::new())),
        active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
        prompt_cache: Arc::new(crate::prompt::PromptCache::new()),
        instructions: std::sync::Arc::new(std::sync::RwLock::new(None)),
        prompt_prefix: std::sync::Arc::new(std::sync::Mutex::new(None)),
        components: crate::RuntimeComponents::default(),
        cancel_token: CancelToken::new(),
        config: Arc::new(std::sync::RwLock::new(crate::config::NaviConfig::default())),
        memory_injection: None,
        compaction_provider: None,
        compaction_model_name: None,
        session_id: "test-session".to_string(),
        agent_mode: crate::plan_mode::AgentMode::Default,
        allowed_tool_names: None,
        is_subagent: false,
        memory_manager: Arc::new(std::sync::Mutex::new(None)),
        harness_card: None,
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
    let config = SecurityConfig {
        permission_mode: PermissionMode::Yolo,
        ..SecurityConfig::default()
    };
    let security_policy =
        SecurityPolicy::new(project_dir.clone(), project_dir.clone(), config).unwrap();
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
        plan_review_resolver: crate::runtime::PlanReviewResolver::new_for_test(),
        sudo_password_resolver: crate::runtime::SudoPasswordResolver::new_for_test(),
        compact_state: Arc::new(tokio::sync::Mutex::new(CompactState::new(128_000))),
        harness_config: crate::config::HarnessConfig::default(),
        include_tool_prompt_manifest: false,
        context_packets: Arc::new(std::sync::Mutex::new(Vec::new())),
        available_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
        skill_pools: Arc::new(std::sync::Mutex::new(Vec::new())),
        active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
        prompt_cache: Arc::new(crate::prompt::PromptCache::new()),
        instructions: std::sync::Arc::new(std::sync::RwLock::new(None)),
        prompt_prefix: std::sync::Arc::new(std::sync::Mutex::new(None)),
        components: crate::RuntimeComponents::default(),
        cancel_token: CancelToken::new(),
        config: Arc::new(std::sync::RwLock::new(crate::config::NaviConfig::default())),
        memory_injection: None,
        compaction_provider: None,
        compaction_model_name: None,
        session_id: "test-session".to_string(),
        agent_mode: crate::plan_mode::AgentMode::Default,
        allowed_tool_names: None,
        is_subagent: false,
        memory_manager: Arc::new(std::sync::Mutex::new(None)),
        harness_card: None,
    }
}

#[test]
fn agent_turn_request_propagates_its_session_identity() {
    let tempdir = tempfile::tempdir().unwrap();
    let mut ctx = build_test_ctx(tempdir.path().to_path_buf());
    ctx.session_id = crate::SessionStore::create_id().into_inner();

    let request = build_model_request(&ctx, &[ModelMessage::user("hello")]);

    assert_eq!(request.session_id.as_deref(), Some(ctx.session_id.as_str()));
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

    // System message (base instructions) + developer message (AGENTS.md).
    assert!(messages.len() >= 2);
    assert_eq!(messages[0].role, ModelRole::System);
    let agents_msg = messages
        .iter()
        .find(|m| m.role == ModelRole::Developer && m.content.contains("AGENTS.md"))
        .expect("should have a developer message for AGENTS.md");
    assert!(
        agents_msg.content.contains("Custom project instructions"),
        "developer message should include AGENTS.md content"
    );
    assert!(
        agents_msg
            .content
            .contains("AGENTS.md / Project Instructions"),
        "developer message should have the AGENTS.md section header"
    );
}

#[tokio::test]
async fn ensure_system_prompt_freezes_prefix_across_context_changes() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(
        tempdir.path().join("AGENTS.md"),
        "Stable project instructions",
    )
    .unwrap();
    let ctx = build_test_ctx(tempdir.path().to_path_buf());

    let mut first = vec![ModelMessage::user("hello")];
    ensure_system_prompt(&ctx, &mut first).await;
    let frozen: Vec<_> = first
        .iter()
        .take_while(|m| matches!(m.role, ModelRole::System | ModelRole::Developer))
        .cloned()
        .collect();
    assert!(
        !frozen.is_empty(),
        "first call should install a prompt prefix"
    );

    // Mutating context that used to rebuild the system prefix mid-session.
    ctx.context_packets
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(crate::context::ContextPacket {
            id: Some("late".into()),
            source: crate::context::ContextSource::Other("test".into()),
            title: Some("late packet".into()),
            content: "this must not rewrite the frozen prefix".into(),
            priority: 10,
            metadata: json!({}),
        });
    *ctx.active_skills.lock().unwrap_or_else(|e| e.into_inner()) =
        vec![crate::skills::SkillManifest {
            id: "late-skill".into(),
            name: "late-skill".into(),
            description: Some("should not appear after freeze".into()),
            version: None,
            author: None,
            tags: vec![],
            requires: vec![],
            allow_tools: vec![],
            deny_tools: vec![],
            harness: false,
            pool: None,
            path: std::path::PathBuf::from("builtin:late-skill"),
            instructions: "skill body".into(),
            source: Default::default(),
            scope: Default::default(),
        }];

    let mut second = vec![ModelMessage::user("second turn")];
    ensure_system_prompt(&ctx, &mut second).await;
    let second_prefix: Vec<_> = second
        .iter()
        .take_while(|m| matches!(m.role, ModelRole::System | ModelRole::Developer))
        .cloned()
        .collect();

    assert_eq!(
        frozen.len(),
        second_prefix.len(),
        "prompt prefix length must stay identical after context/skill mutations"
    );
    for (a, b) in frozen.iter().zip(second_prefix.iter()) {
        assert_eq!(a.role, b.role);
        assert_eq!(
            a.content, b.content,
            "prompt prefix content must stay identical after context/skill mutations"
        );
    }
    assert!(
        !second.iter().any(|m| m.content.contains("late packet")),
        "late context packets must not rewrite a frozen session prefix"
    );
    assert!(
        !second.iter().any(|m| m.content.contains("late-skill")),
        "skills activated mid-session must not rewrite a frozen session prefix"
    );
}

#[tokio::test]
async fn test_ensure_system_prompt_falls_back_without_agents_md() {
    let tempdir = tempfile::tempdir().unwrap();
    // No AGENTS.md — omit the project instructions developer block entirely.
    let ctx = build_test_ctx(tempdir.path().to_path_buf());

    let mut messages = vec![];
    ensure_system_prompt(&ctx, &mut messages).await;

    assert!(!messages.is_empty());
    assert_eq!(messages[0].role, ModelRole::System);
    let agents_msg = messages
        .iter()
        .find(|m| m.role == ModelRole::Developer && m.content.contains("AGENTS.md"));
    assert!(
        agents_msg.is_none(),
        "should omit AGENTS.md developer block when the file is absent"
    );
}

#[tokio::test]
async fn test_ensure_system_prompt_updates_existing_system_message() {
    let tempdir = tempfile::tempdir().unwrap();
    let ctx = build_test_ctx(tempdir.path().to_path_buf());

    let mut messages = vec![ModelMessage::system("stale prompt".to_string())];
    ensure_system_prompt(&ctx, &mut messages).await;

    // Should replace the existing system message, not add a second one.
    let system_count = messages
        .iter()
        .filter(|m| m.role == ModelRole::System)
        .count();
    assert_eq!(system_count, 1, "should have exactly one system message");
    assert_eq!(messages[0].role, ModelRole::System);
    assert!(
        !messages[0].content.contains("stale prompt"),
        "old system message content should be gone"
    );
    let agents_msg = messages
        .iter()
        .find(|m| m.role == ModelRole::Developer && m.content.contains("AGENTS.md"));
    assert!(
        agents_msg.is_none(),
        "should omit AGENTS.md developer block when the file is absent"
    );
}

#[tokio::test]
async fn test_ensure_system_prompt_inserts_before_non_system_message() {
    let tempdir = tempfile::tempdir().unwrap();
    let ctx = build_test_ctx(tempdir.path().to_path_buf());

    let mut messages = vec![ModelMessage::user("hello".to_string())];
    ensure_system_prompt(&ctx, &mut messages).await;

    // System + developer messages are inserted before the user message.
    assert!(messages.len() >= 2);
    assert_eq!(messages[0].role, ModelRole::System);
    let user_msg = messages
        .iter()
        .find(|m| m.role == ModelRole::User)
        .expect("should have the user message");
    assert_eq!(user_msg.content, "hello");
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

/// Provider whose stream never yields — models a hung SSE body after cancel.
struct HangingStreamProvider;

#[async_trait]
impl ModelProvider for HangingStreamProvider {
    fn stream(&self, _request: ModelRequest) -> ModelStream {
        Box::pin(futures_util::stream::pending())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collect_model_output_aborts_hanging_stream_on_cancel() {
    let tempdir = tempfile::tempdir().unwrap();
    let mut ctx = build_test_ctx(tempdir.path().to_path_buf());
    ctx.model_provider = Arc::new(std::sync::RwLock::new(Arc::new(HangingStreamProvider)));
    let cancel = ctx.cancel_token.clone();

    let turn = tokio::spawn(async move {
        let mut messages = vec![ModelMessage::user("hello")];
        let policy = crate::harness::policy_for_profile(
            &crate::config::HarnessConfig::default(),
            crate::config::HarnessProfile::Small,
        );
        run_turn(&ctx, &mut messages, policy).await
    });

    // Let the turn reach `stream.next()` before cancelling.
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancel.cancel();

    let result = tokio::time::timeout(Duration::from_secs(2), turn)
        .await
        .expect("cancelled turn must not hang on stream.next()")
        .expect("join turn task");
    let err = result.expect_err("cancelled hanging stream should error");
    assert!(
        err.to_string().contains("turn cancelled"),
        "expected turn cancelled, got: {err}"
    );
}

#[test]
fn rewrite_unsupported_attachments_keeps_supported_images_and_routes_audio() {
    let tempdir = tempfile::tempdir().unwrap();
    let ctx = build_test_ctx(tempdir.path().to_path_buf());
    let mut config = crate::config::NaviConfig::default();
    config.model.provider = "test-provider".to_string();
    config.model.name = "chat-model".to_string();
    config.providers = vec![crate::config::ProviderConfig {
        id: "test-provider".to_string(),
        label: "Test".to_string(),
        description: String::new(),
        kind: crate::config::ProviderKind::OpenAiResponses,
        api_key_env: "TEST_KEY".to_string(),
        base_url: Some("https://example.test/v1".to_string()),
        models: vec![crate::config::ProviderModelConfig {
            name: "chat-model".to_string(),
            task_size: Some(crate::config::ModelTaskSize::Large),
            context_window_tokens: None,
            max_output_tokens: None,
            recommended_temperature: None,
            supports_thinking: None,
            supports_images: Some(true),
            supports_audio: Some(false),
            supports_video: None,
            supports_documents: None,
            tool_prompt_manifest: None,
            pricing_input_per_1m: None,
            pricing_output_per_1m: None,
            reasoning_levels: Vec::new(),
            default_reasoning_effort: None,
        }],
        ..Default::default()
    }];
    *ctx.config.write().unwrap() = config;

    let messages = vec![ModelMessage::user_multimodal(
        "inspect",
        vec![
            ContentPart::Text {
                text: "inspect".to_string(),
            },
            ContentPart::Image {
                media_type: "image/png".to_string(),
                data: "image-data".to_string(),
            },
            ContentPart::Audio {
                media_type: "audio/mpeg".to_string(),
                data: "audio-data".to_string(),
                name: Some("clip.mp3".to_string()),
            },
        ],
    )];

    let rewritten = rewrite_unsupported_attachments(&ctx, &messages);
    assert!(matches!(
        rewritten[0].content_parts[1],
        ContentPart::Image { .. }
    ));
    let routed = rewritten[0].content_parts[2].as_text().unwrap();
    assert!(
        routed.contains("attachment unavailable") || routed.contains("cannot view"),
        "unsupported attachments should become a short capability notice: {routed}"
    );
    assert!(
        routed.contains("audio"),
        "notice should mention kind: {routed}"
    );
    assert!(
        !routed.contains("audio-data"),
        "must not inline attachment base64 into the chat prompt: {routed}"
    );
}

#[test]
fn rewrite_unsupported_images_do_not_inline_base64() {
    let tempdir = tempfile::tempdir().unwrap();
    let ctx = build_test_ctx(tempdir.path().to_path_buf());
    let mut config = crate::config::NaviConfig::default();
    config.model.provider = "opencode".to_string();
    config.model.name = "mimo-v2.5-free".to_string();
    config.providers = vec![crate::config::ProviderConfig {
        id: "opencode".to_string(),
        label: "OpenCode".to_string(),
        description: String::new(),
        kind: crate::config::ProviderKind::OpenAiChatCompletions,
        api_key_env: "OPENCODE_API_KEY".to_string(),
        base_url: Some("https://example.test/v1".to_string()),
        models: vec![crate::config::ProviderModelConfig {
            name: "mimo-v2.5-free".to_string(),
            task_size: Some(crate::config::ModelTaskSize::Small),
            context_window_tokens: None,
            max_output_tokens: None,
            recommended_temperature: None,
            supports_thinking: None,
            supports_images: Some(false),
            supports_audio: None,
            supports_video: None,
            supports_documents: None,
            tool_prompt_manifest: None,
            pricing_input_per_1m: None,
            pricing_output_per_1m: None,
            reasoning_levels: Vec::new(),
            default_reasoning_effort: None,
        }],
        ..Default::default()
    }];
    *ctx.config.write().unwrap() = config;

    let huge = "A".repeat(50_000);
    let messages = vec![ModelMessage::user_multimodal(
        "analise essa imagem",
        vec![
            ContentPart::Text {
                text: "analise essa imagem".to_string(),
            },
            ContentPart::Image {
                media_type: "image/png".to_string(),
                data: huge.clone(),
            },
        ],
    )];

    let rewritten = rewrite_unsupported_attachments(&ctx, &messages);
    let text = rewritten[0].content_parts[1].as_text().unwrap();
    assert!(
        !text.contains(&huge),
        "must not dump image base64 into prompt"
    );
    assert!(text.contains("image/png"));
    assert!(text.contains("cannot view") || text.contains("unavailable"));
}

#[test]
fn allowlist_deny_message_distinguishes_subagent_and_harness() {
    let sub = super::tool_allowlist_deny_message(true, "bash");
    let root = super::tool_allowlist_deny_message(false, "bash");
    assert!(sub.contains("for this subagent"), "subagent deny: {sub}");
    assert!(
        !root.contains("subagent"),
        "root/harness deny must not say subagent: {root}"
    );
    assert!(root.contains("for the active harness"), "root deny: {root}");
}
