use crate::config::LoadedConfig;
use crate::event::AgentEvent;
use crate::model::{
    ModelMessage, ModelProvider, ModelRequest, ModelResponse, ModelRole, ThinkingConfig,
};
use anyhow::Result;
use std::sync::Arc;

pub struct AgentRuntimeOptions {
    pub loaded_config: LoadedConfig,
    pub model_provider: Arc<dyn ModelProvider>,
}

pub struct AgentRuntime {
    loaded_config: LoadedConfig,
    model_provider: Arc<dyn ModelProvider>,
    events: Vec<AgentEvent>,
}

impl AgentRuntime {
    pub fn new(options: AgentRuntimeOptions) -> Self {
        Self {
            loaded_config: options.loaded_config,
            model_provider: options.model_provider,
            events: Vec::new(),
        }
    }

    pub fn events(&self) -> &[AgentEvent] {
        &self.events
    }

    pub async fn submit_task(&mut self, task: String) -> Result<ModelResponse> {
        self.events
            .push(AgentEvent::UserTaskSubmitted { text: task.clone() });

        let request = ModelRequest {
            model: self.loaded_config.config.model.name.clone(),
            messages: vec![
                ModelMessage {
                    role: ModelRole::System,
                    content: default_system_prompt(),
                },
                ModelMessage {
                    role: ModelRole::User,
                    content: task,
                },
            ],
            thinking: ThinkingConfig::High,
        };

        let response = self.model_provider.complete(request).await?;
        self.events.push(AgentEvent::ModelOutput {
            text: response.text.clone(),
        });
        Ok(response)
    }
}

fn default_system_prompt() -> String {
    "You are NAVI, an autonomous code agent. Inspect before editing, propose diffs before writes, and request approval for commands or file mutations."
        .to_string()
}
