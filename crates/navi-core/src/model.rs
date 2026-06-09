use crate::tool::{ToolDefinition, ToolInvocation};
use anyhow::Result;
use async_trait::async_trait;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use serde::{Deserialize, Serialize};

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
        /// Number of tokens written to the prompt cache (Anthropic).
        cache_creation_tokens: Option<u64>,
        /// Number of tokens read from the prompt cache (Anthropic).
        cache_read_tokens: Option<u64>,
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
/// Maps to provider-specific parameters via [`ThinkingConfig::to_thinking_request`].
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
    /// Adaptive: automatically selects effort based on task complexity.
    /// Simple tasks (read, grep) → Low. Complex tasks (refactor, multi-file) → High.
    Adaptive,
}

/// Normalized thinking/reasoning request produced by [`ThinkingConfig::to_thinking_request`].
///
/// This is a provider-agnostic representation. Each provider converts these
/// fields into its own wire format in the stream layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingRequest {
    /// Whether thinking/reasoning is enabled.
    pub enabled: bool,
    /// Reasoning effort level for providers that use effort strings
    /// (OpenAI, OpenRouter, Groq, etc.).
    pub effort: Option<&'static str>,
    /// Token budget for providers that use budget-based thinking
    /// (Anthropic, Gemini).
    pub budget_tokens: Option<u32>,
}

impl ThinkingConfig {
    /// Produces a normalized [`ThinkingRequest`] from this config.
    ///
    /// The caller (provider stream layer) converts the normalized fields
    /// into the provider-specific wire format.
    pub fn to_thinking_request(self) -> ThinkingRequest {
        match self {
            Self::Max => ThinkingRequest {
                enabled: true,
                effort: Some("high"),
                budget_tokens: Some(32000),
            },
            Self::High => ThinkingRequest {
                enabled: true,
                effort: Some("high"),
                budget_tokens: Some(10000),
            },
            Self::Medium => ThinkingRequest {
                enabled: true,
                effort: Some("medium"),
                budget_tokens: Some(4096),
            },
            Self::Low => ThinkingRequest {
                enabled: true,
                effort: Some("low"),
                budget_tokens: Some(1024),
            },
            Self::Off => ThinkingRequest {
                enabled: false,
                effort: None,
                budget_tokens: None,
            },
            // Adaptive should be resolved before calling to_thinking_request.
            // Fallback to Medium if called directly.
            Self::Adaptive => Self::Medium.to_thinking_request(),
        }
    }

    /// Resolve an adaptive thinking level based on task context heuristics.
    ///
    /// Returns a concrete `ThinkingConfig` (never `Adaptive`).
    pub fn resolve_adaptive(
        messages: &[ModelMessage],
        tool_names: &[String],
        _iteration: usize,
    ) -> Self {
        let complexity = TaskComplexity::classify(messages, tool_names);
        match complexity {
            TaskComplexity::Simple => Self::Low,
            TaskComplexity::Medium => Self::Medium,
            TaskComplexity::Complex => Self::High,
        }
    }
}

/// Task complexity classification for adaptive thinking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskComplexity {
    Simple,
    Medium,
    Complex,
}

