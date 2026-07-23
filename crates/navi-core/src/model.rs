use crate::tool::{ToolDefinition, ToolInvocation};
use anyhow::Result;
use async_trait::async_trait;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use serde::{Deserialize, Serialize};

/// A single part of a multimodal message content.
///
/// Models like GPT-4o, Claude, and Gemini accept messages with mixed
/// text and attachment parts. When a [`ModelMessage`] contains non-empty
/// [`ModelMessage::content_parts`], providers serialize each part
/// according to their native wire format instead of using the plain
/// [`ModelMessage::content`] string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    /// A plain text content block.
    Text {
        /// The text content.
        text: String,
    },
    /// An inline image (base64-encoded).
    Image {
        /// MIME type of the image (e.g. `"image/png"`, `"image/jpeg"`).
        media_type: String,
        /// Base64-encoded image data (no data-URL prefix, raw base64 only).
        data: String,
    },
    /// An inline audio attachment (base64-encoded).
    Audio {
        /// MIME type of the audio (e.g. `"audio/mpeg"`, `"audio/wav"`).
        media_type: String,
        /// Base64-encoded audio data (no data-URL prefix, raw base64 only).
        data: String,
        /// Optional filename or user-facing label.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// An inline video attachment (base64-encoded).
    Video {
        /// MIME type of the video (e.g. `"video/mp4"`).
        media_type: String,
        /// Base64-encoded video data (no data-URL prefix, raw base64 only).
        data: String,
        /// Optional filename or user-facing label.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// An inline document attachment (base64-encoded).
    Document {
        /// MIME type of the document (e.g. `"application/pdf"`, `"text/plain"`).
        media_type: String,
        /// Base64-encoded document data (no data-URL prefix, raw base64 only).
        data: String,
        /// Optional filename or user-facing label.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

impl ContentPart {
    /// Returns `true` if this is a text part.
    pub fn is_text(&self) -> bool {
        matches!(self, Self::Text { .. })
    }

    /// Returns `true` if this is an image part.
    pub fn is_image(&self) -> bool {
        matches!(self, Self::Image { .. })
    }

    /// Returns `true` if this is an audio part.
    pub fn is_audio(&self) -> bool {
        matches!(self, Self::Audio { .. })
    }

    /// Returns `true` if this is a video part.
    pub fn is_video(&self) -> bool {
        matches!(self, Self::Video { .. })
    }

    /// Returns `true` if this is a document part.
    pub fn is_document(&self) -> bool {
        matches!(self, Self::Document { .. })
    }

    /// Returns the attachment kind, if this part is an attachment.
    pub fn attachment_kind(&self) -> Option<AttachmentKind> {
        match self {
            Self::Text { .. } => None,
            Self::Image { .. } => Some(AttachmentKind::Image),
            Self::Audio { .. } => Some(AttachmentKind::Audio),
            Self::Video { .. } => Some(AttachmentKind::Video),
            Self::Document { .. } => Some(AttachmentKind::Document),
        }
    }

    /// Returns the MIME type for attachment parts.
    pub fn media_type(&self) -> Option<&str> {
        match self {
            Self::Text { .. } => None,
            Self::Image { media_type, .. }
            | Self::Audio { media_type, .. }
            | Self::Video { media_type, .. }
            | Self::Document { media_type, .. } => Some(media_type),
        }
    }

    /// Returns base64 data for attachment parts.
    pub fn data(&self) -> Option<&str> {
        match self {
            Self::Text { .. } => None,
            Self::Image { data, .. }
            | Self::Audio { data, .. }
            | Self::Video { data, .. }
            | Self::Document { data, .. } => Some(data),
        }
    }

    /// Optional filename/label for audio, video, or document attachments.
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Audio { name, .. } | Self::Video { name, .. } | Self::Document { name, .. } => {
                name.as_deref()
            }
            Self::Text { .. } | Self::Image { .. } => None,
        }
    }

    /// Extracts the text content if this is a text part.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }

    /// Returns all text content from a slice of parts, concatenated.
    pub fn text_from_parts(parts: &[ContentPart]) -> String {
        parts
            .iter()
            .filter_map(|p| p.as_text())
            .collect::<Vec<_>>()
            .join("")
    }
}

/// Attachment modalities NAVI can route to specialized models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Image,
    Audio,
    Video,
    Document,
}

