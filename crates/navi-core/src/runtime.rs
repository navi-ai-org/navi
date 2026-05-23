use crate::agent::AgentMode;
use crate::config::LoadedConfig;
use crate::context::ContextPacket;
use crate::event::AgentEvent;
use crate::harness::select_harness_policy;
use crate::model::{ModelProvider, ModelResponse};
use crate::security::SecurityPolicy;
use crate::tool::{Tool, ToolExecutor};
use crate::{ModelOption, available_model_options, canonical_provider_id};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

pub struct AgentRuntimeOptions {
    pub loaded_config: LoadedConfig,
    pub model_provider: Arc<dyn ModelProvider>,
    pub project_dir: PathBuf,
    pub tool_executor: Option<Arc<ToolExecutor>>,
    pub agent_mode: Option<AgentMode>,
    pub context_packets: Vec<ContextPacket>,
    pub event_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
}

pub struct AgentRuntime {
    loaded_config: LoadedConfig,
    model_provider: Arc<dyn ModelProvider>,
    project_dir: PathBuf,
    tool_executor: Option<Arc<ToolExecutor>>,
    agent_mode: Option<AgentMode>,
    context_packets: Vec<ContextPacket>,
    event_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
    events: Vec<AgentEvent>,
}

pub type NaviRuntime = AgentRuntime;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ApprovalConfig, HarnessConfig, ModelRequest, ModelStream, NaviConfig, SecurityConfig,
        ToolInvocation,
    };
    use anyhow::Result;
    use async_trait::async_trait;
    use futures_util::stream;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

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
                Box::pin(stream::iter(vec![Ok(
                    crate::model::ModelStreamEvent::ToolCall(ToolInvocation {
                        id: "call-1".to_string(),
                        tool_name: "read_file".to_string(),
                        input: json!({ "path": self.file_path }),
                    }),
                )]))
            } else {
                assert!(
                    request
                        .messages
                        .iter()
                        .any(|message| message.role == crate::model::ModelRole::Tool)
                );
                Box::pin(stream::iter(vec![
                    Ok(crate::model::ModelStreamEvent::TextDelta {
                        text: "read complete".to_string(),
                    }),
                    Ok(crate::model::ModelStreamEvent::Done),
                ]))
            }
        }

        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse> {
            ModelProvider::complete(self, request).await
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
            agent_mode: Some(crate::AgentMode::Plan),
            context_packets: vec![crate::ContextPacket {
                id: Some("ctx-1".to_string()),
                source: crate::ContextSource::FocusThread,
                title: Some("focus".to_string()),
                content: "runtime context".to_string(),
                priority: 10,
                metadata: json!({}),
            }],
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
}

impl AgentRuntime {
    pub fn new(options: AgentRuntimeOptions) -> Self {
        Self {
            loaded_config: options.loaded_config,
            model_provider: options.model_provider,
            project_dir: options.project_dir,
            tool_executor: options.tool_executor,
            agent_mode: options.agent_mode,
            context_packets: options.context_packets,
            event_tx: options.event_tx,
            events: Vec::new(),
        }
    }

    pub fn events(&self) -> &[AgentEvent] {
        &self.events
    }

    pub fn agent_mode(&self) -> Option<AgentMode> {
        self.agent_mode
    }

    pub fn set_agent_mode(&mut self, mode: Option<AgentMode>) {
        self.agent_mode = mode;
    }

    pub fn add_context_packet(&mut self, packet: ContextPacket) {
        self.context_packets.push(packet);
    }

    pub fn clear_context_packets(&mut self) {
        self.context_packets.clear();
    }

    pub fn context_packets(&self) -> &[ContextPacket] {
        &self.context_packets
    }

    pub fn list_models(&self) -> Vec<ModelOption> {
        available_model_options(&self.loaded_config.config)
    }

    pub fn set_model(&mut self, provider: impl Into<String>, model: impl Into<String>) {
        self.loaded_config.config.model.provider =
            canonical_provider_id(&provider.into()).to_string();
        self.loaded_config.config.model.name = model.into();
    }

    pub fn register_host_tool(&mut self, tool: Arc<dyn Tool>) -> Result<()> {
        if self.tool_executor.is_none() {
            let security_policy = SecurityPolicy::new(
                self.project_dir.clone(),
                self.loaded_config.data_dir.clone(),
                self.loaded_config.config.security.clone(),
            )?;
            self.tool_executor = Some(Arc::new(ToolExecutor::new(security_policy)));
        }

        let Some(executor) = self.tool_executor.as_mut() else {
            return Err(anyhow::anyhow!("tool executor unavailable"));
        };
        let Some(executor) = Arc::get_mut(executor) else {
            return Err(anyhow::anyhow!(
                "cannot register host tool while tool executor is shared"
            ));
        };
        executor.register_tool(tool);
        Ok(())
    }

    pub async fn send_turn(&mut self, task: String) -> Result<ModelResponse> {
        self.submit_task(task).await
    }

    fn record_event(&mut self, event: AgentEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(event.clone());
        }
        self.events.push(event);
    }

    pub async fn submit_task(&mut self, task: String) -> Result<ModelResponse> {
        tracing::info!(
            project = %self.project_dir.display(),
            provider = %self.loaded_config.config.model.provider,
            model = %self.loaded_config.config.model.name,
            "agent task submitted"
        );
        self.record_event(AgentEvent::UserTaskSubmitted { text: task.clone() });

        let policy = select_harness_policy(&self.loaded_config.config);
        let tool_executor = match self.tool_executor.clone() {
            Some(executor) => executor,
            None => {
                let security_policy = SecurityPolicy::new(
                    self.project_dir.clone(),
                    self.loaded_config.data_dir.clone(),
                    self.loaded_config.config.security.clone(),
                )?;
                Arc::new(ToolExecutor::new(security_policy))
            }
        };

        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();

        let ctx = Arc::new(crate::turn::TurnContext {
            model_provider: self.model_provider.clone(),
            tool_executor,
            agent_control: crate::agent::AgentControl::new(),
            project_dir: self.project_dir.clone(),
            model_name: self.loaded_config.config.model.name.clone(),
            event_tx: Some(event_tx),
            pending_approvals: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            compact_state: Arc::new(tokio::sync::Mutex::new(crate::compact::CompactState::new(
                crate::config::effective_context_window(&self.loaded_config.config),
            ))),
            harness_config: self.loaded_config.config.harness.clone(),
            include_tool_prompt_manifest: crate::config::effective_tool_prompt_manifest(
                &self.loaded_config.config,
            ),
            agent_mode: self.agent_mode,
            context_packets: self.context_packets.clone(),
        });

        let session_runtime = crate::session::SessionRuntime::spawn(ctx, policy, Vec::new(), None);
        let (tx, rx) = tokio::sync::oneshot::channel();
        session_runtime
            .submission_tx
            .send(crate::session::Submission {
                task,
                response_tx: tx,
            })
            .map_err(|e| anyhow::anyhow!("failed to send submission: {}", e))?;

        let mut final_text = None;
        let mut rx_fut = rx;
        while final_text.is_none() {
            tokio::select! {
                res = &mut rx_fut => {
                    final_text = Some(res??);
                }
                Some(event) = event_rx.recv() => {
                    self.record_event(event);
                }
            }
        }

        while let Ok(event) = event_rx.try_recv() {
            self.record_event(event);
        }

        let response = ModelResponse {
            text: final_text.unwrap(),
        };
        tracing::info!(chars = response.text.len(), "agent task completed");
        Ok(response)
    }
}
