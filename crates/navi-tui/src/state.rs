use std::collections::HashMap;
use std::time::{Duration, Instant};

use navi_sdk::{
    NaviUsageReport, QuestionRequest, SubagentTranscriptItem, ThinkingConfig, ToolInvocation,
    ToolResult,
};
use ratatui::layout::Rect;
use ratatui::text::Line;

/// An image captured from the clipboard, waiting to be sent with the next message.
pub struct PendingImage {
    /// MIME type of the image (e.g. `"image/png"`, `"image/jpeg"`).
    pub media_type: String,
    /// Base64-encoded image data (raw, no data-URL prefix).
    pub data: String,
    /// Image width in pixels, if known.
    pub width: Option<u32>,
    /// Image height in pixels, if known.
    pub height: Option<u32>,
}

#[derive(Debug)]
pub(crate) struct QueuedUserMessage {
    pub(crate) text: String,
    pub(crate) images: Vec<PendingImage>,
}

/// Display + hover-preview metadata for an image attached to a chat message.
/// Base64 is kept for the hover modal (same bytes already live in conversation history).
pub struct ChatImage {
    /// 1-based index shown in `[Image N]` tags.
    pub index: usize,
    /// MIME type (e.g. `"image/png"`).
    pub media_type: String,
    /// Image width in pixels, if known.
    pub width: Option<u32>,
    /// Image height in pixels, if known.
    pub height: Option<u32>,
    /// Raw base64 payload (no data-URL prefix) for hover preview.
    pub data: String,
    /// Short label used by older render paths (e.g. `"PNG"` or `"image PNG 1200x800"`).
    pub label: String,
}

impl std::fmt::Debug for ChatImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatImage")
            .field("index", &self.index)
            .field("media_type", &self.media_type)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("data_len", &self.data.len())
            .field("label", &self.label)
            .finish()
    }
}

impl Clone for ChatImage {
    fn clone(&self) -> Self {
        Self {
            index: self.index,
            media_type: self.media_type.clone(),
            width: self.width,
            height: self.height,
            data: self.data.clone(),
            label: self.label.clone(),
        }
    }
}

impl ChatImage {
    pub fn from_pending(index: usize, image: &PendingImage) -> Self {
        let mime_short = image
            .media_type
            .strip_prefix("image/")
            .unwrap_or(&image.media_type)
            .to_uppercase();
        Self {
            index: index.max(1),
            media_type: image.media_type.clone(),
            width: image.width,
            height: image.height,
            data: image.data.clone(),
            label: mime_short,
        }
    }

    pub fn estimated_bytes(&self) -> usize {
        // base64 → roughly 3/4 raw bytes
        self.data.len().saturating_mul(3) / 4
    }

    pub fn format_short(&self) -> String {
        self.media_type
            .strip_prefix("image/")
            .unwrap_or(&self.media_type)
            .to_uppercase()
    }
}

/// Floating hover preview for an attached image (composer or chat).
#[derive(Debug, Clone)]
pub struct ImageHoverPreview {
    pub index: usize,
    pub media_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size_bytes: usize,
    pub filename: Option<String>,
    /// Reserved for future terminal image-protocol / half-block rendering.
    #[allow(dead_code)]
    pub data: String,
}

impl ImageHoverPreview {
    pub fn from_pending(index: usize, image: &PendingImage) -> Self {
        Self {
            index: index.saturating_add(1),
            media_type: image.media_type.clone(),
            width: image.width,
            height: image.height,
            size_bytes: image.data.len().saturating_mul(3) / 4,
            filename: None,
            data: image.data.clone(),
        }
    }

    pub fn from_chat(image: &ChatImage) -> Self {
        Self {
            index: image.index,
            media_type: image.media_type.clone(),
            width: image.width,
            height: image.height,
            size_bytes: image.estimated_bytes(),
            filename: None,
            data: image.data.clone(),
        }
    }

