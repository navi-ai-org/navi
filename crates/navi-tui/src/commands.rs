//! Hierarchical command palette: root hubs + sub-lists, with global search.
//!
//! Search matches labels, shortcuts, and keyword aliases across the full
//! catalog (including hub-only actions and settings deep-links). Results are
//! ranked so prefix hits beat mid-string matches — muscle-memory queries like
//! `m` → Model and `the` → Theme stay snappy.

use crate::app::TuiApp;

/// Top-level and hub groupings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandHub {
    Session,
    ModelRouting,
    Tools,
    Extensions,
    HelpApp,
}

impl CommandHub {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Session => "Session",
            Self::ModelRouting => "Models",
            Self::Tools => "Tools",
            Self::Extensions => "Extensions",
            Self::HelpApp => "Help",
        }
    }

    #[allow(dead_code)]
    pub(crate) fn detail(self) -> &'static str {
        match self {
            Self::Session => "sessions, queue, compact, export",
            Self::ModelRouting => "model, effort, routing, usage, providers",
            Self::Tools => "shell tasks, permissions, yolo",
            Self::Extensions => "skills, plugins, mcp",
            Self::HelpApp => "shortcuts, theme, updates, about, quit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandVisibility {
    Always,
    WhenGoalActive,
    WhenUpdateAvailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandAction {
    NewSession,
    Sessions,
    CopySession,
    /// Copy assistant/tool output after the latest user message.
    CopyLastResponse,
    ShareSession,
    /// Open rewind modal: pick a past user prompt and restore history + files.
    Rewind,
    SwitchModel,
    RetryLast,
    OpenThinking,
    Compact,
    InitializeProject,
    SyncModels,
    Providers,
    Usage,
    Quit,
    Settings,
    Skills,
    Plugins,
    McpServers,
    BackgroundCommands,
    BackgroundModels,
    ModelRouting,
    ReSetup,
    /// Set the current input as a multi-turn session goal (auto-continue).
    SetGoal,
    /// Pause auto-continuation for the active goal.
    PauseGoal,
    /// Resume a paused goal.
    ResumeGoal,
    ClearGoal,
    AttachmentModels,
    TogglePlanMode,
    Help,
    About,
    CheckForUpdates,
    InstallUpdate,
    MessageQueue,
    ToggleYolo,
    /// Appearance → theme picker (settings deep-link, searchable as "theme").
    Theme,
    /// Open debug modal (settings deep-link).
    Debug,
    /// Toggle show-reasoning / thinking text in chat.
    ToggleShowReasoning,
    ToggleDesktopNotifications,
    /// Cycle permission mode (restricted / accept-edits / auto / yolo).
    CyclePermissionMode,
    /// Open a nested hub list inside the command palette.
    OpenHub(CommandHub),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CommandItem {
    pub label: &'static str,
    pub shortcut: Option<&'static str>,
    pub action: CommandAction,
    pub hub: Option<CommandHub>,
    pub visibility: CommandVisibility,
}

/// Row shown in the palette list (always selectable — no section headers).
#[derive(Debug, Clone)]
pub(crate) enum CommandRow {
    Item(CommandItem),
    /// Host-mediated extension command from installed `tui.json`.
    Extension {
        index: usize,
    },
}

impl CommandRow {
    pub(crate) fn is_selectable(&self) -> bool {
        true
    }

    #[allow(dead_code)]
    pub(crate) fn item(self) -> Option<CommandItem> {
        match self {
            Self::Item(item) => Some(item),
            Self::Extension { .. } => None,
        }
    }
}

// ── Full action catalog (hubs + global search) ───────────────────────────────
// Order inside each hub = most used first.
// Labels are short and start with the word people type (Model, Theme, …).

pub(crate) const COMMANDS: &[CommandItem] = &[
    // Session hub
    CommandItem {
        label: "Sessions…",
        shortcut: Some("ctrl+s"),
        action: CommandAction::Sessions,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Queue…",
        shortcut: Some("ctrl+q"),
        action: CommandAction::MessageQueue,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Retry",
        shortcut: None,
        action: CommandAction::RetryLast,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Rewind…",
        shortcut: None,
        action: CommandAction::Rewind,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Compact",
        shortcut: None,
        action: CommandAction::Compact,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Plan Mode",
        shortcut: None,
        action: CommandAction::TogglePlanMode,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Copy Session",
        shortcut: None,
        action: CommandAction::CopySession,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Copy Last Response",
        shortcut: None,
        action: CommandAction::CopyLastResponse,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Export JSON",
        shortcut: None,
        action: CommandAction::ShareSession,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Set Goal",
        shortcut: None,
        action: CommandAction::SetGoal,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Pause Goal",
        shortcut: None,
        action: CommandAction::PauseGoal,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::WhenGoalActive,
    },
    CommandItem {
        label: "Resume Goal",
        shortcut: None,
        action: CommandAction::ResumeGoal,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::WhenGoalActive,
    },
    CommandItem {
        label: "Clear Goal",
        shortcut: None,
        action: CommandAction::ClearGoal,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::WhenGoalActive,
    },
    CommandItem {
        label: "New Session",
        shortcut: Some("ctrl+n"),
        action: CommandAction::NewSession,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
    // Models hub
    CommandItem {
        label: "Model…",
        shortcut: Some("ctrl+m"),
        action: CommandAction::SwitchModel,
        hub: Some(CommandHub::ModelRouting),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Effort…",
        shortcut: None,
        action: CommandAction::OpenThinking,
        hub: Some(CommandHub::ModelRouting),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Routing…",
        shortcut: Some("ctrl+b"),
        action: CommandAction::ModelRouting,
        hub: Some(CommandHub::ModelRouting),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Sync Models",
        shortcut: None,
        action: CommandAction::SyncModels,
        hub: Some(CommandHub::ModelRouting),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Usage…",
        shortcut: None,
        action: CommandAction::Usage,
        hub: Some(CommandHub::ModelRouting),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Providers…",
        shortcut: None,
        action: CommandAction::Providers,
        hub: Some(CommandHub::ModelRouting),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Agent Routes…",
        shortcut: None,
        action: CommandAction::BackgroundModels,
        hub: Some(CommandHub::ModelRouting),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Attachments…",
        shortcut: None,
        action: CommandAction::AttachmentModels,
        hub: Some(CommandHub::ModelRouting),
        visibility: CommandVisibility::Always,
    },
    // Tools hub
    CommandItem {
        label: "Tasks…",
        shortcut: Some("ctrl+t"),
        action: CommandAction::BackgroundCommands,
        hub: Some(CommandHub::Tools),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "YOLO",
        shortcut: Some("ctrl+g"),
        action: CommandAction::ToggleYolo,
        hub: Some(CommandHub::Tools),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Permissions",
        shortcut: None,
        action: CommandAction::CyclePermissionMode,
        hub: Some(CommandHub::Tools),
        visibility: CommandVisibility::Always,
    },
    // Extensions hub
    CommandItem {
        label: "Skills…",
        shortcut: None,
        action: CommandAction::Skills,
        hub: Some(CommandHub::Extensions),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Plugins…",
        shortcut: None,
        action: CommandAction::Plugins,
        hub: Some(CommandHub::Extensions),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "MCP…",
        shortcut: None,
        action: CommandAction::McpServers,
        hub: Some(CommandHub::Extensions),
        visibility: CommandVisibility::Always,
    },
    // Preferences / appearance (also under Settings; listed for search)
    CommandItem {
        label: "Settings…",
        shortcut: Some("ctrl+,"),
        action: CommandAction::Settings,
        hub: None,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Theme…",
        shortcut: None,
        action: CommandAction::Theme,
        hub: Some(CommandHub::HelpApp),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Show Reasoning",
        shortcut: None,
        action: CommandAction::ToggleShowReasoning,
        hub: Some(CommandHub::HelpApp),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Desktop Notifications",
        shortcut: None,
        action: CommandAction::ToggleDesktopNotifications,
        hub: Some(CommandHub::HelpApp),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Debug…",
        shortcut: Some("ctrl+d"),
        action: CommandAction::Debug,
        hub: Some(CommandHub::HelpApp),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Setup",
        shortcut: None,
        action: CommandAction::ReSetup,
        hub: Some(CommandHub::HelpApp),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Init Project",
        shortcut: None,
        action: CommandAction::InitializeProject,
        hub: Some(CommandHub::HelpApp),
        visibility: CommandVisibility::Always,
    },
    // Help hub
    CommandItem {
        label: "Shortcuts",
        shortcut: Some("? / ctrl+."),
        action: CommandAction::Help,
        hub: Some(CommandHub::HelpApp),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Updates",
        shortcut: None,
        action: CommandAction::CheckForUpdates,
        hub: Some(CommandHub::HelpApp),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Install Update",
        shortcut: None,
        action: CommandAction::InstallUpdate,
        hub: Some(CommandHub::HelpApp),
        visibility: CommandVisibility::WhenUpdateAvailable,
    },
    CommandItem {
        label: "About",
        shortcut: None,
        action: CommandAction::About,
        hub: Some(CommandHub::HelpApp),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Quit",
        shortcut: Some("ctrl+c"),
        action: CommandAction::Quit,
        hub: Some(CommandHub::HelpApp),
        visibility: CommandVisibility::Always,
    },
];

/// Root menu: hottest actions first, then hubs. Short list only.
const ROOT_ENTRIES: &[CommandItem] = &[
    CommandItem {
        label: "Model…",
        shortcut: Some("ctrl+m"),
        action: CommandAction::SwitchModel,
        hub: None,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Sessions…",
        shortcut: Some("ctrl+s"),
        action: CommandAction::Sessions,
        hub: None,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "New Session",
        shortcut: Some("ctrl+n"),
        action: CommandAction::NewSession,
        hub: None,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Session →",
        shortcut: None,
        action: CommandAction::OpenHub(CommandHub::Session),
        hub: None,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Models →",
        shortcut: None,
        action: CommandAction::OpenHub(CommandHub::ModelRouting),
        hub: None,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Tools →",
        shortcut: None,
        action: CommandAction::OpenHub(CommandHub::Tools),
        hub: None,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Extensions →",
        shortcut: None,
        action: CommandAction::OpenHub(CommandHub::Extensions),
        hub: None,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Settings…",
        shortcut: Some("ctrl+,"),
        action: CommandAction::Settings,
        hub: None,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Help →",
        shortcut: None,
        action: CommandAction::OpenHub(CommandHub::HelpApp),
        hub: None,
        visibility: CommandVisibility::Always,
    },
];

fn is_visible(item: &CommandItem, app: &TuiApp) -> bool {
    match item.visibility {
        CommandVisibility::Always => true,
        CommandVisibility::WhenGoalActive => app.goal_state.is_some(),
        CommandVisibility::WhenUpdateAvailable => app.available_update.is_some(),
    }
}

/// Extra search tokens (not shown in the UI) so short queries hit deep items.
fn action_keywords(action: CommandAction) -> &'static str {
    match action {
        CommandAction::SwitchModel => "chat model llm ai picker switch",
        CommandAction::OpenThinking => "thinking reason effort level binary",
        CommandAction::ModelRouting => "routing routes background models agents",
        CommandAction::BackgroundModels => "agent routes subagent model",
        CommandAction::AttachmentModels => "attachment image audio video fallback vision",
        CommandAction::SyncModels => "refresh models catalog registry",
        CommandAction::Providers => "accounts api key oauth credentials login",
        CommandAction::Usage => "credits tokens cost billing hypercredits rate limit",
        CommandAction::Sessions => "history resume load",
        CommandAction::NewSession => "clear reset conversation",
        CommandAction::MessageQueue => "queue pending messages",
        CommandAction::RetryLast => "retry regenerate redo",
        CommandAction::Rewind => "rewind undo restore checkpoint revert files history grok",
        CommandAction::Compact => "summarize context compress",
        CommandAction::TogglePlanMode => "plan mode planning",
        CommandAction::CopySession => "copy clipboard transcript share session full",
        CommandAction::CopyLastResponse => {
            "copy clipboard last response output turn since user message"
        }
        CommandAction::ShareSession => "export json dump",
        CommandAction::SetGoal => "goal set objective start auto continue",
        CommandAction::PauseGoal => "goal pause stop auto continue",
        CommandAction::ResumeGoal => "goal resume continue active",
        CommandAction::ClearGoal => "goal clear stop",
        CommandAction::BackgroundCommands => "shell tasks background jobs processes",
        CommandAction::ToggleYolo => "yolo unrestricted auto approve dangerous",
        CommandAction::CyclePermissionMode => {
            "permissions permission mode restricted accept edits auto yolo security"
        }
        CommandAction::Skills => "skills skill",
        CommandAction::Plugins => "plugins plugin marketplace wasm",
        CommandAction::McpServers => "mcp model context protocol servers tools",
        CommandAction::Settings => "preferences options config",
        CommandAction::Theme => "theme themes colors appearance dark light lain palette ui",
        CommandAction::ToggleShowReasoning => "reasoning thinking show hide thought chain",
        CommandAction::ToggleDesktopNotifications => "desktop notifications toast notify unfocused",
        CommandAction::Debug => "debug diagnostics logs status",
        CommandAction::ReSetup => "setup wizard onboarding",
        CommandAction::InitializeProject => "init project navi config",
        CommandAction::Help => "help shortcuts keys keyboard keymap bindings",
        CommandAction::About => "about version",
        CommandAction::CheckForUpdates => "update upgrades release",
        CommandAction::InstallUpdate => "install update upgrade",
        CommandAction::Quit => "quit exit leave",
        CommandAction::OpenHub(hub) => hub.detail(),
    }
}

/// Lower is better. `None` = no match.
fn match_score(text: &str, filter: &str) -> Option<u8> {
    if filter.is_empty() {
        return Some(0);
    }
    let text = text.to_lowercase();
    if text.starts_with(filter) {
        return Some(0);
    }
    // Word-prefix: "the" → "Theme…", "msg" → "Message Queue" if present.
    if text
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| !word.is_empty() && word.starts_with(filter))
    {
        return Some(1);
    }
    if text.contains(filter) {
        return Some(2);
    }
    None
}

fn command_match_score(item: &CommandItem, filter: &str) -> Option<u8> {
    if filter.is_empty() {
        return Some(0);
    }
    let mut best: Option<u8> = match_score(item.label, filter);
    if let Some(shortcut) = item.shortcut {
        best = min_score(best, match_score(shortcut, filter));
    }
    best = min_score(best, match_score(action_keywords(item.action), filter));
    // Soft hub-name boost only (not hub.detail — that starts with "skills,"
    // and would make every Extensions item rank equal for "skills").
    if let Some(hub) = item.hub {
        if let Some(s) = match_score(hub.label(), filter) {
            // Never beat a direct label/keyword hit.
            best = min_score(best, Some(s.saturating_add(2).min(4)));
        }
    }
    best
}

fn min_score(a: Option<u8>, b: Option<u8>) -> Option<u8> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

/// All runnable actions matching the current filter (global search — full catalog).
pub(crate) fn filtered_commands(app: &TuiApp) -> Vec<CommandItem> {
    let filter = app.command_filter.trim().to_lowercase();
    let mut scored: Vec<(u8, CommandItem)> = COMMANDS
        .iter()
        .copied()
        .filter(|command| is_visible(command, app))
        .filter_map(|command| command_match_score(&command, &filter).map(|score| (score, command)))
        .collect();

    // Dedup by action (root hot keys + hub copies).
    let mut seen = Vec::new();
    scored.retain(|(_, c)| {
        if seen.contains(&c.action) {
            false
        } else {
            seen.push(c.action);
            true
        }
    });

    // Rank: better score first; prefer hotkeys (ctrl+m Model beats short "MCP…");
    // then shorter label; then alpha.
    scored.sort_by(|(sa, a), (sb, b)| {
        sa.cmp(sb)
            .then_with(|| match (a.shortcut.is_some(), b.shortcut.is_some()) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.label.len().cmp(&b.label.len()))
            .then_with(|| a.label.cmp(b.label))
    });

    let commands: Vec<CommandItem> = scored.into_iter().map(|(_, c)| c).collect();

    if commands.is_empty() && !filter.is_empty() {
        return commands;
    }
    if commands.is_empty() {
        return ROOT_ENTRIES
            .iter()
            .copied()
            .filter(|c| is_visible(c, app))
            .collect();
    }
    commands
}

/// Rows for the palette: global search, hub list, or root menu.
pub(crate) fn command_rows(app: &TuiApp) -> Vec<CommandRow> {
    let filter = app.command_filter.trim();
    let filter_l = filter.to_lowercase();

    // Typing always searches the full catalog (hubs + deep links + extensions).
    if !filter.is_empty() {
        let mut rows: Vec<CommandRow> = filtered_commands(app)
            .into_iter()
            .map(CommandRow::Item)
            .collect();
        let mut ext_hits: Vec<(u8, usize)> = Vec::new();
        for (index, ext) in app.extension_palette.iter().enumerate() {
            let score = match_score(&ext.title, &filter_l)
                .or_else(|| match_score(&ext.id, &filter_l))
                .or_else(|| match_score(&ext.description, &filter_l));
            if let Some(score) = score {
                ext_hits.push((score, index));
            }
        }
        ext_hits.sort_by_key(|(s, _)| *s);
        for (_, index) in ext_hits {
            rows.push(CommandRow::Extension { index });
        }
        return rows;
    }

    if let Some(hub) = app.command_hub {
        let mut rows: Vec<CommandRow> = COMMANDS
            .iter()
            .copied()
            .filter(|c| c.hub == Some(hub) && is_visible(c, app))
            .map(CommandRow::Item)
            .collect();
        if hub == CommandHub::Extensions {
            for index in 0..app.extension_palette.len() {
                rows.push(CommandRow::Extension { index });
            }
        }
        return rows;
    }

    ROOT_ENTRIES
        .iter()
        .copied()
        .filter(|c| is_visible(c, app))
        .map(CommandRow::Item)
        .collect()
}

pub(crate) fn palette_title(app: &TuiApp) -> String {
    if !app.command_filter.trim().is_empty() {
        return "Commands · search".into();
    }
    match app.command_hub {
        Some(hub) => format!("Commands · {}", hub.label()),
        None => "Commands".into(),
    }
}

pub(crate) fn first_selectable_command_row(rows: &[CommandRow]) -> usize {
    if rows.is_empty() { 0 } else { 0 }
}

pub(crate) fn next_selectable_command_row(rows: &[CommandRow], current: usize) -> usize {
    if rows.is_empty() {
        return 0;
    }
    (current + 1).min(rows.len().saturating_sub(1))
}

pub(crate) fn previous_selectable_command_row(_rows: &[CommandRow], current: usize) -> usize {
    current.saturating_sub(1)
}

pub(crate) fn page_next_command_row(rows: &[CommandRow], current: usize, page: usize) -> usize {
    if rows.is_empty() {
        return 0;
    }
    (current + page).min(rows.len().saturating_sub(1))
}

pub(crate) fn page_previous_command_row(
    _rows: &[CommandRow],
    current: usize,
    page: usize,
) -> usize {
    current.saturating_sub(page)
}

pub(crate) fn clamp_command_selection(rows: &[CommandRow], selected: usize) -> usize {
    if rows.is_empty() {
        0
    } else {
        selected.min(rows.len().saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::test_app;

    #[test]
    fn search_m_prefers_model_over_longer_matches() {
        let mut app = test_app("");
        app.command_filter = "m".into();
        let cmds = filtered_commands(&app);
        assert!(!cmds.is_empty(), "expected matches for 'm'");
        assert!(
            matches!(cmds[0].action, CommandAction::SwitchModel),
            "first hit for 'm' should be Model…, got {:?}",
            cmds[0].label
        );
    }

    #[test]
    fn search_the_finds_theme() {
        let mut app = test_app("");
        app.command_filter = "the".into();
        let cmds = filtered_commands(&app);
        assert!(
            cmds.iter()
                .any(|c| matches!(c.action, CommandAction::Theme)),
            "Theme should appear for query 'the', got: {:?}",
            cmds.iter().map(|c| c.label).collect::<Vec<_>>()
        );
        assert!(
            matches!(cmds[0].action, CommandAction::Theme),
            "Theme should rank first for 'the', got {:?}",
            cmds[0].label
        );
    }

    #[test]
    fn search_reaches_hub_only_actions() {
        let mut app = test_app("");
        // "compact" lives under Session hub, not root.
        app.command_filter = "compact".into();
        let cmds = filtered_commands(&app);
        assert!(
            cmds.iter()
                .any(|c| matches!(c.action, CommandAction::Compact)),
            "hub action Compact must be reachable via global search"
        );
    }

    #[test]
    fn root_lists_model_not_chat_model() {
        let app = test_app("");
        let rows = command_rows(&app);
        let labels: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                CommandRow::Item(i) => Some(i.label),
                _ => None,
            })
            .collect();
        assert!(
            labels.iter().any(|l| l.starts_with("Model")),
            "root should list Model…, got {labels:?}"
        );
        assert!(
            labels.iter().all(|l| !l.contains("Chat Model")),
            "Chat Model label must be gone, got {labels:?}"
        );
    }
}
