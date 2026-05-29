use super::*;
use crate::config::{ApprovalConfig, HarnessConfig};
use crate::{
    AgentMode, ModelRequest, ModelStream, ModelStreamEvent, NaviConfig, SecurityConfig,
    ToolInvocation,
};
use anyhow::Result;
use async_trait::async_trait;
use futures_util::stream;
use serde_json::json;
use std::sync::Mutex;
use std::time::Duration;
use tokio::time::timeout;

struct MockToolProvider {
    calls: Arc<Mutex<usize>>,
    file_path: String,
}

#[async_trait]
impl ModelProvider for MockToolProvider {
    fn stream(&self, request: ModelRequest) -> ModelStream {
        let mut calls = self.calls.lock().expect("calls");
        *calls += 1;
        let call_number = *calls;
        drop(calls);

        if call_number == 1 {
            assert!(!request.tools.is_empty());
            assert!(request.messages[0].content.contains("Workflow contract"));
            assert!(request.messages[0].content.contains("Agent mode: Plan"));
            assert!(request.messages[0].content.contains("runtime context"));
            Box::pin(stream::iter(vec![Ok(ModelStreamEvent::ToolCall(
                ToolInvocation {
                    id: "call-1".to_string(),
                    tool_name: "read_file".to_string(),
                    input: json!({ "path": self.file_path }),
                },
            ))]))
        } else {
            assert!(
                request
                    .messages
                    .iter()
                    .any(|message| { message.role == crate::model::ModelRole::Tool })
            );
            Box::pin(stream::iter(vec![
                Ok(ModelStreamEvent::TextDelta {
                    text: "read complete".to_string(),
                }),
                Ok(ModelStreamEvent::Done),
            ]))
        }
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse> {
        ModelProvider::complete(self, request).await
    }
}

struct SimpleProvider;

#[async_trait]
impl ModelProvider for SimpleProvider {
    fn stream(&self, _request: ModelRequest) -> ModelStream {
        Box::pin(stream::iter(vec![
            Ok(ModelStreamEvent::TextDelta {
                text: "simple".to_string(),
            }),
            Ok(ModelStreamEvent::Done),
        ]))
    }
}

#[tokio::test]
async fn headless_runtime_executes_read_tools_and_continues() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let file = tempdir.path().join("Cargo.toml");
    std::fs::write(&file, "[package]\nname = \"demo\"\n").expect("write file");
    let loaded_config = crate::LoadedConfig {
        config: NaviConfig {
            harness: HarnessConfig::default(),
            approvals: ApprovalConfig::default(),
            security: SecurityConfig::default(),
            ..NaviConfig::default()
        },
        global_config_path: None,
        project_config_path: None,
        data_dir: tempdir.path().join("data"),
    };
    let provider = Arc::new(MockToolProvider {
        calls: Arc::new(Mutex::new(0)),
        file_path: file.display().to_string(),
    });
    let mut runtime = AgentRuntime::new(AgentRuntimeOptions {
        loaded_config,
        model_provider: provider,
        project_dir: tempdir.path().to_path_buf(),
        tool_executor: None,
        agent_mode: Some(AgentMode::Plan),
        context_packets: vec![crate::ContextPacket {
            id: Some("ctx-1".to_string()),
            source: crate::ContextSource::FocusThread,
            title: Some("focus".to_string()),
            content: "runtime context".to_string(),
            priority: 10,
            metadata: json!({}),
        }],
        active_skills: Vec::new(),
        initial_messages: Vec::new(),
        session_id: None,
        event_tx: None,
    });

    let response = runtime
        .submit_task("inspect".to_string())
        .await
        .expect("run");

    assert_eq!(response.text, "read complete");
    assert!(
        runtime
            .events()
            .iter()
            .any(|event| matches!(event, AgentEvent::ToolCompleted(_)))
    );
    assert!(
        runtime
            .events()
            .iter()
            .any(|event| matches!(event, AgentEvent::HarnessTrace(_)))
    );
}

