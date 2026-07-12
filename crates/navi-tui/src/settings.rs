//! Settings hub: sectioned list of toggles, cycles, and deep-links to other modals.

use crate::TuiApp;
use crate::keybindings::global::{current_permission_mode, permission_mode_label};

/// Actionable (or navigable) settings entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingAction {
    ShowReasoning,
    CompactToolView,
    CompactToolRows,
    Theme,
    ChatModel,
    Effort,
    AgentRoutes,
    AttachmentFallbacks,
    Providers,
    PermissionMode,
    AutoUpdate,
    CheckUpdates,
    Debug,
    SetupWizard,
    MemoryHint,
}

/// Visual row in the Settings modal.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SettingRow {
    Section(&'static str),
    Action(SettingAction),
}

impl SettingRow {
    pub(crate) fn is_selectable(self) -> bool {
        matches!(self, Self::Action(_))
    }

}

/// Full settings list with section headers.
pub(crate) const SETTINGS_ROWS: &[SettingRow] = &[
    SettingRow::Section("Appearance"),
    SettingRow::Action(SettingAction::ShowReasoning),
    SettingRow::Action(SettingAction::CompactToolView),
    SettingRow::Action(SettingAction::CompactToolRows),
    SettingRow::Action(SettingAction::Theme),
    SettingRow::Section("Model routing"),
    SettingRow::Action(SettingAction::ChatModel),
    SettingRow::Action(SettingAction::Effort),
    SettingRow::Action(SettingAction::AgentRoutes),
    SettingRow::Action(SettingAction::AttachmentFallbacks),
    SettingRow::Section("Accounts"),
    SettingRow::Action(SettingAction::Providers),
    SettingRow::Section("Permissions"),
    SettingRow::Action(SettingAction::PermissionMode),
    SettingRow::Section("Memory"),
    SettingRow::Action(SettingAction::MemoryHint),
    SettingRow::Section("Updates"),
    SettingRow::Action(SettingAction::AutoUpdate),
    SettingRow::Action(SettingAction::CheckUpdates),
    SettingRow::Section("Advanced"),
    SettingRow::Action(SettingAction::Debug),
    SettingRow::Action(SettingAction::SetupWizard),
];

pub(crate) fn first_selectable_setting_row() -> usize {
    SETTINGS_ROWS
        .iter()
        .position(|row| row.is_selectable())
        .unwrap_or(0)
}

#[cfg(test)]
pub(crate) fn index_of_action(action: SettingAction) -> usize {
    SETTINGS_ROWS
        .iter()
        .position(|row| matches!(row, SettingRow::Action(a) if *a == action))
        .unwrap_or(0)
}

pub(crate) fn next_selectable_setting(current: usize) -> usize {
    let start = current.saturating_add(1).min(SETTINGS_ROWS.len());
    for i in start..SETTINGS_ROWS.len() {
        if SETTINGS_ROWS[i].is_selectable() {
            return i;
        }
    }
    SETTINGS_ROWS
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, row)| row.is_selectable().then_some(i))
        .unwrap_or(0)
}

pub(crate) fn previous_selectable_setting(current: usize) -> usize {
    let end = current.min(SETTINGS_ROWS.len());
    for i in (0..end).rev() {
        if SETTINGS_ROWS[i].is_selectable() {
            return i;
        }
    }
    first_selectable_setting_row()
}

pub(crate) fn clamp_setting_selection(selected: usize) -> usize {
    let selected = selected.min(SETTINGS_ROWS.len().saturating_sub(1));
    if SETTINGS_ROWS
        .get(selected)
        .is_some_and(|r| r.is_selectable())
    {
        return selected;
    }
    let next = next_selectable_setting(selected.saturating_sub(1));
    if SETTINGS_ROWS.get(next).is_some_and(|r| r.is_selectable()) {
        return next;
    }
    previous_selectable_setting(selected)
}

/// Label + right-hand value for a settings action.
pub(crate) fn setting_display(app: &TuiApp, action: SettingAction) -> (&'static str, String) {
    match action {
        SettingAction::ShowReasoning => (
            "Show Reasoning",
            if app.show_thinking {
                "[x]".into()
            } else {
                "[ ]".into()
            },
        ),
        SettingAction::CompactToolView => (
            "Compact Tool View",
            if !app.full_tool_view {
                "[x]".into()
            } else {
                "[ ]".into()
            },
        ),
        SettingAction::CompactToolRows => (
            "Compact Tool Rows",
            app.compact_tool_visible_limit.to_string(),
        ),
        SettingAction::Theme => ("Theme", format!("{} →", app.theme_id.label())),
        SettingAction::ChatModel => {
            let model = &app.loaded_config.config.model;
            (
                "Chat Model",
                format!("{}:{}  (Model Routing) →", model.provider, model.name),
            )
        }
        SettingAction::Effort => (
            "Effort Level",
            format!("{} →", app.thinking_level.label()),
        ),
        SettingAction::AgentRoutes => ("Agent Routes", "Model Routing →".into()),
        SettingAction::AttachmentFallbacks => ("Attachment Fallbacks", "Model Routing →".into()),
        SettingAction::Providers => ("Providers / Accounts", "open →".into()),
        SettingAction::PermissionMode => {
            let mode = current_permission_mode(app);
            (
                "Permission Mode",
                format!("{}  (cycle)", permission_mode_label(mode)),
            )
        }
        SettingAction::AutoUpdate => (
            "Auto-update NAVI",
            if app.loaded_config.config.updates.auto_update {
                "[x]".into()
            } else {
                "[ ]".into()
            },
        ),
        SettingAction::CheckUpdates => ("Check for Updates", "run →".into()),
        SettingAction::Debug => ("Debug", "open →".into()),
        SettingAction::SetupWizard => ("Setup Wizard", "restart →".into()),
        SettingAction::MemoryHint => {
            let cfg = &app.loaded_config.config.memory;
            let status = app
                .engine()
                .memory_quick_status()
                .unwrap_or_else(|_| {
                    format!(
                        "{} · dream {}d",
                        if cfg.enabled { "on" } else { "off" },
                        cfg.dream_interval_days
                    )
                });
            ("Memory", format!("{status}  ↵"))
        }
    }
}

/// Whether the value is a checkbox-style toggle (render `[x] label`).
pub(crate) fn is_checkbox(action: SettingAction) -> bool {
    matches!(
        action,
        SettingAction::ShowReasoning
            | SettingAction::CompactToolView
            | SettingAction::AutoUpdate
    )
}