    pub fn format_short(&self) -> String {
        self.media_type
            .strip_prefix("image/")
            .unwrap_or(&self.media_type)
            .to_uppercase()
    }

    pub fn header_line(&self) -> String {
        let mut parts = vec![format!("Image #{}", self.index), self.format_short()];
        if let (Some(w), Some(h)) = (self.width, self.height) {
            parts.push(format!("{w}×{h}"));
        }
        parts.push(format_byte_size(self.size_bytes));
        if let Some(name) = &self.filename {
            parts.push(name.clone());
        }
        parts.join("  ·  ")
    }
}

fn format_byte_size(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let n = bytes as f64;
    if n >= MB {
        format!("{:.1} MB", n / MB)
    } else if n >= KB {
        format!("{:.1} KB", n / KB)
    } else {
        format!("{bytes} B")
    }
}

impl std::fmt::Debug for PendingImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingImage")
            .field("media_type", &self.media_type)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

impl PendingImage {
    /// Returns a human-readable label like `"image PNG 1200x800"`.
    pub fn label(&self) -> String {
        let mime_short = self
            .media_type
            .strip_prefix("image/")
            .unwrap_or(&self.media_type)
            .to_uppercase();
        match (self.width, self.height) {
            (Some(w), Some(h)) => format!("image {mime_short} {w}x{h}"),
            _ => format!("image {mime_short}"),
        }
    }

    /// Returns a numbered label like `"image 1 PNG 1200x800"`.
    pub fn numbered_label(&self, index: usize) -> String {
        let label = self.label();
        let details = label.strip_prefix("image ").unwrap_or(&label);
        format!("image {} {}", index + 1, details)
    }

    /// Estimated base64 size in bytes (for size-cap enforcement).
    pub fn estimated_bytes(&self) -> usize {
        self.data.len()
    }
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    /// Image attachments carried with this message (display metadata only).
    /// The actual base64 data lives in `content_parts` on the engine side;
    /// this field stores labels/dimensions for the TUI render.
    pub image_labels: Vec<String>,
    pub images: Vec<ChatImage>,
    pub model_label: Option<String>,
    pub provider_label: Option<String>,
    pub elapsed_ms: Option<u64>,
    pub status: Option<String>,
    pub usage_label: Option<String>,
    pub thinking_content: String,
    pub tool_invocation: Option<ToolInvocation>,
    pub tool_result: Option<ToolResult>,
    pub is_compact_summary: bool,
}

