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
    Agent,
    SwitchModel,
    RetryLast,
    OpenThinking,
    Compact,
    InitializeProject,
    SyncModels,
    Providers,
    Quit,
    Settings,
    Skills,
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
        label: "Agent",
        shortcut: Some("tab"),
        action: CommandAction::Agent,
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
        label: "Skills",
        shortcut: None,
        action: CommandAction::Skills,
    },
    CommandItem {
        label: "Retry Last Response",
        shortcut: None,
        action: CommandAction::RetryLast,
    },
    CommandItem {
        label: "Thinking Mode",
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
        label: "Quit",
        shortcut: Some("ctrl+c"),
        action: CommandAction::Quit,
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
