use crate::tool::{ToolDefinition, ToolInvocation};
use anyhow::Result;
use async_trait::async_trait;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn stream(&self, request: ModelRequest) -> ModelStream;

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse> {
        let mut stream = self.stream(request);
        let mut text = String::new();

        while let Some(event) = stream.next().await {
            match event? {
                ModelStreamEvent::TextDelta { text: delta } => text.push_str(&delta),
                ModelStreamEvent::Done => break,
                ModelStreamEvent::Status { .. }
                | ModelStreamEvent::Usage { .. }
                | ModelStreamEvent::ThinkingDelta { .. }
                | ModelStreamEvent::ToolCall(_) => {}
            }
        }

        Ok(ModelResponse { text })
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        anyhow::bail!("listing models is not supported by this provider")
    }
}

pub type ModelStream = BoxStream<'static, Result<ModelStreamEvent>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequest {
    pub model: String,
    pub messages: Vec<ModelMessage>,
    pub thinking: ThinkingConfig,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMessage {
    pub role: ModelRole,
    pub content: String,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolInvocation>,
    #[serde(default, skip_serializing, skip_deserializing)]
    pub created_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_content: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResponse {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ModelStreamEvent {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    Status {
        label: String,
    },
    Usage {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
    ToolCall(ToolInvocation),
    Done,
}

impl ModelMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::new(ModelRole::System, content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new(ModelRole::User, content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            thinking_content: None,
            ..Self::new(ModelRole::Assistant, content)
        }
    }

    pub fn assistant_with_thinking(content: impl Into<String>, thinking: Option<String>) -> Self {
        Self {
            thinking_content: thinking,
            ..Self::new(ModelRole::Assistant, content)
        }
    }

    pub fn tool_result(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: ModelRole::Tool,
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_name: Some(tool_name.into()),
            tool_calls: Vec::new(),
            created_at: Some(current_unix_millis()),
            thinking_content: None,
        }
    }

    pub fn assistant_tool_call(invocation: ToolInvocation) -> Self {
        Self::assistant_tool_call_with_context(invocation, String::new(), None)
    }

    pub fn assistant_tool_call_with_context(
        invocation: ToolInvocation,
        content: impl Into<String>,
        thinking: Option<String>,
    ) -> Self {
        Self {
            role: ModelRole::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_name: None,
            tool_calls: vec![invocation],
            created_at: Some(current_unix_millis()),
            thinking_content: thinking,
        }
    }

    fn new(role: ModelRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_call_id: None,
            tool_name: None,
            tool_calls: Vec::new(),
            created_at: Some(current_unix_millis()),
            thinking_content: None,
        }
    }
}

fn current_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingConfig {
    Max,
    High,
    Medium,
    Low,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinkingAdapter {
    OpenAiResponses(Value),
    OpenAiChatCompletions(&'static str),
    AnthropicOpenAiCompatible(Value),
    GeminiOpenAiCompatible(Value),
    OpenRouter(Value),
    Groq(&'static str),
    Unsupported,
}

impl ThinkingConfig {
    pub fn to_openai_effort(self) -> Option<&'static str> {
        match self {
            Self::Max | Self::High => Some("high"),
            Self::Medium => Some("medium"),
            Self::Low => Some("low"),
            Self::Off => None,
        }
    }

    pub fn to_openrouter_effort(self) -> &'static str {
        match self {
            Self::Max => "xhigh",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Off => "none",
        }
    }

    pub fn to_anthropic_thinking(self) -> Value {
        match self {
            Self::Max => json!({ "type": "enabled", "budget_tokens": 32000 }),
            Self::High => json!({ "type": "enabled", "budget_tokens": 10000 }),
            Self::Medium => json!({ "type": "enabled", "budget_tokens": 4096 }),
            Self::Low => json!({ "type": "enabled", "budget_tokens": 1024 }),
            Self::Off => json!({ "type": "disabled" }),
        }
    }

    pub fn to_gemini_thinking_config(self) -> Value {
        match self {
            Self::Max => json!({ "thinkingBudget": 24576 }),
            Self::High => json!({ "thinkingBudget": 8192 }),
            Self::Medium => json!({ "thinkingBudget": 4096 }),
            Self::Low => json!({ "thinkingBudget": 1024 }),
            Self::Off => json!({ "thinkingBudget": 0 }),
        }
    }

    pub fn adapter_for_provider(self, provider_id: &str) -> ThinkingAdapter {
        match provider_id {
            "openai" | "xai" => self
                .to_openai_effort()
                .map(|effort| ThinkingAdapter::OpenAiResponses(json!({ "effort": effort })))
                .unwrap_or(ThinkingAdapter::Unsupported),
            "anthropic" => ThinkingAdapter::AnthropicOpenAiCompatible(self.to_anthropic_thinking()),
            "google-gemini" => {
                ThinkingAdapter::GeminiOpenAiCompatible(self.to_gemini_thinking_config())
            }
            "openrouter" => ThinkingAdapter::OpenRouter(json!({
                "effort": self.to_openrouter_effort(),
                "exclude": true
            })),
            "groq" => match self {
                Self::Max | Self::High => ThinkingAdapter::Groq("high"),
                Self::Medium => ThinkingAdapter::Groq("medium"),
                Self::Low => ThinkingAdapter::Groq("low"),
                Self::Off => ThinkingAdapter::Groq("none"),
            },
            _ => self
                .to_openai_effort()
                .map(ThinkingAdapter::OpenAiChatCompletions)
                .unwrap_or(ThinkingAdapter::Unsupported),
        }
    }
}
