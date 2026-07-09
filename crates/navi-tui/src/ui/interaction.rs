use crossterm::event::{KeyCode, KeyModifiers};

// Re-export the generic interaction primitives from copland.
pub use copland::interaction::{HitRegion, InteractionRegistry, line_rect};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitAction {
    Key {
        code: KeyCode,
        modifiers: KeyModifiers,
    },
    CloseModal,
    ReopenQuestion,
    OpenMessageQueue,
    QueuedMessage(usize),
    QuestionOption(usize),
    QuestionText,
    QuestionDeny,
    QuestionSend,
    Command(usize),
    Model(usize),
    ModelProviderRefresh(String),
    ProviderApiKey(usize),
    ProviderOAuth(usize),
    OAuthOpen,
    Session(usize),
    Skill(usize),
    Setting(usize),
    PluginInstallOrUpdate(usize),
    PluginRefresh,
    BackgroundCommand(usize),
    McpServer(usize),
    McpTool(usize),
    ToolApprove,
    ToolDeny,
    PluginApprove,
    PluginDeny,
    ThemeSelect(usize),
    ThemePicker,
    ChatMessage(usize),
    ToolResult(String),
    ToolGroup(Vec<String>),
    Subagent(String),
    MessageAction(usize),
    ScrollTo {
        target: ScrollTarget,
        offset: usize,
    },
    #[allow(dead_code)]
    RemoveImage(usize),
    /// Hover/preview a pending composer image (0-based index into `pending_images`).
    PreviewPendingImage(usize),
    /// Hover/preview an image on a sent chat message.
    PreviewChatImage {
        message_index: usize,
        image_index: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollTarget {
    Commands,
    Models,
    Providers,
    Sessions,
    Skills,
    Plugins,
    PluginApproval,
    QuestionOptions,
    BackgroundCommands,
    BackgroundCommandOutput,
    MessageQueue,
}
