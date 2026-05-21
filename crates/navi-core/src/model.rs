use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequest {
    pub model: String,
    pub messages: Vec<ModelMessage>,
    pub thinking: ThinkingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMessage {
    pub role: ModelRole,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResponse {
    pub text: String,
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
