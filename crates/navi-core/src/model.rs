use crate::ProviderId;
use crate::tool::{ToolDefinition, ToolInvocation};
use anyhow::Result;
use async_trait::async_trait;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Trait for model provider backends that can stream and complete requests.
///
/// Implementors handle the wire protocol for a specific API (OpenAI, Anthropic,
/// Gemini, etc.) while the engine works with the generic [`ModelRequest`] and
/// [`ModelStreamEvent`] types.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Starts a streaming request and returns a stream of [`ModelStreamEvent`].
    fn stream(&self, request: ModelRequest) -> ModelStream;

    /// Completes a request by consuming the full stream and returning the
    /// accumulated text response. Default implementation calls [`Self::stream`].
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

    /// Lists available model identifiers from this provider.
    ///
    /// Returns an error if the provider does not support model listing.
    async fn list_models(&self) -> Result<Vec<String>> {
        anyhow::bail!("listing models is not supported by this provider")
    }
}

/// A boxed async stream of [`ModelStreamEvent`] results from a provider.
pub type ModelStream = BoxStream<'static, Result<ModelStreamEvent>>;

/// A request to a model provider containing the conversation, model name,
/// thinking configuration, and available tool definitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequest {
    /// The model identifier to use (e.g. `"gpt-5.5"`, `"claude-sonnet-4-20250514"`).
    pub model: String,
    /// The conversation messages to send to the model.
    pub messages: Vec<ModelMessage>,
    /// The thinking/reasoning effort level to request.
    pub thinking: ThinkingConfig,
    /// Tool definitions the model may invoke.
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
}

/// A single message in a model conversation.
///
/// Messages carry role, content, and optional tool-related metadata so the
/// same type can represent system prompts, user input, assistant responses,
/// tool calls, and tool results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMessage {
    /// The conversational role of this message.
    pub role: ModelRole,
    /// The text content of the message.
    pub content: String,
    /// For tool-result messages, the id of the tool call being answered.
    #[serde(default)]
    pub tool_call_id: Option<String>,
    /// For tool-result messages, the name of the tool that produced this result.
    #[serde(default)]
    pub tool_name: Option<String>,
    /// For assistant messages, the tool invocations requested by the model.
    #[serde(default)]
    pub tool_calls: Vec<ToolInvocation>,
    /// Creation timestamp in milliseconds since Unix epoch (not serialized).
    #[serde(default, skip_serializing, skip_deserializing)]
    pub created_at: Option<u64>,
    /// Optional thinking/reasoning content from the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_content: Option<String>,
}

/// The conversational role of a [`ModelMessage`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelRole {
    /// System-level instructions (system prompt).
    System,
    /// End-user input.
    User,
    /// Model-generated response.
    Assistant,
    /// A tool result returned to the model.
    Tool,
}

/// A completed model response containing the full text output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResponse {
    /// The full text content of the model's response.
    pub text: String,
}

/// A single event from a model provider's streaming response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ModelStreamEvent {
    /// An incremental text delta from the assistant.
    TextDelta {
        /// The incremental text content.
        text: String,
    },
    /// An incremental thinking/reasoning delta.
    ThinkingDelta {
        /// The incremental thinking content.
        text: String,
    },
    /// A status message from the provider (e.g. "processing", "queued").
    Status {
        /// Human-readable status label.
        label: String,
    },
    /// Token usage information reported by the provider.
    Usage {
        /// Number of input/prompt tokens consumed, if reported.
        input_tokens: Option<u64>,
        /// Number of output/completion tokens produced, if reported.
        output_tokens: Option<u64>,
    },
    /// The model requested a tool invocation.
    ToolCall(ToolInvocation),
    /// The stream has ended.
    Done,
}

impl ModelMessage {
    /// Creates a system-role message.
    pub fn system(content: impl Into<String>) -> Self {
        Self::new(ModelRole::System, content)
    }

    /// Creates a user-role message.
    pub fn user(content: impl Into<String>) -> Self {
        Self::new(ModelRole::User, content)
    }

