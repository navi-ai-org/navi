use std::collections::HashMap;
use std::time::{Duration, Instant};

use navi_sdk::{
    NaviUsageReport, QuestionRequest, SubagentTranscriptItem, ThinkingConfig, ToolInvocation,
    ToolResult,
};
use ratatui::layout::Rect;
use ratatui::text::Line;

/// An image captured from the clipboard, waiting to be attached to the next message.
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

/// A large clipboard paste held outside the composer draft and shown as a chip.
#[derive(Debug, Clone)]
pub struct PendingPaste {
    /// Full pasted body (newlines normalized to `\n`).
    pub text: String,
    /// Line count shown in the chip (`+N lines`).
    pub line_count: usize,
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
    /// Base64 payload used to encode a Kitty/Sixel/iTerm2 preview when supported.
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
        parts.join(" · ")
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
    /// Post-turn recap line ("Recap").
    pub is_recap: bool,
    /// User message that defined a thread goal (shown with a Goal label in chat).
    pub is_goal: bool,
    /// Wall-clock time when the message was submitted/received (for /// right-aligned timestamps on user prompts).
    pub sent_at: Option<std::time::SystemTime>,
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
            is_recap: false,
            is_goal: false,
            sent_at: None,
        }
    }

    /// Stamp `sent_at` with the local wall clock (user submit / assistant done).
    pub fn stamp_now(mut self) -> Self {
        self.sent_at = Some(std::time::SystemTime::now());
        self
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
    /// Number of finalized messages covered by `history_*` (excludes streaming tail).
    pub history_message_count: usize,
    /// Signature of finalized history only (cheap to recheck while streaming).
    pub history_signature: u64,
    /// Cached lines for finalized history prefix.
    pub history_lines: Vec<Line<'static>>,
    pub history_line_sources: Vec<ChatLineSource>,
    /// Absolute top line of the chat viewport when the user has scrolled up.
    ///
    /// `scroll_offset` is bottom-relative (0 = follow the live end). While the
    /// user is reading history, streaming / composer-height changes would move
    /// that bottom-relative window. We lock the absolute top line across frames
    /// so the text under the cursor stays put for copy/selection.
    pub locked_viewport_top: Option<usize>,
    /// `scroll_offset` value that produced `locked_viewport_top`. When the user
    /// (or jump-to-latest) changes `scroll_offset`, we re-derive the lock.
    pub locked_scroll_offset: usize,
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

/// Model is still generating tool-call arguments (before ToolRequested).
#[derive(Debug, Clone)]
pub(crate) struct StreamingToolCall {
    pub(crate) id: Option<String>,
    pub(crate) tool_name: String,
    pub(crate) arguments_chars: usize,
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
    /// Copy assistant/tool output after this user message (until the next user turn).
    CopyResponse,
    /// Copy only the selected message text.
    Copy,
    /// Copy the full session transcript.
    CopySession,
    Revert,
    Fork,
}

impl MessageAction {
    pub(crate) const ALL: [Self; 5] = [
        Self::CopyResponse,
        Self::Copy,
        Self::CopySession,
        Self::Revert,
        Self::Fork,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::CopyResponse => "Copy response",
            Self::Copy => "Copy text",
            Self::CopySession => "Copy session",
            Self::Revert => "Revert to here",
            Self::Fork => "Fork from here",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::CopyResponse => "assistant output since this message",
            Self::Copy => "copy selected message only",
            Self::CopySession => "full session transcript",
            Self::Revert => "restore files + history to this prompt",
            Self::Fork => "start new session from this point",
        }
    }

    /// Stable config key for preference persistence (order-independent).
    pub(crate) fn config_key(self) -> &'static str {
        match self {
            Self::CopyResponse => "copy_response",
            Self::Copy => "copy_text",
            Self::CopySession => "copy_session",
            Self::Revert => "revert",
            Self::Fork => "fork",
        }
    }

    pub(crate) fn from_config_key(key: &str) -> Option<Self> {
        match key.trim() {
            "copy_response" => Some(Self::CopyResponse),
            "copy_text" => Some(Self::Copy),
            "copy_session" => Some(Self::CopySession),
            "revert" => Some(Self::Revert),
            "fork" => Some(Self::Fork),
            _ => None,
        }
    }

    pub(crate) fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|action| *action == self)
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupPhase {
    /// User needs to pick/configure a provider.
    ProviderLogin,
    /// User chooses the dedicated model used for automatic memory extraction.
    MemoryModel,
    /// Choose default permission mode (restricted / accept-edits / yolo).
    Approvals,
    /// Optional tip about marketplace WASM plugins (skip or continue).
    MarketplaceTip,
    /// Model is interviewing the user with `question` tool.
    Interview,
}

