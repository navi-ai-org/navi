use std::time::{Duration, Instant};

use navi_sdk::{ThinkingConfig, ToolInvocation, ToolResult};
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThinkingLevel {
    Max,
    High,
    Medium,
    Low,
    Off,
}

impl From<ThinkingLevel> for ThinkingConfig {
    fn from(value: ThinkingLevel) -> Self {
        match value {
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
            Self::Max => "max",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Off => "off",
        }
    }

    pub(crate) fn from_config(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "max" => Self::Max,
            "medium" => Self::Medium,
            "low" => Self::Low,
            "off" => Self::Off,
            _ => Self::High,
        }
    }

    pub(crate) fn config_value(self) -> &'static str {
        self.label()
    }

    pub(crate) fn index(self) -> usize {
        match self {
            Self::Max => 0,
            Self::High => 1,
            Self::Medium => 2,
            Self::Low => 3,
            Self::Off => 4,
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
