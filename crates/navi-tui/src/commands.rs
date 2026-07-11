#[derive(Debug, Clone, Copy)]
pub(crate) struct CommandItem {
    pub label: &'static str,
    pub shortcut: Option<&'static str>,
    pub action: CommandAction,
}

#[derive(Debug, Clone, Copy)]
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
    ReSetup,
    ClearGoal,
    AttachmentModels,
    Memory,
    Dream,
    TogglePlanMode,
    Help,
    About,
    CheckForUpdates,
    InstallUpdate,
}

pub(crate) const COMMANDS: &[CommandItem] = &[
    CommandItem {
        label: "New Session",
        shortcut: Some("ctrl+n"),
        action: CommandAction::NewSession,
    },
    CommandItem {
        label: "Sessions",
        shortcut: Some("ctrl+s"),
        action: CommandAction::Sessions,
    },
    CommandItem {
        label: "Copy Session",
        shortcut: None,
        action: CommandAction::CopySession,
    },
    CommandItem {
        label: "Share Session",
        shortcut: None,
        action: CommandAction::ShareSession,
    },
    CommandItem {
        label: "Models",
        shortcut: Some("ctrl+m"),
        action: CommandAction::SwitchModel,
    },
    CommandItem {
        label: "Providers",
        shortcut: None,
        action: CommandAction::Providers,
    },
    CommandItem {
        label: "Usage",
        shortcut: None,
        action: CommandAction::Usage,
    },
    CommandItem {
        label: "Skills",
        shortcut: None,
        action: CommandAction::Skills,
    },
    CommandItem {
        label: "Plugins",
        shortcut: None,
        action: CommandAction::Plugins,
    },
    CommandItem {
        label: "MCP Servers",
        shortcut: None,
        action: CommandAction::McpServers,
    },
    CommandItem {
        label: "Background Tasks",
        shortcut: Some("ctrl+t"),
        action: CommandAction::BackgroundCommands,
    },
    CommandItem {
        label: "Background Agents",
        shortcut: Some("ctrl+b"),
        action: CommandAction::BackgroundModels,
    },
    CommandItem {
        label: "Retry Last Response",
        shortcut: None,
        action: CommandAction::RetryLast,
    },
    CommandItem {
        label: "Effort Level",
        shortcut: None,
        action: CommandAction::OpenThinking,
    },
    CommandItem {
        label: "Compact Context",
        shortcut: None,
        action: CommandAction::Compact,
    },
    CommandItem {
        label: "Initialize Layer",
        shortcut: None,
        action: CommandAction::InitializeProject,
    },
    CommandItem {
        label: "Sync Models",
        shortcut: None,
        action: CommandAction::SyncModels,
    },
    CommandItem {
        label: "Settings",
        shortcut: None,
        action: CommandAction::Settings,
    },
    CommandItem {
        label: "Help",
        shortcut: Some("? / ctrl+."),
        action: CommandAction::Help,
    },
    CommandItem {
        label: "About",
        shortcut: None,
        action: CommandAction::About,
    },
    CommandItem {
        label: "Check for Updates",
        shortcut: None,
        action: CommandAction::CheckForUpdates,
    },
    CommandItem {
        label: "Install Update",
        shortcut: None,
        action: CommandAction::InstallUpdate,
    },
    CommandItem {
        label: "Quit",
        shortcut: None,
        action: CommandAction::Quit,
    },
    CommandItem {
        label: "Setup Wizard",
        shortcut: None,
        action: CommandAction::ReSetup,
    },
    CommandItem {
        label: "Clear Goal",
        shortcut: None,
        action: CommandAction::ClearGoal,
    },
    CommandItem {
        label: "Attachment Models",
        shortcut: None,
        action: CommandAction::AttachmentModels,
    },
    CommandItem {
        label: "Memory",
        shortcut: None,
        action: CommandAction::Memory,
    },
    CommandItem {
        label: "Run Dream",
        shortcut: None,
        action: CommandAction::Dream,
    },
    CommandItem {
        label: "Toggle Plan Mode",
        shortcut: None,
        action: CommandAction::TogglePlanMode,
    },
];

pub(crate) fn filtered_commands(app: &TuiApp) -> Vec<CommandItem> {
    let filter = app.command_filter.trim().to_lowercase();
    let commands = COMMANDS
        .iter()
        .copied()
        .filter(|command| filter.is_empty() || command.label.to_lowercase().contains(&filter))
        .collect::<Vec<_>>();

    if commands.is_empty() {
        COMMANDS.to_vec()
    } else {
        commands
    }
}
use crate::app::TuiApp;