/// Tab inside the unified Model Routing modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ModelRoutingTab {
    Chat,
    #[default]
    Agents,
    Attachments,
}

impl ModelRoutingTab {
    pub(crate) const ALL: [Self; 3] = [Self::Chat, Self::Agents, Self::Attachments];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Agents => "Agents",
            Self::Attachments => "Attachments",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Chat => Self::Agents,
            Self::Agents => Self::Attachments,
            Self::Attachments => Self::Chat,
        }
    }

    pub(crate) fn previous(self) -> Self {
        match self {
            Self::Chat => Self::Attachments,
            Self::Agents => Self::Chat,
            Self::Attachments => Self::Agents,
        }
    }
}

/// Row in the Extensions hub modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExtensionsHubItem {
    Skills,
    Plugins,
    McpServers,
}

impl ExtensionsHubItem {
    pub(crate) const ALL: [Self; 3] = [Self::Skills, Self::Plugins, Self::McpServers];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Skills => "Skills…",
            Self::Plugins => "Plugins…",
            Self::McpServers => "MCP Servers…",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Skills => "Activate prompt skills for this session",
            Self::Plugins => "Browse and manage installed plugins",
            Self::McpServers => "Configure Model Context Protocol servers",
        }
    }
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
    /// Rewind modal: pick a past user prompt to restore history + files.
    Rewind,
    Mcp,
    OAuth,
    BackgroundCommands,
    BackgroundCommandOutput,
    BackgroundModels,
    BgModelPicker,
    /// Unified model routing (Chat / Agents / Attachments).
    ModelRouting,
    /// Extensions hub (Skills / Plugins / MCP).
    /// Constructed via [`ModalKind::Extensions`]; palette often uses OpenHub instead.
    #[allow(dead_code)] // dedicated hub mode; palette prefers OpenHub deep-links
    Extensions,
    Setup,
    AttachmentModels,
    MessageQueue,
    QueuedMessageEdit,
    /// Multi-line entry for setting a thread goal (sent as chat + set_goal).
    SetGoal,
    ConfirmCancelTurn,
    ConfirmPlan,
    /// Confirm merging mcp.json from a just-installed plugin into global config.
    ConfirmMcpMerge,
    /// Masked sudo password (secret never enters chat/model context).
    SudoPassword,
    /// `@` path/file/folder mention palette.
    PathMentions,
    /// About NAVI (product blurb + links).
    About,
    /// Confirm / install a pending self-update.
    UpdateAvailable,
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
    Rewind,
    Mcp,
    OAuth,
    BackgroundCommands,
    BackgroundCommandOutput,
    /// Legacy standalone agent-routes modal (superseded by [`Self::ModelRouting`]).
    #[allow(dead_code)] // retained for mode map / migration from old routing UI
    BackgroundModels,
    BgModelPicker,
    ModelRouting,
    Extensions,
    /// Legacy standalone attachment modal (superseded by [`Self::ModelRouting`]).
    #[allow(dead_code)] // retained for mode map / migration from old attachment UI
    AttachmentModels,
    MessageQueue,
    QueuedMessageEdit,
    SetGoal,
    ConfirmCancelTurn,
    ConfirmPlan,
    SudoPassword,
    PathMentions,
    About,
    UpdateAvailable,
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
            Self::Rewind => Mode::Rewind,
            Self::Mcp => Mode::Mcp,
            Self::OAuth => Mode::OAuth,
            Self::BackgroundCommands => Mode::BackgroundCommands,
            Self::BackgroundCommandOutput => Mode::BackgroundCommandOutput,
            Self::BackgroundModels => Mode::BackgroundModels,
            Self::BgModelPicker => Mode::BgModelPicker,
            Self::ModelRouting => Mode::ModelRouting,
            Self::Extensions => Mode::Extensions,
            Self::AttachmentModels => Mode::AttachmentModels,
            Self::MessageQueue => Mode::MessageQueue,
            Self::QueuedMessageEdit => Mode::QueuedMessageEdit,
            Self::SetGoal => Mode::SetGoal,
            Self::ConfirmCancelTurn => Mode::ConfirmCancelTurn,
            Self::ConfirmPlan => Mode::ConfirmPlan,
            Self::SudoPassword => Mode::SudoPassword,
            Self::PathMentions => Mode::PathMentions,
            Self::About => Mode::About,
            Self::UpdateAvailable => Mode::UpdateAvailable,
        }
    }
}

