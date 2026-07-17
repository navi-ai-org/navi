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
    RemoveQueuedMessage(usize),
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
    /// Extensions hub row (Skills / Plugins / MCP).
    ExtensionsItem(usize),
    PluginInstallOrUpdate(usize),
    PluginRefresh,
    /// Open background-task output (chevron / card body).
    BackgroundCommandOpen(usize),
    /// Cancel a running background task (✕ control).
    BackgroundCommandCancel(usize),
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
    /// Return keyboard focus to the main composer.
    ///
    /// This is a dedicated hit instead of relying on the empty-space fallback:
    /// the fallback is intentionally shared with chat drag selection, while a
    /// click inside the composer must always restore the input cursor.
    FocusComposer,
    MessageAction(usize),
    /// Select a checkpoint row in the Rewind modal.
    RewindCheckpoint(usize),
    ScrollTo {
        target: ScrollTarget,
        offset: usize,
    },
    /// Jump chat scrollback to the latest message (bottom / follow tail).
    ScrollToBottom,
    /// Hover/click the composer context-usage chip (`3 / 200k` → show %).
    ContextUsage,
    #[allow(dead_code)]
    RemoveImage(usize),
    /// Hover/preview a pending composer image (0-based index into `pending_images`).
    PreviewPendingImage(usize),
    /// Hover/preview an image on a sent chat message.
    PreviewChatImage {
        message_index: usize,
        image_index: usize,
    },
    /// Full lightbox body: keep the image preview open while the cursor is on it.
    /// Registered above chat hits so content under the modal does not steal hover.
    ImageLightboxKeep,
    /// Select a row in the Help cheatsheet modal.
    HelpRow(usize),
    /// About modal link row.
    AboutLink(usize),
    /// Composer chip: open pending update modal.
    OpenUpdateAvailable,
    /// Plan review: click a plan body line (0-based view line index).
    PlanReviewLine(usize),
    PlanReviewApprove,
    PlanReviewChanges,
    PlanReviewComment,
    PlanReviewQuit,
    /// Toggle the plan progress topbar (compact N/M ↔ expanded checklist).
    TogglePlanTopbar,
    /// Expand remaining plan steps after "+N more" in the plan topbar.
    ExpandPlanMore,
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
    /// Agent model routes list (Model Routing → Agents / legacy modal).
    BackgroundModels,
    MessageQueue,
    Help,
    PathMentions,
    /// Rewind checkpoint list.
    Rewind,
}