impl AttachmentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Document => "document",
        }
    }
}

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
                | ModelStreamEvent::ToolCall(_)
                | ModelStreamEvent::ToolCallProgress { .. } => {}
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
    /// Stable base instructions sent in the provider's `instructions` field
    /// (Responses API) or as the first system message (Chat Completions,
    /// Anthropic, Gemini). Kept separate from [`Self::messages`] so that
    /// dynamic context blocks (developer messages) don't invalidate the
    /// provider's prompt cache for this prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// The conversation messages to send to the model.
    pub messages: Vec<ModelMessage>,
    /// The thinking/reasoning effort level to request.
    pub thinking: ThinkingConfig,
    /// Tool definitions the model may invoke.
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    /// Stable session id for provider-side prompt-cache affinity.
    ///
    /// Providers such as Charm Hyper use this to set `x-session-id` and
    /// `x-session-affinity` so consecutive turns of the same agent session
    /// hit the same KV-cache shard. Not serialized to disk/transcripts.
    #[serde(default, skip_serializing, skip_deserializing)]
    pub session_id: Option<String>,
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
    /// Multimodal content parts (text + images).
    ///
    /// When non-empty, providers use these parts instead of the plain
    /// [`content`](Self::content) field to build the wire-format message.
    /// This allows attaching images alongside text in user messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_parts: Vec<ContentPart>,
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
    /// Developer-level instructions (context blocks injected separately from
    /// the base system prompt for provider cache efficiency).
    Developer,
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
    /// The model is streaming a tool call (name known; arguments may still be incomplete).
    ///
    /// Providers emit this while native tool-call arguments are being generated
    /// so clients can show progress instead of a false "waiting for model" idle.
    ToolCallProgress {
        /// Provider tool-call id when known.
        id: Option<String>,
        /// Tool name when known (empty only until the first name chunk arrives).
        tool_name: String,
        /// Total characters of arguments streamed so far for this call.
        arguments_chars: usize,
    },
    /// The stream has ended.
    Done,
}

impl ModelMessage {
    /// Creates a system-role message.
    pub fn system(content: impl Into<String>) -> Self {
        Self::new(ModelRole::System, content)
    }

    /// Creates a developer-role message (context block injected separately
    /// from the base system prompt for provider cache efficiency).
    pub fn developer(content: impl Into<String>) -> Self {
        Self::new(ModelRole::Developer, content)
    }

    /// Creates a user-role message.
    pub fn user(content: impl Into<String>) -> Self {
        Self::new(ModelRole::User, content)
    }

    /// Creates a user-role message with text and optional image attachments.
    pub fn user_multimodal(content: impl Into<String>, parts: Vec<ContentPart>) -> Self {
        Self {
            role: ModelRole::User,
            content: content.into(),
            content_parts: parts,
            tool_call_id: None,
            tool_name: None,
            tool_calls: Vec::new(),
            created_at: Some(current_unix_millis()),
            thinking_content: None,
        }
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
        Self::tool_result_with_parts(tool_call_id, tool_name, content, Vec::new())
    }

    /// Creates a tool-result message with optional multimodal content parts
    /// (e.g. images from `view_image` for vision-capable models).
    pub fn tool_result_with_parts(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: impl Into<String>,
        content_parts: Vec<ContentPart>,
    ) -> Self {
        Self {
            role: ModelRole::Tool,
            content: content.into(),
            content_parts,
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
            content_parts: Vec::new(),
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
            content_parts: Vec::new(),
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
///
/// Effort is fixed for a session preference (no adaptive re-scoring). Registry
/// `reasoning_levels` drive the picker; models without levels get binary
/// thinking on/off. Models that do not support reasoning are forced to [`Off`].
/// Default preference is [`Max`] (highest available effort).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingConfig {
    /// Maximum reasoning effort (default).
    ///
    /// Legacy config/session values `"adaptive"` / `"auto"` deserialize as Max.
    #[serde(alias = "adaptive", alias = "auto")]
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

/// Normalized thinking/reasoning request produced by [`ThinkingConfig::to_thinking_request`].
///
/// This is a provider-agnostic representation. Each provider converts these
/// fields into its own wire format in the stream layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingRequest {
    /// Whether thinking/reasoning is enabled.
    pub enabled: bool,
    /// Reasoning effort level for providers that use effort strings
    /// (OpenAI, OpenRouter, Groq, etc.). Owned so registry labels
    /// (e.g. `xhigh`, `minimal`) can pass through unchanged.
    pub effort: Option<String>,
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
                // Prefer xhigh on wire when providers accept it; callers may
                // remap via [`resolve_effort_label`] using registry levels.
                effort: Some("xhigh".to_string()),
                budget_tokens: Some(32000),
            },
            Self::High => ThinkingRequest {
                enabled: true,
                effort: Some("high".to_string()),
                budget_tokens: Some(10000),
            },
            Self::Medium => ThinkingRequest {
                enabled: true,
                effort: Some("medium".to_string()),
                budget_tokens: Some(4096),
            },
            Self::Low => ThinkingRequest {
                enabled: true,
                effort: Some("low".to_string()),
                budget_tokens: Some(1024),
            },
            Self::Off => ThinkingRequest {
                enabled: false,
                effort: None,
                budget_tokens: None,
            },
        }
    }

    /// Config key used in `tui.thinking_level` / UI labels.
    pub fn as_config_str(self) -> &'static str {
        match self {
            Self::Max => "max",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Off => "off",
        }
    }

    /// Parse a config / registry effort string into a [`ThinkingConfig`].
    ///
    /// Unknown values (including legacy `"adaptive"`) fall back to [`Max`].
    pub fn from_config_str(value: &str) -> Self {
        parse_reasoning_level(value).unwrap_or(Self::Max)
    }

    /// Clamp this level to one supported by the model (registry reasoning_levels).
    ///
    /// Prefers the highest remaining effort; `Off` is last. Empty `supported`
    /// leaves the value unchanged.
    pub fn clamp_to_supported(self, supported: &[ThinkingConfig]) -> Self {
        if supported.is_empty() || supported.contains(&self) {
            return self;
        }
        // Prefer maximum effort when the requested level is unavailable.
        for candidate in [Self::Max, Self::High, Self::Medium, Self::Low, Self::Off] {
            if supported.contains(&candidate) {
                return candidate;
            }
        }
        supported[0]
    }
}

