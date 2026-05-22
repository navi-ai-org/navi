use crate::config::LoadedConfig;
use crate::event::AgentEvent;
use crate::harness::{
    AgentRunState, ToolLoopDecision, build_system_prompt, compact_tool_observation,
    record_tool_call, select_harness_policy, tool_error_result, trace_request_summary,
};
use crate::model::{ModelMessage, ModelProvider, ModelRequest, ModelResponse, ThinkingConfig};
use crate::security::{SecurityDecision, SecurityPolicy};
use crate::tool::ToolExecutor;
use anyhow::Result;
use futures_util::StreamExt;
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
        ApprovalConfig, HarnessConfig, ModelStream, NaviConfig, SecurityConfig, ToolInvocation,
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
        let mut run_state = AgentRunState::default();
        let mut messages = vec![
            ModelMessage::system(build_system_prompt(
                &self.loaded_config.config,
                &self.project_dir,
            )),
            ModelMessage::user(task),
        ];
        let final_text = loop {
            let request = ModelRequest {
                model: self.loaded_config.config.model.name.clone(),
                messages: messages.clone(),
                thinking: ThinkingConfig::High,
                tools: tool_executor.definitions(),
            };
            self.events
                .push(AgentEvent::HarnessTrace(trace_request_summary(
                    &request, policy,
                )));

            tracing::info!(
                model = %request.model,
                messages = request.messages.len(),
                tools = request.tools.len(),
                "model request started"
            );
            let mut stream = self.model_provider.stream(request);
            let mut text = String::new();
            let mut tool_call = None;
            while let Some(event) = stream.next().await {
                match event? {
                    crate::model::ModelStreamEvent::TextDelta { text: delta } => {
                        text.push_str(&delta)
                    }
                    crate::model::ModelStreamEvent::ToolCall(invocation) => {
                        tracing::info!(
                            tool = %invocation.tool_name,
                            invocation_id = %invocation.id,
                            "model requested tool"
                        );
                        tool_call = Some(invocation);
                        break;
                    }
                    crate::model::ModelStreamEvent::Done => break,
                    crate::model::ModelStreamEvent::Status { .. }
                    | crate::model::ModelStreamEvent::Usage { .. }
                    | crate::model::ModelStreamEvent::ThinkingDelta { .. } => {}
                }
            }

            if let Some(invocation) = tool_call {
                if !text.trim().is_empty() {
                    messages.push(ModelMessage::assistant(text.clone()));
                }
                messages.push(ModelMessage::assistant_tool_call(invocation.clone()));
                self.events
                    .push(AgentEvent::ToolRequested(invocation.clone()));

                let result = match record_tool_call(&mut run_state, policy, &invocation) {
                    ToolLoopDecision::Continue => match tool_executor.validate(&invocation) {
                        SecurityDecision::Allow => tool_executor.invoke(invocation.clone()).await,
                        SecurityDecision::NeedsApproval(_) => tool_error_result(
                            &invocation,
                            "approval required in headless mode; rerun in TUI or enable an explicit approval policy",
                        ),
                        SecurityDecision::Deny(reason) => tool_error_result(&invocation, reason),
                    },
                    ToolLoopDecision::RepeatedCall(reason) => {
                        tool_error_result(&invocation, reason)
                    }
                };

                tracing::info!(
                    tool = %invocation.tool_name,
                    invocation_id = %invocation.id,
                    ok = result.ok,
                    "tool completed"
                );
                self.events.push(AgentEvent::ToolCompleted(result.clone()));
                let observation = compact_tool_observation(&invocation, &result, policy);
                messages.push(ModelMessage::tool_result(
                    invocation.id,
                    invocation.tool_name,
                    observation,
                ));
                continue;
            }

            break text;
        };

        let response = ModelResponse { text: final_text };
        tracing::info!(chars = response.text.len(), "agent task completed");
        self.events.push(AgentEvent::ModelOutput {
            text: response.text.clone(),
            thinking: None,
        });
        Ok(response)
    }
}
