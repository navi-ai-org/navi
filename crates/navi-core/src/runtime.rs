use crate::config::LoadedConfig;
use crate::event::AgentEvent;
use crate::harness::select_harness_policy;
use crate::model::{ModelProvider, ModelResponse};
use crate::security::SecurityPolicy;
use crate::tool::ToolExecutor;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

pub struct AgentRuntimeOptions {
    pub loaded_config: LoadedConfig,
    pub model_provider: Arc<dyn ModelProvider>,
    pub project_dir: PathBuf,
    pub tool_executor: Option<Arc<ToolExecutor>>,
}

pub struct AgentRuntime {
    loaded_config: LoadedConfig,
    model_provider: Arc<dyn ModelProvider>,
    project_dir: PathBuf,
    tool_executor: Option<Arc<ToolExecutor>>,
    events: Vec<AgentEvent>,
}

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
            events: Vec::new(),
        }
    }

    pub fn events(&self) -> &[AgentEvent] {
        &self.events
    }

    pub async fn submit_task(&mut self, task: String) -> Result<ModelResponse> {
        tracing::info!(
            project = %self.project_dir.display(),
            provider = %self.loaded_config.config.model.provider,
            model = %self.loaded_config.config.model.name,
            "agent task submitted"
        );
        self.events
            .push(AgentEvent::UserTaskSubmitted { text: task.clone() });

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
        });

        let session_runtime = crate::session::SessionRuntime::spawn(ctx, policy, Vec::new());
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
                    self.events.push(event);
                }
            }
        }

        while let Ok(event) = event_rx.try_recv() {
            self.events.push(event);
        }

        let response = ModelResponse {
            text: final_text.unwrap(),
        };
        tracing::info!(chars = response.text.len(), "agent task completed");
        Ok(response)
    }
}