/// UI state for the masked sudo password modal.
#[derive(Debug, Clone)]
pub(crate) struct SudoPasswordUiState {
    pub request_id: String,
    pub command_summary: String,
    /// In-memory only; never written to session logs.
    pub password: String,
    pub cursor: usize,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct UsageUiState {
    pub loading: bool,
    /// When true, kick another refresh after the in-flight one finishes
    /// (coalesces open-modal + R while a fetch is already running).
    pub refresh_pending: bool,
    pub report: Option<NaviUsageReport>,
    pub error: Option<String>,
    /// Cumulative tokens for the current TUI session (all providers).
    pub session_input_tokens: u64,
    pub session_output_tokens: u64,
    pub last_input_tokens: Option<u64>,
    pub last_output_tokens: Option<u64>,
    /// Estimated session spend in USD from registry list pricing × tokens.
    /// Used for API-key and prepaid credit providers that publish list rates.
    pub session_cost_usd: f64,
    /// True once at least one turn had list pricing available.
    pub session_cost_known: bool,
    /// Estimated prepaid credits spent this session (e.g. Hypercredits).
    pub session_credits_spent: Option<f64>,
    /// Credit unit label when `session_credits_spent` is set.
    pub session_credit_unit: Option<String>,
    /// Remaining account Hypercredits (or other prepaid balance) from the last
    /// usage fetch. Crush shows this in the header/sidebar after each turn.
    pub remaining_credits: Option<f64>,
    /// Unit for `remaining_credits` (e.g. `hypercredits`).
    pub remaining_credit_unit: Option<String>,
    /// Last turn in→out label (e.g. `34k→1.2k`) for the footer after each UsageReported.
    pub last_turn_label: Option<String>,
    /// Latest cumulative usage snapshot for the model request currently being
    /// streamed. Provider usage chunks are snapshots, not independent bills;
    /// keeping this lets the TUI add only the newly reported portion to
    /// session totals and cost.
    pub request_input_tokens: u64,
    pub request_output_tokens: u64,
    pub request_cache_creation_tokens: u64,
    pub request_cache_read_tokens: u64,
    /// Conservative in-flight estimate shown only while the provider has not
    /// completed the current request. It is intentionally kept separate from
    /// billed session totals and cost.
    pub estimated_request_input_tokens: Option<u64>,
    pub estimated_request_output_bytes: usize,
    /// First streamed model byte this request (text / thinking / tool args).
    /// Presence means "stream has started"; **not** the t/s denominator.
    pub stream_started_at: Option<Instant>,
    /// Bytes of streamed model output: assistant text, reasoning, tool-call args.
    pub stream_output_bytes: usize,
    /// Accumulated **active generation** time (ms). Excludes TTFT idle and
    /// tool/approval stalls so avg t/s is real throughput, not wall time.
    pub stream_active_ms: u64,
    /// Last counted stream byte. Cleared on pause so idle gaps never inflate t/s.
    pub stream_last_byte_at: Option<Instant>,
    /// Last time an account usage request was started. This rate-limits quiet
    /// refreshes while a long-running turn is active.
    pub last_account_refresh_at: Option<Instant>,
}

/// Inter-chunk gaps longer than this are treated as a pause (tools/network).
const STREAM_ACTIVE_GAP_CAP_MS: u64 = 2_500;
/// Hot tail after the last byte still treated as active generation.
const STREAM_HOT_TAIL_MS: u64 = 400;

impl UsageUiState {
    pub(crate) fn begin_request_estimate(&mut self, input_tokens: u64) {
        self.reset_request_usage();
        self.estimated_request_input_tokens = Some(input_tokens);
    }

