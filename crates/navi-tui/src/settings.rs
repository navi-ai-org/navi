//! Settings hub: sectioned list of toggles, cycles, and deep-links to other modals.
//!
//! Keep rows short and scannable — label on the left, state on the right.
//! No redundant "open →", "Model Routing →", or parenthetical action hints.

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
    SettingRow::Section("Model"),
    SettingRow::Action(SettingAction::ChatModel),
    SettingRow::Action(SettingAction::Effort),
    SettingRow::Action(SettingAction::AgentRoutes),
    SettingRow::Action(SettingAction::AttachmentFallbacks),
    SettingRow::Section("System"),
    SettingRow::Action(SettingAction::Providers),
    SettingRow::Action(SettingAction::PermissionMode),
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

/// How a setting value is presented on the right-hand side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingValueKind {
    /// `● / ○ Label` — toggle, no separate value column.
    Toggle,
    /// `Label          value ›` — opens another modal.
    Link,
    /// `Label          value` — enter cycles or shows plain state.
    Cycle,
}

/// Label + right-hand value + presentation kind.
pub(crate) fn setting_display(
    app: &TuiApp,
    action: SettingAction,
) -> (&'static str, String, SettingValueKind) {
    match action {
        SettingAction::ShowReasoning => (
            "Show reasoning",
            if app.show_thinking {
                "on".into()
            } else {
                "off".into()
            },
            SettingValueKind::Toggle,
        ),
        SettingAction::CompactToolView => (
            "Compact tools",
            if !app.full_tool_view {
                "on".into()
            } else {
                "off".into()
            },
            SettingValueKind::Toggle,
        ),
        SettingAction::CompactToolRows => (
            "Tool rows",
            app.compact_tool_visible_limit.to_string(),
            SettingValueKind::Cycle,
        ),
        SettingAction::Theme => (
            "Theme",
            app.theme_id.label().to_string(),
            SettingValueKind::Link,
        ),
        SettingAction::ChatModel => {
            let model = &app.loaded_config.config.model;
            (
                "Model",
                short_model_label(&model.provider, &model.name),
                SettingValueKind::Link,
            )
        }
        SettingAction::Effort => (
            "Effort",
            app.thinking_level.label().to_string(),
            SettingValueKind::Link,
        ),
        SettingAction::AgentRoutes => ("Agent routes", String::new(), SettingValueKind::Link),
        SettingAction::AttachmentFallbacks => {
            ("Attachments", String::new(), SettingValueKind::Link)
        }
        SettingAction::Providers => ("Providers", String::new(), SettingValueKind::Link),
        SettingAction::PermissionMode => {
            let mode = current_permission_mode(app);
            (
                "Permissions",
                permission_mode_label(mode).to_string(),
                SettingValueKind::Cycle,
            )
        }
        SettingAction::AutoUpdate => (
            "Auto-update",
            if app.loaded_config.config.updates.auto_update {
                "on".into()
            } else {
                "off".into()
            },
            SettingValueKind::Toggle,
        ),
        SettingAction::CheckUpdates => ("Check for updates", String::new(), SettingValueKind::Link),
        SettingAction::Debug => ("Debug", String::new(), SettingValueKind::Link),
        SettingAction::SetupWizard => ("Setup wizard", String::new(), SettingValueKind::Link),
        SettingAction::MemoryHint => {
            let cfg = &app.loaded_config.config.memory;
            let status = app.engine().memory_quick_status().unwrap_or_else(|_| {
                if cfg.enabled {
                    "on".into()
                } else {
                    "off".into()
                }
            });
            ("Memory", short_memory_status(&status), SettingValueKind::Link)
        }
    }
}

/// Format a settings row for the list (aligned label / value columns).
pub(crate) fn format_setting_line(
    label: &str,
    value: &str,
    kind: SettingValueKind,
    col_width: usize,
) -> String {
    let label_w = col_width.clamp(12, 22);
    match kind {
        SettingValueKind::Toggle => {
            let mark = if value == "on" { "●" } else { "○" };
            format!("{mark}  {label}")
        }
        SettingValueKind::Link if value.is_empty() => {
            format!("{label:<label_w$}  ›")
        }
        SettingValueKind::Link => {
            format!("{label:<label_w$}  {value}  ›")
        }
        SettingValueKind::Cycle => {
            if value.is_empty() {
                label.to_string()
            } else {
                format!("{label:<label_w$}  {value}")
            }
        }
    }
}

/// Prefer just the model id; drop long `provider:org/` prefixes when space is tight.
fn short_model_label(provider: &str, name: &str) -> String {
    let short_name = name.rsplit('/').next().unwrap_or(name);
    // If provider is obvious from context or name is unique enough, show model only.
    if short_name.len() <= 28 {
        short_name.to_string()
    } else {
        format!("{provider}:{short_name}")
    }
}

/// Collapse verbose memory doctor strings into a scannable status.
fn short_memory_status(status: &str) -> String {
    // Typical: "on · 1 active · embeddings ready"
    let lower = status.to_ascii_lowercase();
    if lower.contains("off") {
        return "off".into();
    }
    // Keep first two clauses max.
    let parts: Vec<&str> = status.split('·').map(str::trim).collect();
    match parts.as_slice() {
        [] => status.to_string(),
        [a] => (*a).to_string(),
        [a, b, ..] => format!("{a} · {b}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_model_label_strips_org_path() {
        assert_eq!(
            short_model_label("commandcode", "xiaomi/mimo-v2.5-pro"),
            "mimo-v2.5-pro"
        );
        assert_eq!(short_model_label("openai", "gpt-5.5"), "gpt-5.5");
    }

    #[test]
    fn format_toggle_and_link_rows_are_compact() {
        let on = format_setting_line("Show reasoning", "on", SettingValueKind::Toggle, 16);
        assert!(on.starts_with('●'), "{on}");
        assert!(!on.contains("→") && !on.contains("open"), "{on}");

        let link = format_setting_line("Providers", "", SettingValueKind::Link, 16);
        assert!(link.contains('›'), "{link}");
        assert!(!link.contains("open"), "{link}");

        let model = format_setting_line(
            "Model",
            "mimo-v2.5-pro",
            SettingValueKind::Link,
            16,
        );
        assert!(model.contains("mimo-v2.5-pro"), "{model}");
        assert!(!model.contains("commandcode"), "{model}");
        assert!(!model.contains("Routing"), "{model}");
    }
}