    /// Creates an assistant-role message without thinking content.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            thinking_content: None,
            ..Self::new(ModelRole::Assistant, content)
        }
    }

    /// Creates an assistant-role message with optional thinking content.
    pub fn assistant_with_thinking(content: impl Into<String>, thinking: Option<String>) -> Self {
        Self {
            thinking_content: thinking,
            ..Self::new(ModelRole::Assistant, content)
        }
    }

    /// Creates a tool-result message responding to a specific tool call.
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

    /// Creates an assistant message that requests a single tool invocation.
    pub fn assistant_tool_call(invocation: ToolInvocation) -> Self {
        Self::assistant_tool_call_with_context(invocation, String::new(), None)
    }

    /// Creates an assistant message that requests a tool invocation with
    /// accompanying text content and optional thinking.
    pub fn assistant_tool_call_with_context(
        invocation: ToolInvocation,
        content: impl Into<String>,
        thinking: Option<String>,
    ) -> Self {
        Self::assistant_tool_calls_with_context(vec![invocation], content, thinking)
    }

    pub fn assistant_tool_calls_with_context(
        invocations: Vec<ToolInvocation>,
        content: impl Into<String>,
        thinking: Option<String>,
    ) -> Self {
        Self {
            role: ModelRole::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_name: None,
            tool_calls: invocations,
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

/// The thinking/reasoning effort level requested from the model.
///
/// Maps to provider-specific parameters via [`ThinkingConfig::adapter_for_provider`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingConfig {
    /// Maximum reasoning effort.
    Max,
    /// High reasoning effort.
    High,
    /// Medium reasoning effort.
    Medium,
    /// Low reasoning effort.
    Low,
    /// Thinking/reasoning disabled.
    Off,
}

/// Provider-specific thinking configuration produced by
/// [`ThinkingConfig::adapter_for_provider`].
///
/// Each variant carries the JSON value or string needed by the corresponding
/// provider's request format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinkingAdapter {
    /// OpenAI Responses API reasoning effort object.
    OpenAiResponses(Value),
    /// OpenAI Chat Completions reasoning effort string.
    OpenAiChatCompletions(&'static str),
    /// Anthropic thinking configuration in OpenAI-compatible format.
    AnthropicOpenAiCompatible(Value),
    /// Gemini thinking budget configuration in OpenAI-compatible format.
    GeminiOpenAiCompatible(Value),
    /// OpenRouter reasoning effort and exclusion config.
    OpenRouter(Value),
    /// Groq reasoning effort string.
    Groq(&'static str),
    /// Thinking is not supported for this provider.
    Unsupported,
}

impl ThinkingConfig {
    /// Maps this config to an OpenAI Responses API effort level string.
    ///
    /// Returns `None` for `Off`.
    pub fn to_openai_effort(self) -> Option<&'static str> {
        match self {
            Self::Max | Self::High => Some("high"),
            Self::Medium => Some("medium"),
            Self::Low => Some("low"),
            Self::Off => None,
        }
    }

    /// Maps this config to an OpenRouter reasoning effort string.
    pub fn to_openrouter_effort(self) -> &'static str {
        match self {
            Self::Max => "xhigh",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Off => "none",
        }
    }

    /// Maps this config to an Anthropic thinking configuration JSON value.
    pub fn to_anthropic_thinking(self) -> Value {
        match self {
            Self::Max => json!({ "type": "enabled", "budget_tokens": 32000 }),
            Self::High => json!({ "type": "enabled", "budget_tokens": 10000 }),
            Self::Medium => json!({ "type": "enabled", "budget_tokens": 4096 }),
            Self::Low => json!({ "type": "enabled", "budget_tokens": 1024 }),
            Self::Off => json!({ "type": "disabled" }),
        }
    }

    /// Maps this config to a Gemini thinking budget configuration JSON value.
    pub fn to_gemini_thinking_config(self) -> Value {
        match self {
            Self::Max => json!({ "thinkingBudget": 24576 }),
            Self::High => json!({ "thinkingBudget": 8192 }),
            Self::Medium => json!({ "thinkingBudget": 4096 }),
            Self::Low => json!({ "thinkingBudget": 1024 }),
            Self::Off => json!({ "thinkingBudget": 0 }),
        }
    }

    /// Produces a provider-specific [`ThinkingAdapter`] for the given provider id.
    ///
    /// Provider ids with known adapters include `openai`, `xai`, `anthropic`,
    /// `google-gemini`, `openrouter`, and `groq`. Unknown providers fall back
    /// to the OpenAI Chat Completions format.
    pub fn adapter_for_provider(self, provider_id: &str) -> ThinkingAdapter {
        match ProviderId::from_config_id(provider_id) {
            ProviderId::OpenAi | ProviderId::Xai => self
                .to_openai_effort()
                .map(|effort| ThinkingAdapter::OpenAiResponses(json!({ "effort": effort })))
                .unwrap_or(ThinkingAdapter::Unsupported),
            ProviderId::Anthropic => {
                ThinkingAdapter::AnthropicOpenAiCompatible(self.to_anthropic_thinking())
            }
            ProviderId::GoogleGemini => {
                ThinkingAdapter::GeminiOpenAiCompatible(self.to_gemini_thinking_config())
            }
            ProviderId::OpenRouter => ThinkingAdapter::OpenRouter(json!({
                "effort": self.to_openrouter_effort(),
                "exclude": true
            })),
            ProviderId::Groq => match self {
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