    pub(crate) fn reset_request_usage(&mut self) {
        self.request_input_tokens = 0;
        self.request_output_tokens = 0;
        self.request_cache_creation_tokens = 0;
        self.request_cache_read_tokens = 0;
        self.estimated_request_input_tokens = None;
        self.estimated_request_output_bytes = 0;
        self.stream_started_at = None;
        self.stream_output_bytes = 0;
        self.stream_active_ms = 0;
        self.stream_last_byte_at = None;
    }

    /// Pause generation clock (tools running, approvals, end of model step).
    pub(crate) fn pause_stream_throughput(&mut self) {
        self.stream_last_byte_at = None;
    }

    pub(crate) fn add_estimated_output(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.note_streamed_bytes(text.len());
    }

    /// Count streamed bytes (text, thinking, or tool-call argument payloads).
    pub(crate) fn note_streamed_bytes(&mut self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        let now = Instant::now();
        if let Some(last) = self.stream_last_byte_at {
            let gap_ms = now.duration_since(last).as_millis() as u64;
            self.stream_active_ms = self
                .stream_active_ms
                .saturating_add(gap_ms.min(STREAM_ACTIVE_GAP_CAP_MS));
        }
        // First byte after reset/pause: do not invent time for TTFT idle.
        if self.stream_started_at.is_none() {
            self.stream_started_at = Some(now);
        }
        self.stream_last_byte_at = Some(now);
        self.estimated_request_output_bytes =
            self.estimated_request_output_bytes.saturating_add(bytes);
        self.stream_output_bytes = self.stream_output_bytes.saturating_add(bytes);
    }

    pub(crate) fn estimated_request_output_tokens(&self) -> u64 {
        self.estimated_request_output_bytes.saturating_add(3) as u64 / 4
    }

    /// Estimated tokens delivered by the live stream (thinking + content + tool args).
    pub(crate) fn stream_output_tokens(&self) -> u64 {
        self.stream_output_bytes.saturating_add(3) as u64 / 4
    }

    fn stream_active_elapsed_ms(&self) -> u64 {
        let mut ms = self.stream_active_ms;
        if let Some(last) = self.stream_last_byte_at {
            let since = last.elapsed().as_millis() as u64;
            if since <= STREAM_HOT_TAIL_MS {
                ms = ms.saturating_add(since);
            }
        }
        ms
    }

    /// Average generation rate over **active** stream time only.
    ///
    /// Excludes time-to-first-token and tool/approval idle. Counts assistant
    /// text, reasoning, and tool-call argument bytes (≈ chars/4 tokens).
    pub(crate) fn stream_avg_tokens_per_sec(&self) -> Option<f64> {
        let tokens = self.stream_output_tokens();
        if tokens == 0 {
            return None;
        }
        let elapsed_ms = self.stream_active_elapsed_ms();
        if elapsed_ms < 80 {
            return None;
        }
        let secs = elapsed_ms as f64 / 1_000.0;
        if secs <= 0.0 {
            return None;
        }
        Some(tokens as f64 / secs)
    }

