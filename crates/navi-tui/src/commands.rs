//! Hierarchical command palette: root hubs + sub-lists, with global search.

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
            Self::ModelRouting => "Model & routing",
            Self::Tools => "Tools",
            Self::Extensions => "Extensions",
            Self::HelpApp => "Help & app",
        }
    }

    #[allow(dead_code)]
    pub(crate) fn detail(self) -> &'static str {
        match self {
            Self::Session => "sessions, queue, compact, export",
            Self::ModelRouting => "models, effort, routing, usage",
            Self::Tools => "shell tasks, permissions",
            Self::Extensions => "skills, plugins, MCP",
            Self::HelpApp => "shortcuts, updates, about, quit",
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
    ShareSession,
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
    ClearGoal,
    AttachmentModels,
    TogglePlanMode,
    Help,
    About,
    CheckForUpdates,
    InstallUpdate,
    MessageQueue,
    ToggleYolo,
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
    Extension { index: usize },
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
        label: "Message Queue…",
        shortcut: Some("ctrl+q"),
        action: CommandAction::MessageQueue,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Retry Last Response",
        shortcut: None,
        action: CommandAction::RetryLast,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Compact Conversation",
        shortcut: None,
        action: CommandAction::Compact,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Toggle Plan Mode",
        shortcut: None,
        action: CommandAction::TogglePlanMode,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Copy Transcript",
        shortcut: None,
        action: CommandAction::CopySession,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Export Session JSON",
        shortcut: None,
        action: CommandAction::ShareSession,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Clear Goal",
        shortcut: None,
        action: CommandAction::ClearGoal,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::WhenGoalActive,
    },
    // Model & routing hub
    CommandItem {
        label: "Chat Model…",
        shortcut: Some("ctrl+m"),
        action: CommandAction::SwitchModel,
        hub: Some(CommandHub::ModelRouting),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Effort Level…",
        shortcut: None,
        action: CommandAction::OpenThinking,
        hub: Some(CommandHub::ModelRouting),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Model Routing…",
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
    // Search-only deep links into routing tabs
    CommandItem {
        label: "Agent Model Routes…",
        shortcut: None,
        action: CommandAction::BackgroundModels,
        hub: Some(CommandHub::ModelRouting),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Attachment Fallbacks…",
        shortcut: None,
        action: CommandAction::AttachmentModels,
        hub: Some(CommandHub::ModelRouting),
        visibility: CommandVisibility::Always,
    },
    // Tools hub
    CommandItem {
        label: "Shell Tasks…",
        shortcut: Some("ctrl+t"),
        action: CommandAction::BackgroundCommands,
        hub: Some(CommandHub::Tools),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Toggle YOLO",
        shortcut: Some("ctrl+g"),
        action: CommandAction::ToggleYolo,
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
        label: "MCP Servers…",
        shortcut: None,
        action: CommandAction::McpServers,
        hub: Some(CommandHub::Extensions),
        visibility: CommandVisibility::Always,
    },
    // Preferences (reachable via root Settings + search)
    CommandItem {
        label: "Settings…",
        shortcut: Some("ctrl+,"),
        action: CommandAction::Settings,
        hub: None,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Setup Wizard",
        shortcut: None,
        action: CommandAction::ReSetup,
        hub: Some(CommandHub::HelpApp),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Initialize Project",
        shortcut: None,
        action: CommandAction::InitializeProject,
        hub: Some(CommandHub::HelpApp),
        visibility: CommandVisibility::Always,
    },
    // Help & app hub
    CommandItem {
        label: "Keyboard Shortcuts",
        shortcut: Some("? / ctrl+."),
        action: CommandAction::Help,
        hub: Some(CommandHub::HelpApp),
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Check for Updates",
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
        label: "About NAVI",
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
    // Hot actions also listed for search (also on root)
    CommandItem {
        label: "New Session",
        shortcut: Some("ctrl+n"),
        action: CommandAction::NewSession,
        hub: Some(CommandHub::Session),
        visibility: CommandVisibility::Always,
    },
];

/// Root menu: hottest actions first, then hubs. Short list only.
const ROOT_ENTRIES: &[CommandItem] = &[
    CommandItem {
        label: "Chat Model…",
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
        label: "Model & routing →",
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
        label: "Help & app →",
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

/// All runnable actions matching the current filter (global search — ignores hub).
pub(crate) fn filtered_commands(app: &TuiApp) -> Vec<CommandItem> {
    let filter = app.command_filter.trim().to_lowercase();
    let mut commands = COMMANDS
        .iter()
        .copied()
        .filter(|command| is_visible(command, app))
        .filter(|command| filter.is_empty() || command.label.to_lowercase().contains(&filter))
        .collect::<Vec<_>>();

    // Dedup by action when search returns hub duplicates of root hot keys.
    if !filter.is_empty() {
        let mut seen = Vec::new();
        commands.retain(|c| {
            if seen.contains(&c.action) {
                false
            } else {
                seen.push(c.action);
                true
            }
        });
    }

    if commands.is_empty() && !filter.is_empty() {
        // Keep empty on search miss so the UI can show "no matches".
        return commands;
    }
    if commands.is_empty() {
        commands = ROOT_ENTRIES
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

    // Search always spans the full catalog (submodals included) + extension cmds.
    if !filter.is_empty() {
        let mut rows: Vec<CommandRow> = filtered_commands(app)
            .into_iter()
            .map(CommandRow::Item)
            .collect();
        for (index, ext) in app.extension_palette.iter().enumerate() {
            if ext.title.to_lowercase().contains(&filter_l)
                || ext.id.to_lowercase().contains(&filter_l)
                || ext.description.to_lowercase().contains(&filter_l)
            {
                rows.push(CommandRow::Extension { index });
            }
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
    if rows.is_empty() {
        0
    } else {
        0
    }
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

pub(crate) fn page_previous_command_row(_rows: &[CommandRow], current: usize, page: usize) -> usize {
    current.saturating_sub(page)
}

pub(crate) fn clamp_command_selection(rows: &[CommandRow], selected: usize) -> usize {
    if rows.is_empty() {
        0
    } else {
        selected.min(rows.len().saturating_sub(1))
    }
}