impl TaskComplexity {
    fn classify(messages: &[ModelMessage], tool_names: &[String]) -> Self {
        let mut score: u32 = 0;

        // More messages → more complex conversation context.
        let msg_count = messages.len() as u32;
        if msg_count > 20 {
            score += 2;
        } else if msg_count > 8 {
            score += 1;
        }

        // Check recent tool calls for complexity signals.
        let has_write_tools = tool_names
            .iter()
            .any(|t| matches!(t.as_str(), "write_file" | "apply_patch"));
        let has_complex_tools = tool_names
            .iter()
            .any(|t| matches!(t.as_str(), "bash" | "test_runner" | "build_runner"));
        let has_read_only = tool_names
            .iter()
            .any(|t| matches!(t.as_str(), "read_file" | "grep" | "fs_browser" | "git_ops"));

        if has_write_tools {
            score += 2;
        }
        if has_complex_tools {
            score += 1;
        }
        // If only read-only tools and nothing else → simpler.
        if has_read_only && !has_write_tools && !has_complex_tools {
            score = score.saturating_sub(1);
        }

        // Check for error patterns in recent messages (retries = complex).
        let recent_errors = messages
            .iter()
            .rev()
            .take(6)
            .filter(|m| {
                m.role == ModelRole::Tool
                    && m.content.contains("\"error\"")
                    && !m.content.contains("[Old tool result content cleared]")
            })
            .count();
        if recent_errors > 1 {
            score += 1;
        }

        // User messages with long content → likely complex instructions.
        let last_user = messages.iter().rev().find(|m| m.role == ModelRole::User);
        if let Some(user_msg) = last_user
            && user_msg.content.len() > 500
        {
            score += 1;
        }

        match score {
            0..=1 => Self::Simple,
            2..=3 => Self::Medium,
            _ => Self::Complex,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Regression: ThinkingConfig to ThinkingRequest ──────────────────────────

    #[test]
    fn regression_thinking_request_high_produces_effort_and_budget() {
        let request = ThinkingConfig::High.to_thinking_request();
        assert!(request.enabled);
        assert_eq!(request.effort, Some("high"));
        assert_eq!(request.budget_tokens, Some(10000));
    }

    #[test]
    fn regression_thinking_request_max_produces_effort_and_budget() {
        let request = ThinkingConfig::Max.to_thinking_request();
        assert!(request.enabled);
        assert_eq!(request.effort, Some("high"));
        assert_eq!(request.budget_tokens, Some(32000));
    }

    #[test]
    fn regression_thinking_request_off_produces_disabled() {
        let request = ThinkingConfig::Off.to_thinking_request();
        assert!(!request.enabled);
        assert!(request.effort.is_none());
        assert!(request.budget_tokens.is_none());
    }

    #[test]
    fn regression_thinking_request_medium_produces_medium_effort() {
        let request = ThinkingConfig::Medium.to_thinking_request();
        assert!(request.enabled);
        assert_eq!(request.effort, Some("medium"));
        assert_eq!(request.budget_tokens, Some(4096));
    }

    #[test]
    fn regression_thinking_request_low_produces_low_effort() {
        let request = ThinkingConfig::Low.to_thinking_request();
        assert!(request.enabled);
        assert_eq!(request.effort, Some("low"));
        assert_eq!(request.budget_tokens, Some(1024));
    }

    // ── Regression: ModelMessage constructors ─────────────────────────────────

    #[test]
    fn regression_system_message_has_correct_role() {
        let msg = ModelMessage::system("test".to_string());
        assert_eq!(msg.role, ModelRole::System);
        assert_eq!(msg.content, "test");
    }

    #[test]
    fn regression_user_message_has_correct_role() {
        let msg = ModelMessage::user("hello".to_string());
        assert_eq!(msg.role, ModelRole::User);
        assert_eq!(msg.content, "hello");
    }

    #[test]
    fn regression_assistant_message_has_correct_role() {
        let msg = ModelMessage::assistant("response".to_string());
        assert_eq!(msg.role, ModelRole::Assistant);
        assert_eq!(msg.content, "response");
    }

    #[test]
    fn regression_tool_result_sets_call_id_and_name() {
        let msg = ModelMessage::tool_result("call-1", "read_file", "content");
        assert_eq!(msg.role, ModelRole::Tool);
        assert_eq!(msg.tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(msg.tool_name.as_deref(), Some("read_file"));
        assert_eq!(msg.content, "content");
    }

    #[test]
    fn regression_assistant_tool_call_with_context_sets_fields() {
        let inv = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({"path": "test.rs"}),
        };
        let msg = ModelMessage::assistant_tool_call_with_context(
            inv,
            "thinking text",
            Some("reasoning".to_string()),
        );
        assert_eq!(msg.role, ModelRole::Assistant);
        assert_eq!(msg.content, "thinking text");
        assert_eq!(msg.thinking_content.as_deref(), Some("reasoning"));
        assert_eq!(msg.tool_calls.len(), 1);
        assert_eq!(msg.tool_calls[0].id, "call-1");
    }

    // ── Regression: ModelMessage serialization roundtrip ──────────────────────

    #[test]
    fn regression_model_message_serialization_roundtrip() {
        let msg = ModelMessage {
            role: ModelRole::Assistant,
            content: "hello".to_string(),
            tool_call_id: None,
            tool_name: None,
            tool_calls: vec![],
            thinking_content: Some("thinking".to_string()),
            created_at: Some(12345),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: ModelMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.role, msg.role);
        assert_eq!(deserialized.content, msg.content);
        assert_eq!(deserialized.thinking_content, msg.thinking_content);
        // created_at is intentionally not serialized (runtime-only field)
        assert!(deserialized.created_at.is_none());
    }

    // ── Adaptive thinking ──────────────────────────────────────────────────────

    #[test]
    fn adaptive_thinking_simple_read_only_gives_low() {
        let messages = vec![
            ModelMessage::user("read this file"),
            ModelMessage::assistant("ok"),
        ];
        let tools = vec!["read_file".to_string()];
        let result = ThinkingConfig::resolve_adaptive(&messages, &tools, 0);
        assert_eq!(result, ThinkingConfig::Low);
    }

    #[test]
    fn adaptive_thinking_write_tools_gives_medium_or_high() {
        let messages = vec![
            ModelMessage::user("refactor this module"),
            ModelMessage::assistant("ok"),
        ];
        let tools = vec!["write_file".to_string(), "read_file".to_string()];
        let result = ThinkingConfig::resolve_adaptive(&messages, &tools, 0);
        assert!(matches!(
            result,
            ThinkingConfig::Medium | ThinkingConfig::High
        ));
    }

    #[test]
    fn adaptive_thinking_long_conversation_gives_higher() {
        let mut messages = Vec::new();
        for i in 0..25 {
            messages.push(ModelMessage::user(&format!("message {i}")));
            messages.push(ModelMessage::assistant(&format!("response {i}")));
        }
        let tools = vec!["bash".to_string(), "write_file".to_string()];
        let result = ThinkingConfig::resolve_adaptive(&messages, &tools, 0);
        assert!(matches!(
            result,
            ThinkingConfig::Medium | ThinkingConfig::High
        ));
    }

    #[test]
    fn adaptive_thinking_errors_increase_complexity() {
        let messages = vec![
            ModelMessage::user("fix this"),
            {
                let mut m = ModelMessage::tool_result("c1", "bash", "{\"error\": \"failed\"}");
                m
            },
            {
                let mut m =
                    ModelMessage::tool_result("c2", "bash", "{\"error\": \"failed again\"}");
                m
            },
        ];
        let tools = vec!["bash".to_string()];
        let result = ThinkingConfig::resolve_adaptive(&messages, &tools, 0);
        assert!(matches!(
            result,
            ThinkingConfig::Medium | ThinkingConfig::High
        ));
    }

    #[test]
    fn adaptive_falls_back_to_medium_in_to_thinking_request() {
        let request = ThinkingConfig::Adaptive.to_thinking_request();
        assert!(request.enabled);
        assert_eq!(request.effort, Some("medium"));
    }

    #[test]
    fn adaptive_serde_roundtrip() {
        let config = ThinkingConfig::Adaptive;
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(json, "\"adaptive\"");
        let deserialized: ThinkingConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, ThinkingConfig::Adaptive);
    }
}