    /// Records a provider usage snapshot and returns only the newly reported
    /// token amounts. Zero means "not supplied" for this stream event.
    pub(crate) fn observe_request_usage(
        &mut self,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
    ) -> (u64, u64, u64, u64) {
        fn delta(snapshot: &mut u64, reported: u64) -> u64 {
            if reported == 0 || reported <= *snapshot {
                return 0;
            }
            let delta = reported.saturating_sub(*snapshot);
            *snapshot = reported;
            delta
        }

        let delta = (
            delta(&mut self.request_input_tokens, input_tokens),
            delta(&mut self.request_output_tokens, output_tokens),
            delta(
                &mut self.request_cache_creation_tokens,
                cache_creation_tokens,
            ),
            delta(&mut self.request_cache_read_tokens, cache_read_tokens),
        );
        // The provider has supplied authoritative values for this request.
        self.estimated_request_input_tokens = None;
        self.estimated_request_output_bytes = 0;
        delta
    }
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

/// Live MCP connection snapshot for the TUI modal (probed on open/refresh).
#[derive(Debug, Clone)]
pub struct McpLiveServer {
    pub id: String,
    pub enabled: bool,
    /// True when connected to the MCP process/server.
    pub connected: bool,
    /// False until session seed or a probe confirms status. Unknown must not
    /// render as "failed" (red) — that flash made every open look broken.
    pub known: bool,
    pub tools: Vec<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct McpUiState {
    pub scroll: usize,
    pub selected_server: usize,
    pub selected_tool: usize,
    pub is_focused_on_tools: bool,
    /// True while a background probe is in flight.
    pub loading: bool,
    /// Last probe result (config order). Empty until first probe completes.
    pub live: Vec<McpLiveServer>,
    pub probe_error: Option<String>,
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
    Max,
    XHigh,
    High,
    Medium,
    Low,
    Off,
}

impl From<ThinkingLevel> for ThinkingConfig {
    fn from(value: ThinkingLevel) -> Self {
        match value {
            ThinkingLevel::Max => Self::Max,
            ThinkingLevel::XHigh => Self::XHigh,
            ThinkingLevel::High => Self::High,
            ThinkingLevel::Medium => Self::Medium,
            ThinkingLevel::Low => Self::Low,
            ThinkingLevel::Off => Self::Off,
        }
    }
}

impl From<ThinkingConfig> for ThinkingLevel {
    fn from(value: ThinkingConfig) -> Self {
        match value {
            ThinkingConfig::Max => Self::Max,
            ThinkingConfig::XHigh => Self::XHigh,
            ThinkingConfig::High => Self::High,
            ThinkingConfig::Medium => Self::Medium,
            ThinkingConfig::Low => Self::Low,
            ThinkingConfig::Off => Self::Off,
        }
    }
}

impl ThinkingLevel {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Max => "max",
            Self::XHigh => "xhigh",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Off => "off",
        }
    }

    pub(crate) fn is_off(self) -> bool {
        matches!(self, Self::Off)
    }

    /// User-facing label for the effort picker / status bar.
    ///
    /// In binary mode (model has no registry effort levels) non-off levels are
    /// shown as "thinking on" and Off as "thinking off".
    pub(crate) fn display_label(self, binary_mode: bool) -> &'static str {
        navi_sdk::effort_display_label(self.into(), binary_mode)
    }

    pub(crate) fn from_config(value: &str) -> Self {
        ThinkingConfig::from_config_str(value).into()
    }