/// Parse a registry / config reasoning level string.
///
/// Accepts common aliases used across OpenAI, OpenRouter, Anthropic, and xAI.
/// `"on"` / `"enabled"` / `"true"` map to [`ThinkingConfig::Medium`] (binary
/// effort "thinking on").
pub fn parse_reasoning_level(raw: &str) -> Option<ThinkingConfig> {
    match raw.trim().to_ascii_lowercase().as_str() {
        // Legacy adaptive/auto maps to Max (highest fixed effort).
        "adaptive" | "auto" | "max" | "xhigh" | "x-high" | "ultra" | "highest" => {
            Some(ThinkingConfig::Max)
        }
        "high" => Some(ThinkingConfig::High),
        // Binary "thinking on" uses Max so the default highest effort is stable.
        "medium" | "med" | "mid" | "default" => Some(ThinkingConfig::Medium),
        "on" | "enabled" | "true" | "1" => Some(ThinkingConfig::Max),
        "low" | "minimal" | "min" => Some(ThinkingConfig::Low),
        "off" | "none" | "disabled" | "false" | "0" => Some(ThinkingConfig::Off),
        _ => None,
    }
}

/// User-facing effort label for a level.
///
/// In binary mode (no registry levels) non-off levels display as
/// `"thinking on"` and off as `"thinking off"`.
pub fn effort_display_label(level: ThinkingConfig, binary: bool) -> &'static str {
    if binary {
        match level {
            ThinkingConfig::Off => "thinking off",
            _ => "thinking on",
        }
    } else {
        level.as_config_str()
    }
}

/// Canonical sort order for effort levels in pickers (most → least / off last).
pub const DEFAULT_REASONING_LEVELS: &[ThinkingConfig] = &[
    ThinkingConfig::Max,
    ThinkingConfig::High,
    ThinkingConfig::Medium,
    ThinkingConfig::Low,
    ThinkingConfig::Off,
];

/// Binary effort options when a model has no registry `reasoning_levels`.
///
/// UI presents these as "thinking on" / "thinking off". Internally "on" is
/// [`ThinkingConfig::Max`] so the highest fixed effort is the default.
pub const BINARY_REASONING_LEVELS: &[ThinkingConfig] = &[ThinkingConfig::Max, ThinkingConfig::Off];

/// Resolve the effort levels the UI / runtime should offer for a model.
///
/// - `supports_thinking == false` → only Off (reasoning unsupported)
/// - empty / unparseable `reasoning_levels` + thinking supported/unknown →
///   binary thinking on (`Max`) / thinking off
/// - non-empty registry levels → exactly those levels
pub fn thinking_levels_for_model(
    supports_thinking: Option<bool>,
    reasoning_levels: &[String],
) -> Vec<ThinkingConfig> {
    if supports_thinking == Some(false) {
        return vec![ThinkingConfig::Off];
    }

    if reasoning_levels.is_empty() {
        return BINARY_REASONING_LEVELS.to_vec();
    }

    let mut out = Vec::new();
    for raw in reasoning_levels {
        if let Some(level) = parse_reasoning_level(raw) {
            if !out.contains(&level) {
                out.push(level);
            }
        }
    }
    if out.is_empty() {
        return BINARY_REASONING_LEVELS.to_vec();
    }
    // Stable UI order (max/high/medium/low/off).
    let order = DEFAULT_REASONING_LEVELS;
    out.sort_by_key(|l| order.iter().position(|o| o == l).unwrap_or(99));
    out
}

