//! Command palette catalog: grouped actions with optional visibility gates.

use crate::app::TuiApp;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CommandItem {
    pub label: &'static str,
    pub shortcut: Option<&'static str>,
    pub action: CommandAction,
    pub group: CommandGroup,
    pub visibility: CommandVisibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandGroup {
    Session,
    ModelRouting,
    Tools,
    Extensions,
    Preferences,
    HelpApp,
}

impl CommandGroup {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Session => "Session",
            Self::ModelRouting => "Model & routing",
            Self::Tools => "Tools & permissions",
            Self::Extensions => "Extensions",
            Self::Preferences => "Preferences",
            Self::HelpApp => "Help & app",
        }
    }

    fn order(self) -> u8 {
        match self {
            Self::Session => 0,
            Self::ModelRouting => 1,
            Self::Tools => 2,
            Self::Extensions => 3,
            Self::Preferences => 4,
            Self::HelpApp => 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandVisibility {
    Always,
    /// Only when an active goal is set.
    WhenGoalActive,
    /// Only when a pending self-update is known.
    WhenUpdateAvailable,
    /// Hidden in the default list; appears when the user types a filter match.
    /// Keeps hubs clean while preserving discoverability via search.
    SearchOnly,
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
    ExtensionsHub,
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
}

/// Visual row in the command palette (section headers + selectable items).
#[derive(Debug, Clone, Copy)]
pub(crate) enum CommandRow {
    Section(&'static str),
    Item(CommandItem),
}

impl CommandRow {
    pub(crate) fn is_selectable(self) -> bool {
        matches!(self, Self::Item(_))
    }

}

pub(crate) const COMMANDS: &[CommandItem] = &[
    // ── Session ──────────────────────────────────────────────────────────
    CommandItem {
        label: "New Session",
        shortcut: Some("ctrl+n"),
        action: CommandAction::NewSession,
        group: CommandGroup::Session,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Sessions…",
        shortcut: Some("ctrl+s"),
        action: CommandAction::Sessions,
        group: CommandGroup::Session,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Message Queue…",
        shortcut: Some("ctrl+q"),
        action: CommandAction::MessageQueue,
        group: CommandGroup::Session,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Retry Last Response",
        shortcut: None,
        action: CommandAction::RetryLast,
        group: CommandGroup::Session,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Compact Conversation",
        shortcut: None,
        action: CommandAction::Compact,
        group: CommandGroup::Session,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Toggle Plan Mode",
        shortcut: None,
        action: CommandAction::TogglePlanMode,
        group: CommandGroup::Session,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Clear Goal",
        shortcut: None,
        action: CommandAction::ClearGoal,
        group: CommandGroup::Session,
        visibility: CommandVisibility::WhenGoalActive,
    },
    CommandItem {
        label: "Copy Transcript",
        shortcut: None,
        action: CommandAction::CopySession,
        group: CommandGroup::Session,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Export Session JSON",
        shortcut: None,
        action: CommandAction::ShareSession,
        group: CommandGroup::Session,
        visibility: CommandVisibility::Always,
    },
    // ── Model & routing ──────────────────────────────────────────────────
    CommandItem {
        label: "Chat Model…",
        shortcut: Some("ctrl+m"),
        action: CommandAction::SwitchModel,
        group: CommandGroup::ModelRouting,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Effort Level…",
        shortcut: None,
        action: CommandAction::OpenThinking,
        group: CommandGroup::ModelRouting,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Model Routing…",
        shortcut: Some("ctrl+b"),
        action: CommandAction::ModelRouting,
        group: CommandGroup::ModelRouting,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Agent Model Routes…",
        shortcut: None,
        action: CommandAction::BackgroundModels,
        group: CommandGroup::ModelRouting,
        visibility: CommandVisibility::SearchOnly,
    },
    CommandItem {
        label: "Attachment Fallbacks…",
        shortcut: None,
        action: CommandAction::AttachmentModels,
        group: CommandGroup::ModelRouting,
        visibility: CommandVisibility::SearchOnly,
    },
    CommandItem {
        label: "Sync Models",
        shortcut: None,
        action: CommandAction::SyncModels,
        group: CommandGroup::ModelRouting,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Usage…",
        shortcut: None,
        action: CommandAction::Usage,
        group: CommandGroup::ModelRouting,
        visibility: CommandVisibility::Always,
    },
    // ── Tools & permissions ──────────────────────────────────────────────
    CommandItem {
        label: "Shell Tasks…",
        shortcut: Some("ctrl+t"),
        action: CommandAction::BackgroundCommands,
        group: CommandGroup::Tools,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Toggle YOLO",
        shortcut: Some("ctrl+g"),
        action: CommandAction::ToggleYolo,
        group: CommandGroup::Tools,
        visibility: CommandVisibility::Always,
    },
    // ── Extensions ───────────────────────────────────────────────────────
    CommandItem {
        label: "Extensions…",
        shortcut: None,
        action: CommandAction::ExtensionsHub,
        group: CommandGroup::Extensions,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Skills…",
        shortcut: None,
        action: CommandAction::Skills,
        group: CommandGroup::Extensions,
        visibility: CommandVisibility::SearchOnly,
    },
    CommandItem {
        label: "Plugins…",
        shortcut: None,
        action: CommandAction::Plugins,
        group: CommandGroup::Extensions,
        visibility: CommandVisibility::SearchOnly,
    },
    CommandItem {
        label: "MCP Servers…",
        shortcut: None,
        action: CommandAction::McpServers,
        group: CommandGroup::Extensions,
        visibility: CommandVisibility::SearchOnly,
    },
    // ── Preferences ──────────────────────────────────────────────────────
    CommandItem {
        label: "Settings…",
        shortcut: Some("ctrl+,"),
        action: CommandAction::Settings,
        group: CommandGroup::Preferences,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Providers…",
        shortcut: None,
        action: CommandAction::Providers,
        group: CommandGroup::Preferences,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Setup Wizard",
        shortcut: None,
        action: CommandAction::ReSetup,
        group: CommandGroup::Preferences,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Initialize Project",
        shortcut: None,
        action: CommandAction::InitializeProject,
        group: CommandGroup::Preferences,
        visibility: CommandVisibility::Always,
    },
    // ── Help & app ───────────────────────────────────────────────────────
    CommandItem {
        label: "Keyboard Shortcuts",
        shortcut: Some("? / ctrl+."),
        action: CommandAction::Help,
        group: CommandGroup::HelpApp,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "About NAVI",
        shortcut: None,
        action: CommandAction::About,
        group: CommandGroup::HelpApp,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Check for Updates",
        shortcut: None,
        action: CommandAction::CheckForUpdates,
        group: CommandGroup::HelpApp,
        visibility: CommandVisibility::Always,
    },
    CommandItem {
        label: "Install Update",
        shortcut: None,
        action: CommandAction::InstallUpdate,
        group: CommandGroup::HelpApp,
        visibility: CommandVisibility::WhenUpdateAvailable,
    },
    CommandItem {
        label: "Quit",
        shortcut: Some("ctrl+c"),
        action: CommandAction::Quit,
        group: CommandGroup::HelpApp,
        visibility: CommandVisibility::Always,
    },
];

fn is_visible(item: &CommandItem, app: &TuiApp, filtering: bool) -> bool {
    match item.visibility {
        CommandVisibility::Always => true,
        CommandVisibility::WhenGoalActive => app.goal_state.is_some(),
        CommandVisibility::WhenUpdateAvailable => app.available_update.is_some(),
        CommandVisibility::SearchOnly => filtering,
    }
}

/// Visible commands matching the current filter (no section headers).
pub(crate) fn filtered_commands(app: &TuiApp) -> Vec<CommandItem> {
    let filter = app.command_filter.trim().to_lowercase();
    let filtering = !filter.is_empty();
    let mut commands = COMMANDS
        .iter()
        .copied()
        .filter(|command| is_visible(command, app, filtering))
        .filter(|command| !filtering || command.label.to_lowercase().contains(&filter))
        .collect::<Vec<_>>();

    if commands.is_empty() {
        // Fall back to always-visible commands so the palette is never blank.
        commands = COMMANDS
            .iter()
            .copied()
            .filter(|c| c.visibility == CommandVisibility::Always)
            .collect();
    }
    commands
}

/// Rows for rendering: section headers when unfiltered; flat items while searching.
pub(crate) fn command_rows(app: &TuiApp) -> Vec<CommandRow> {
    let filter = app.command_filter.trim();
    let commands = filtered_commands(app);

    if !filter.is_empty() {
        return commands.into_iter().map(CommandRow::Item).collect();
    }

    let mut rows = Vec::with_capacity(commands.len() + 6);
    let mut last_group: Option<CommandGroup> = None;
    // Stable group order (commands are already ordered by group in COMMANDS).
    let mut sorted = commands;
    sorted.sort_by_key(|c| c.group.order());

    for command in sorted {
        if last_group != Some(command.group) {
            rows.push(CommandRow::Section(command.group.label()));
            last_group = Some(command.group);
        }
        rows.push(CommandRow::Item(command));
    }
    rows
}

/// Index of the first selectable row, or 0 if the list is empty.
pub(crate) fn first_selectable_command_row(rows: &[CommandRow]) -> usize {
    rows.iter()
        .position(|row| row.is_selectable())
        .unwrap_or(0)
}

/// Next selectable row after `current` (wrapping at end → stays on last selectable).
pub(crate) fn next_selectable_command_row(rows: &[CommandRow], current: usize) -> usize {
    if rows.is_empty() {
        return 0;
    }
    let start = current.saturating_add(1).min(rows.len());
    for i in start..rows.len() {
        if rows[i].is_selectable() {
            return i;
        }
    }
    // Stay on last selectable at or before current.
    rows.iter()
        .enumerate()
        .rev()
        .find_map(|(i, row)| row.is_selectable().then_some(i))
        .unwrap_or(0)
}

/// Previous selectable row before `current`.
pub(crate) fn previous_selectable_command_row(rows: &[CommandRow], current: usize) -> usize {
    if rows.is_empty() {
        return 0;
    }
    let end = current.min(rows.len());
    for i in (0..end).rev() {
        if rows[i].is_selectable() {
            return i;
        }
    }
    first_selectable_command_row(rows)
}

/// Page down: move roughly `page` selectable steps forward.
pub(crate) fn page_next_command_row(rows: &[CommandRow], current: usize, page: usize) -> usize {
    let mut idx = current;
    for _ in 0..page {
        let next = next_selectable_command_row(rows, idx);
        if next == idx {
            break;
        }
        idx = next;
    }
    idx
}

/// Page up: move roughly `page` selectable steps backward.
pub(crate) fn page_previous_command_row(rows: &[CommandRow], current: usize, page: usize) -> usize {
    let mut idx = current;
    for _ in 0..page {
        let prev = previous_selectable_command_row(rows, idx);
        if prev == idx {
            break;
        }
        idx = prev;
    }
    idx
}

/// Ensure `selected` points at a selectable row.
pub(crate) fn clamp_command_selection(rows: &[CommandRow], selected: usize) -> usize {
    if rows.is_empty() {
        return 0;
    }
    let selected = selected.min(rows.len().saturating_sub(1));
    if rows.get(selected).is_some_and(|r| r.is_selectable()) {
        return selected;
    }
    // Prefer next, then previous.
    let next = next_selectable_command_row(rows, selected.saturating_sub(1));
    if rows.get(next).is_some_and(|r| r.is_selectable()) {
        return next;
    }
    previous_selectable_command_row(rows, selected)
}