    pub(crate) fn config_value(self) -> &'static str {
        self.label()
    }

    pub(crate) fn index(self) -> usize {
        match self {
            Self::Max => 0,
            Self::XHigh => 1,
            Self::High => 2,
            Self::Medium => 3,
            Self::Low => 4,
            Self::Off => 5,
        }
    }

    /// Whether the given model uses binary off/on effort (no registry levels).
    pub(crate) fn is_binary_for_model(model: Option<&navi_sdk::ModelOption>) -> bool {
        let (supports, levels) = match model {
            Some(m) => (m.supports_thinking, m.reasoning_levels.as_slice()),
            None => (None, &[][..]),
        };
        navi_sdk::is_binary_effort_model(supports, levels)
    }

    /// Levels offered for the currently selected model (registry-aware).
    pub(crate) fn options_for_model(model: Option<&navi_sdk::ModelOption>) -> Vec<Self> {
        let (supports, levels) = match model {
            Some(m) => (m.supports_thinking, m.reasoning_levels.as_slice()),
            None => (None, &[][..]),
        };
        navi_sdk::thinking_levels_for_model(supports, levels)
            .into_iter()
            .map(Self::from)
            .collect()
    }

    /// Clamp `self` to a level the model supports; apply registry default if needed.
    pub(crate) fn resolve_for_model(self, model: Option<&navi_sdk::ModelOption>) -> Self {
        let (supports, levels, default) = match model {
            Some(m) => (
                m.supports_thinking,
                m.reasoning_levels.as_slice(),
                m.default_reasoning_effort.as_deref(),
            ),
            None => (None, &[][..], None),
        };
        navi_sdk::resolve_model_thinking_level(self.into(), supports, levels, default).into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectionState {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub active: bool,
    /// When set, free-form text selection stays inside this chat block
    /// (per-entry selection, no cross-block bleed).
    pub bound_source: Option<ChatLineSource>,
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

/// Live plan checklist shown in the topbar above the chat.
#[derive(Debug, Clone)]
pub(crate) struct ActivePlanUiState {
    pub plan_id: String,
    pub title: String,
    pub steps: Vec<ActivePlanStepUi>,
    /// `proposed` (awaiting review) | `active` | `completed` | `abandoned`
    pub status: String,
    /// When true, topbar expands to show the checklist (click summary to toggle).
    pub expanded: bool,
    /// When true (and expanded), show every step instead of the first N + "+more".
    pub show_all_steps: bool,
    /// When the plan first became fully done. Used to auto-dismiss after 1 minute.
    pub completed_at: Option<std::time::Instant>,
}

#[derive(Debug, Clone)]
pub(crate) struct ActivePlanStepUi {
    pub description: String,
    pub completed: bool,
}

impl ActivePlanUiState {
    /// How long a finished plan stays in the topbar before auto-dismiss.
    pub(crate) const DONE_DISMISS_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

    pub(crate) fn completed_count(&self) -> usize {
        self.steps.iter().filter(|s| s.completed).count()
    }

    pub(crate) fn total_count(&self) -> usize {
        self.steps.len()
    }

    /// Fully done (status completed, or every step checked).
    pub(crate) fn is_done(&self) -> bool {
        self.status == "completed"
            || (self.total_count() > 0 && self.completed_count() >= self.total_count())
    }

    /// Stamp `completed_at` the first time the plan becomes done.
    pub(crate) fn note_completed_if_needed(&mut self) {
        if self.is_done() && self.completed_at.is_none() {
            self.status = "completed".into();
            self.completed_at = Some(std::time::Instant::now());
        }
    }

    /// First incomplete step (current phase), if any.
    pub(crate) fn current_step(&self) -> Option<&ActivePlanStepUi> {
        self.steps.iter().find(|s| !s.completed)
    }

    pub(crate) fn mark_step_completed(&mut self, index: usize) {
        if let Some(step) = self.steps.get_mut(index) {
            step.completed = true;
        }
        if self.steps.iter().all(|s| s.completed) && !self.steps.is_empty() {
            self.status = "completed".into();
            self.note_completed_if_needed();
        }
    }

    /// True when a finished plan has been showing long enough to hide.
    pub(crate) fn should_auto_dismiss(&self) -> bool {
        self.completed_at
            .is_some_and(|t| t.elapsed() >= Self::DONE_DISMISS_AFTER)
    }
}