/// Whether the model uses the binary off/on effort picker (no registry levels).
pub fn is_binary_effort_model(
    supports_thinking: Option<bool>,
    reasoning_levels: &[String],
) -> bool {
    if supports_thinking == Some(false) {
        return false;
    }
    if reasoning_levels.is_empty() {
        return true;
    }
    // Unparseable registry levels also fall back to binary.
    !reasoning_levels
        .iter()
        .any(|raw| parse_reasoning_level(raw).is_some())
}

/// Pick a thinking level for a model from registry + current preference.
///
/// - Models without reasoning support always resolve to [`ThinkingConfig::Off`].
/// - Supported preference is kept when valid.
/// - Otherwise uses registry `default_reasoning_effort`, then highest supported
///   (typically [`ThinkingConfig::Max`]).
pub fn resolve_model_thinking_level(
    current: ThinkingConfig,
    supports_thinking: Option<bool>,
    reasoning_levels: &[String],
    default_reasoning_effort: Option<&str>,
) -> ThinkingConfig {
    let supported = thinking_levels_for_model(supports_thinking, reasoning_levels);
    if supports_thinking == Some(false) {
        return ThinkingConfig::Off;
    }
    // Off is always a valid user preference; do not override it with a default.
    if current == ThinkingConfig::Off {
        return current;
    }
    if supported.contains(&current) {
        return current;
    }
    if let Some(def) = default_reasoning_effort.and_then(parse_reasoning_level) {
        return def.clamp_to_supported(&supported);
    }
    // Default: maximum supported effort (stable across tool-loop iterations).
    ThinkingConfig::Max.clamp_to_supported(&supported)
}

