use std::time::{Duration, Instant};

use navi_sdk::{QuestionRequest, ThinkingConfig, ToolInvocation, ToolResult};
use ratatui::layout::Rect;
use ratatui::text::Line;

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub signature: String,
    pub lines: Vec<Line<'static>>,
    pub chat_rect: Option<Rect>,
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
    Debug,
    Help,
    Skills,
    Plugins,
    PluginApproval,
    Question,
    ThemePicker,
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
    Debug,
    Help,
    Skills,
    Plugins,
    PluginApproval,
    Question,
    ThemePicker,
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
            Self::Debug => Mode::Debug,
            Self::Help => Mode::Help,
            Self::Skills => Mode::Skills,
            Self::Plugins => Mode::Plugins,
            Self::PluginApproval => Mode::PluginApproval,
            Self::Question => Mode::Question,
            Self::ThemePicker => Mode::ThemePicker,
        }
    }
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