#[tokio::test]
async fn runtime_session_lifecycle_streams_events_and_snapshots() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let loaded_config = crate::LoadedConfig {
        config: NaviConfig {
            harness: HarnessConfig::default(),
            approvals: ApprovalConfig::default(),
            security: SecurityConfig::default(),
            ..NaviConfig::default()
        },
        global_config_path: None,
        project_config_path: None,
        data_dir: tempdir.path().join("data"),
    };
    let provider = Arc::new(SimpleProvider);
    let mut runtime = AgentRuntime::new(AgentRuntimeOptions {
        loaded_config,
        model_provider: provider,
        project_dir: tempdir.path().to_path_buf(),
        tool_executor: None,
        agent_mode: Some(AgentMode::Plan),
        context_packets: vec![crate::ContextPacket {
            id: Some("ctx-1".to_string()),
            source: crate::ContextSource::FocusThread,
            title: Some("focus".to_string()),
            content: "runtime context".to_string(),
            priority: 10,
            metadata: json!({}),
        }],
        active_skills: Vec::new(),
        initial_messages: Vec::new(),
        session_id: None,
        event_tx: None,
    });

    let mut events = runtime.stream_events();
    let session_id = runtime.start_session().expect("start session");

    let first_event = timeout(Duration::from_secs(1), events.recv())
        .await
        .expect("session event timeout")
        .expect("session event");
    assert!(matches!(
        first_event.kind,
        RuntimeEventKind::SessionStarted { session_id: ref id } if id.as_str() == session_id.as_str()
    ));

    let response = runtime
        .send_turn("inspect".to_string())
        .await
        .expect("run turn");
    assert_eq!(response.text, "simple");

    let mut saw_turn_started = false;
    let mut saw_turn_completed = false;
    for _ in 0..8 {
        let event = timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("turn event timeout")
            .expect("turn event");
        match event.kind {
            RuntimeEventKind::TurnStarted { .. } => saw_turn_started = true,
            RuntimeEventKind::TurnCompleted { ref text, .. } => {
                saw_turn_completed = true;
                assert_eq!(text, "simple");
                break;
            }
            _ => {}
        }
    }
    assert!(saw_turn_started);
    assert!(saw_turn_completed);

    let snapshot = runtime.snapshot_session().expect("snapshot");
    assert_eq!(snapshot.id.as_str(), session_id.as_str());
    assert!(snapshot.title.is_some());
    let snapshot_path = runtime
        .session_store()
        .root()
        .join(format!("{}.json", snapshot.id.as_str()));
    assert!(snapshot_path.exists());
}

#[tokio::test]
async fn runtime_uses_requested_session_id_once() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let loaded_config = crate::LoadedConfig {
        config: NaviConfig {
            harness: HarnessConfig::default(),
            approvals: ApprovalConfig::default(),
            security: SecurityConfig::default(),
            ..NaviConfig::default()
        },
        global_config_path: None,
        project_config_path: None,
        data_dir: tempdir.path().join("data"),
    };
    let provider = Arc::new(SimpleProvider);
    let mut runtime = AgentRuntime::new(AgentRuntimeOptions {
        loaded_config,
        model_provider: provider,
        project_dir: tempdir.path().to_path_buf(),
        tool_executor: None,
        agent_mode: Some(AgentMode::Tutor),
        context_packets: Vec::new(),
        active_skills: Vec::new(),
        initial_messages: Vec::new(),
        session_id: Some(crate::SessionId::new(
            "navi_tutor_algoritmos_2026-05-25_14-32-10".to_string(),
        )),
        event_tx: None,
    });

    let first_id = runtime.start_session().expect("start first session");
    let second_id = runtime.start_session().expect("start second session");

    assert_eq!(first_id.as_str(), "navi_tutor_algoritmos_2026-05-25_14-32-10");
    assert_ne!(second_id.as_str(), first_id.as_str());
}