/// Map a [`ThinkingConfig`] to a provider effort label, preferring registry strings.
///
/// Returns `None` when thinking is off.
pub fn resolve_effort_label(
    thinking: ThinkingConfig,
    reasoning_levels: &[String],
    provider_id: &str,
) -> Option<String> {
    if matches!(thinking, ThinkingConfig::Off) {
        return None;
    }
    let concrete = thinking;

    // Prefer an exact registry string that maps to this level.
    for raw in reasoning_levels {
        if parse_reasoning_level(raw) == Some(concrete) {
            return Some(raw.trim().to_ascii_lowercase());
        }
    }

    // Provider-specific fallbacks when registry has no levels yet.
    let provider = crate::ProviderId::from_config_id(provider_id);
    if provider.as_str() == crate::ProviderId::OPENROUTER {
        return Some(
            match concrete {
                ThinkingConfig::Max => "xhigh",
                ThinkingConfig::High => "high",
                ThinkingConfig::Medium => "medium",
                ThinkingConfig::Low => "low",
                ThinkingConfig::Off => "medium",
            }
            .to_string(),
        );
    }

    Some(
        match concrete {
            ThinkingConfig::Max => {
                // OpenAI-style: xhigh when present in levels else high.
                if reasoning_levels
                    .iter()
                    .any(|l| matches!(l.trim().to_ascii_lowercase().as_str(), "xhigh" | "max"))
                {
                    "xhigh"
                } else {
                    "high"
                }
            }
            ThinkingConfig::High => "high",
            ThinkingConfig::Medium => "medium",
            ThinkingConfig::Low => "low",
            ThinkingConfig::Off => return None,
        }
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Regression: ThinkingConfig to ThinkingRequest ──────────────────────────

    #[test]
    fn regression_thinking_request_high_produces_effort_and_budget() {
        let request = ThinkingConfig::High.to_thinking_request();
        assert!(request.enabled);
        assert_eq!(request.effort.as_deref(), Some("high"));
        assert_eq!(request.budget_tokens, Some(10000));
    }

    #[test]
    fn regression_thinking_request_max_produces_effort_and_budget() {
        let request = ThinkingConfig::Max.to_thinking_request();
        assert!(request.enabled);
        assert_eq!(request.effort.as_deref(), Some("xhigh"));
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
        assert_eq!(request.effort.as_deref(), Some("medium"));
        assert_eq!(request.budget_tokens, Some(4096));
    }

    #[test]
    fn regression_thinking_request_low_produces_low_effort() {
        let request = ThinkingConfig::Low.to_thinking_request();
        assert!(request.enabled);
        assert_eq!(request.effort.as_deref(), Some("low"));
        assert_eq!(request.budget_tokens, Some(1024));
    }

    #[test]
    fn thinking_levels_for_model_respects_registry() {
        let levels = thinking_levels_for_model(
            Some(true),
            &["none".into(), "low".into(), "high".into(), "xhigh".into()],
        );
        // Exactly the registry levels — no Adaptive inject, no Medium fill-in.
        assert_eq!(
            levels,
            vec![
                ThinkingConfig::Max,
                ThinkingConfig::High,
                ThinkingConfig::Low,
                ThinkingConfig::Off,
            ]
        );
        assert!(!is_binary_effort_model(
            Some(true),
            &["none".into(), "low".into(), "high".into(), "xhigh".into()],
        ));
    }

    #[test]
    fn thinking_levels_off_only_when_no_thinking() {
        let levels = thinking_levels_for_model(Some(false), &["high".into()]);
        assert_eq!(levels, vec![ThinkingConfig::Off]);
        assert!(!is_binary_effort_model(Some(false), &["high".into()]));
    }

    #[test]
    fn thinking_levels_binary_when_registry_empty() {
        let levels = thinking_levels_for_model(Some(true), &[]);
        assert_eq!(levels, vec![ThinkingConfig::Max, ThinkingConfig::Off]);
        assert!(is_binary_effort_model(Some(true), &[]));
        assert!(is_binary_effort_model(None, &[]));
    }

    #[test]
    fn thinking_levels_model_specific_no_extra_options() {
        let levels = thinking_levels_for_model(Some(true), &["low".into(), "high".into()]);
        assert_eq!(levels, vec![ThinkingConfig::High, ThinkingConfig::Low]);
    }

    #[test]
    fn resolve_effort_prefers_registry_label() {
        let label = resolve_effort_label(
            ThinkingConfig::Max,
            &["low".into(), "high".into(), "xhigh".into()],
            "openai",
        );
        assert_eq!(label.as_deref(), Some("xhigh"));
    }

    #[test]
    fn clamp_unsupported_level_to_supported() {
        let supported = vec![ThinkingConfig::Low, ThinkingConfig::Off];
        assert_eq!(
            ThinkingConfig::High.clamp_to_supported(&supported),
            ThinkingConfig::Low
        );
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
            content_parts: Vec::new(),
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

    // ── Effort resolution (no adaptive) ────────────────────────────────────────

    #[test]
    fn legacy_adaptive_string_maps_to_max() {
        assert_eq!(
            ThinkingConfig::from_config_str("adaptive"),
            ThinkingConfig::Max
        );
        assert_eq!(parse_reasoning_level("auto"), Some(ThinkingConfig::Max));
        assert_eq!(parse_reasoning_level("on"), Some(ThinkingConfig::Max));
        let deserialized: ThinkingConfig = serde_json::from_str("\"adaptive\"").unwrap();
        assert_eq!(deserialized, ThinkingConfig::Max);
    }

    #[test]
    fn resolve_forces_off_when_model_lacks_reasoning() {
        let resolved =
            resolve_model_thinking_level(ThinkingConfig::Max, Some(false), &["high".into()], None);
        assert_eq!(resolved, ThinkingConfig::Off);
    }

    #[test]
    fn resolve_defaults_to_max_when_preference_unsupported() {
        let resolved = resolve_model_thinking_level(
            ThinkingConfig::Low,
            Some(true),
            &["high".into(), "xhigh".into()],
            None,
        );
        assert_eq!(resolved, ThinkingConfig::Max);
    }

    #[test]
    fn resolve_respects_off_for_thinking_model() {
        let resolved = resolve_model_thinking_level(
            ThinkingConfig::Off,
            Some(true),
            &["high".into(), "max".into()],
            Some("high"),
        );
        assert_eq!(resolved, ThinkingConfig::Off);
    }

    #[test]
    fn binary_on_is_max() {
        assert_eq!(
            BINARY_REASONING_LEVELS,
            &[ThinkingConfig::Max, ThinkingConfig::Off]
        );
        assert_eq!(
            effort_display_label(ThinkingConfig::Max, true),
            "thinking on"
        );
        assert_eq!(
            effort_display_label(ThinkingConfig::Off, true),
            "thinking off"
        );
    }
}