impl ChatMessage {
    pub fn new(role: ChatRole, content: String) -> Self {
        Self {
            role,
            content,
            image_labels: Vec::new(),
            images: Vec::new(),
            model_label: None,
            provider_label: None,
            elapsed_ms: None,
            status: None,
            usage_label: None,
            thinking_content: String::new(),
            tool_invocation: None,
            tool_result: None,
            is_compact_summary: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChatRole {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub(crate) struct Notification {
    pub title: String,
    pub message: String,
    pub created_at: Instant,
    pub ttl: Duration,
}

#[derive(Default)]
pub(crate) struct ChatRenderCache {
    pub width: usize,
    pub full_tool_view: bool,
    pub show_thinking: bool,
    pub compact_tool_visible_limit: usize,
    pub expanded_tool_signature: String,
    pub signature_hash: u64,
    pub lines: Vec<Line<'static>>,
    pub line_sources: Vec<ChatLineSource>,
    pub chat_rect: Option<Rect>,
    pub tool_render_cache: HashMap<String, Vec<Line<'static>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub(crate) enum ChatView {
    #[default]
    Parent,
    Subagent {
        invocation_id: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct SubagentTranscript {
    pub(crate) title: String,
    pub(crate) items: Vec<SubagentTranscriptItem>,
}

impl SubagentTranscript {
    pub(crate) fn new(title: String) -> Self {
        Self {
            title,
            items: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum ChatLineSource {
    #[default]
    None,
    Message(usize),
    ToolResult(String),
    ToolGroup(Vec<String>),
    Subagent(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageAction {
    Revert,
    Copy,
    Fork,
}

impl MessageAction {
    pub(crate) const ALL: [Self; 3] = [Self::Revert, Self::Copy, Self::Fork];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Revert => "Revert to here",
            Self::Copy => "Copy text",
            Self::Fork => "Fork from here",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Revert => "move this message back to input",
            Self::Copy => "copy selected message",
            Self::Fork => "start new session from this point",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupPhase {
    /// User needs to pick/configure a provider.
    ProviderLogin,
    /// Model is interviewing the user with `question` tool.
    Interview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Commands,
    Models,
    ApiKeyEntry,
    Thinking,
    Sessions,
    Settings,
    Providers,
    Usage,
    Debug,
    Help,
    Skills,
    Plugins,
    PluginApproval,
    Question,
    ThemePicker,
    MessageActions,
    Mcp,
    OAuth,
    BackgroundCommands,
    BackgroundCommandOutput,
    BackgroundModels,
    BgModelPicker,
    Setup,
    AttachmentModels,
    MessageQueue,
    QueuedMessageEdit,
    ConfirmCancelTurn,
    ConfirmPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModalKind {
    Commands,
    Models,
    ApiKeyEntry,
    Thinking,
    Sessions,
    Settings,
    Providers,
    Usage,
    Debug,
    Help,
    Skills,
    Plugins,
    PluginApproval,
    Question,
    ThemePicker,
    MessageActions,
    Mcp,
    OAuth,
    BackgroundCommands,
    BackgroundCommandOutput,
    BackgroundModels,
    BgModelPicker,
    AttachmentModels,
    MessageQueue,
    QueuedMessageEdit,
    ConfirmCancelTurn,
    ConfirmPlan,
}

impl ModalKind {
    pub(crate) fn mode(self) -> Mode {
        match self {
            Self::Commands => Mode::Commands,
            Self::Models => Mode::Models,
            Self::ApiKeyEntry => Mode::ApiKeyEntry,
            Self::Thinking => Mode::Thinking,
            Self::Sessions => Mode::Sessions,
            Self::Settings => Mode::Settings,
            Self::Providers => Mode::Providers,
            Self::Usage => Mode::Usage,
            Self::Debug => Mode::Debug,
            Self::Help => Mode::Help,
            Self::Skills => Mode::Skills,
            Self::Plugins => Mode::Plugins,
            Self::PluginApproval => Mode::PluginApproval,
            Self::Question => Mode::Question,
            Self::ThemePicker => Mode::ThemePicker,
            Self::MessageActions => Mode::MessageActions,
            Self::Mcp => Mode::Mcp,
            Self::OAuth => Mode::OAuth,
            Self::BackgroundCommands => Mode::BackgroundCommands,
            Self::BackgroundCommandOutput => Mode::BackgroundCommandOutput,
            Self::BackgroundModels => Mode::BackgroundModels,
            Self::BgModelPicker => Mode::BgModelPicker,
            Self::AttachmentModels => Mode::AttachmentModels,
            Self::MessageQueue => Mode::MessageQueue,
            Self::QueuedMessageEdit => Mode::QueuedMessageEdit,
            Self::ConfirmCancelTurn => Mode::ConfirmCancelTurn,
            Self::ConfirmPlan => Mode::ConfirmPlan,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct UsageUiState {
    pub loading: bool,
    pub report: Option<NaviUsageReport>,
    pub error: Option<String>,
    /// Cumulative tokens for the current TUI session (all providers).
    pub session_input_tokens: u64,
    pub session_output_tokens: u64,
    pub last_input_tokens: Option<u64>,
    pub last_output_tokens: Option<u64>,
    /// Estimated session spend in USD from registry list pricing × tokens.
    /// Used for non-OAuth / API-key providers that bill per token.
    pub session_cost_usd: f64,
    /// True once at least one turn had list pricing available.
    pub session_cost_known: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct OAuthUiState {
    pub provider_id: String,
    pub verification_uri: String,
    pub user_code: String,
    /// When set, the TUI can write a pasted authorization code here for the
    /// waiting OAuth task (xAI shows a copy-code page when loopback fails).
    pub paste_slot: Option<std::sync::Arc<std::sync::Mutex<Option<String>>>>,
    /// Last paste feedback shown in the modal.
    pub paste_status: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct QuestionUiState {
    pub request: QuestionRequest,
    pub selected_row: usize,
    pub option_scroll: usize,
    pub selected_options: Vec<bool>,
    pub custom_answer: String,
    pub custom_cursor: usize,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct McpUiState {
    pub scroll: usize,
    pub selected_server: usize,
    pub selected_tool: usize,
    pub is_focused_on_tools: bool,
}

impl QuestionUiState {
    pub(crate) fn new(request: QuestionRequest) -> Self {
        let selected_options = vec![false; request.options.len()];
        Self {
            request,
            selected_row: 0,
            option_scroll: 0,
            selected_options,
            custom_answer: String::new(),
            custom_cursor: 0,
        }
    }

    pub(crate) fn row_count(&self) -> usize {
        self.request.options.len() + 2
    }

    pub(crate) fn custom_row_index(&self) -> usize {
        self.request.options.len()
    }

    pub(crate) fn selected_is_custom(&self) -> bool {
        self.selected_row == self.custom_row_index()
    }

    pub(crate) fn deny_row_index(&self) -> usize {
        self.request.options.len() + 1
    }

    pub(crate) fn selected_is_deny(&self) -> bool {
        self.selected_row == self.deny_row_index()
    }

    pub(crate) fn selected_answers(&self) -> Vec<String> {
        if self.selected_is_custom() {
            let answer = self.custom_answer.trim();
            return if answer.is_empty() {
                Vec::new()
            } else {
                vec![answer.to_string()]
            };
        }

        if self.request.multiple {
            let answers = self
                .request
                .options
                .iter()
                .enumerate()
                .filter(|(index, _)| self.selected_options.get(*index).copied().unwrap_or(false))
                .map(|(_, option)| option.label.clone())
                .collect::<Vec<_>>();
            if !answers.is_empty() {
                return answers;
            }
        }

        self.request
            .options
            .get(self.selected_row)
            .map(|option| vec![option.label.clone()])
            .unwrap_or_default()
    }

    pub(crate) fn focus_custom(&mut self) {
        self.selected_row = self.custom_row_index();
        self.custom_cursor = floor_boundary(&self.custom_answer, self.custom_cursor);
    }

    pub(crate) fn insert_custom_char(&mut self, ch: char) {
        self.focus_custom();
        self.custom_answer.insert(self.custom_cursor, ch);
        self.custom_cursor += ch.len_utf8();
    }

    pub(crate) fn delete_custom_previous_char(&mut self) {
        self.focus_custom();
        if self.custom_cursor == 0 {
            return;
        }
        let previous = self.custom_answer[..self.custom_cursor]
            .char_indices()
            .last()
            .map(|(index, _)| index)
            .unwrap_or(0);
        self.custom_answer.drain(previous..self.custom_cursor);
        self.custom_cursor = previous;
    }

    pub(crate) fn delete_custom_next_char(&mut self) {
        self.focus_custom();
        if self.custom_cursor >= self.custom_answer.len() {
            return;
        }
        let next = next_boundary(&self.custom_answer, self.custom_cursor);
        self.custom_answer.drain(self.custom_cursor..next);
    }

    pub(crate) fn move_custom_left(&mut self) {
        self.focus_custom();
        if self.custom_cursor == 0 {
            return;
        }
        self.custom_cursor = self.custom_answer[..self.custom_cursor]
            .char_indices()
            .last()
            .map(|(index, _)| index)
            .unwrap_or(0);
    }

    pub(crate) fn move_custom_right(&mut self) {
        self.focus_custom();
        self.custom_cursor = next_boundary(&self.custom_answer, self.custom_cursor);
    }

    pub(crate) fn move_custom_home(&mut self) {
        self.focus_custom();
        self.custom_cursor = 0;
    }

    pub(crate) fn move_custom_end(&mut self) {
        self.focus_custom();
        self.custom_cursor = self.custom_answer.len();
    }

    pub(crate) fn clear_custom(&mut self) {
        self.focus_custom();
        self.custom_answer.clear();
        self.custom_cursor = 0;
    }
}

fn floor_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn next_boundary(value: &str, index: usize) -> usize {
    let index = floor_boundary(value, index);
    value[index..]
        .char_indices()
        .nth(1)
        .map(|(offset, _)| index + offset)
        .unwrap_or(value.len())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThinkingLevel {
    Adaptive,
    Max,
    High,
    Medium,
    Low,
    Off,
}

impl From<ThinkingLevel> for ThinkingConfig {
    fn from(value: ThinkingLevel) -> Self {
        match value {
            ThinkingLevel::Adaptive => Self::Adaptive,
            ThinkingLevel::Max => Self::Max,
            ThinkingLevel::High => Self::High,
            ThinkingLevel::Medium => Self::Medium,
            ThinkingLevel::Low => Self::Low,
            ThinkingLevel::Off => Self::Off,
        }
    }
}

impl ThinkingLevel {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Adaptive => "adaptive",
            Self::Max => "max",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Off => "off",
        }
    }

    pub(crate) fn from_config(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "adaptive" => Self::Adaptive,
            "max" => Self::Max,
            "high" => Self::High,
            "medium" => Self::Medium,
            "low" => Self::Low,
            "off" => Self::Off,
            _ => Self::Adaptive,
        }
    }

    pub(crate) fn config_value(self) -> &'static str {
        self.label()
    }

    pub(crate) fn index(self) -> usize {
        match self {
            Self::Adaptive => 0,
            Self::Max => 1,
            Self::High => 2,
            Self::Medium => 3,
            Self::Low => 4,
            Self::Off => 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectionState {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub active: bool,
}

/// A pending plugin install/update approval in the TUI.
#[derive(Debug, Clone)]
pub(crate) struct PluginApprovalRequest {
    /// Unique id used to correlate with the decision callback.
    pub id: String,
    /// Source directory or path being installed from.
    pub source_path: String,
    /// The plugin id.
    pub plugin_id: String,
    /// The plugin version.
    pub version: String,
    /// The plugin publisher.
    pub publisher: String,
    /// Overall risk string (LOW, MEDIUM, HIGH, CRITICAL).
    pub overall_risk: String,
    /// Pre-formatted capabilities list (one per line, already truncated).
    pub capabilities_text: String,
    /// Pre-formatted tools list.
    pub tools_text: String,
    /// Pre-formatted warnings list.
    pub warnings_text: String,
    /// Whether this is an install or an update.
    pub kind: PluginApprovalKind,
    /// Pre-formatted diff (for updates), empty for installs.
    pub changes_text: String,
    /// For updates, the reconsent action label.
    pub reconsent_action: Option<String>,
    /// When the user approves, the on-disk install is performed.
    pub install_on_approve: bool,
}

/// Whether the approval is for a fresh install or an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PluginApprovalKind {
    Install,
    Update,
}

#[derive(Debug, Clone)]
pub(crate) struct GoalUiState {
    pub objective: String,
    pub short_description: Option<String>,
    pub tokens_used: i64,
    pub token_budget: Option<i64>,
}

impl Default for GoalUiState {
    fn default() -> Self {
        Self {
            objective: String::new(),
            short_description: None,
            tokens_used: 0,
            token_budget: None,
        }
    }
}
