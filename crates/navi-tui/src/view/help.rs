//! Keyboard shortcuts / help cheatsheet modal.
//!
//! Opened with `?` (empty input), `ctrl+.`, or the command palette **Help**.
//! Layout: section headers + key/description rows, scrollable list,
//! selected row detail in the footer.

use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Paragraph, Wrap};

use crate::TuiApp;
use crate::render::text::display_width;
use crate::render::{clear_modal_area, modal_block};
use crate::theme::*;
use crate::ui::interaction::{HitAction, ScrollTarget, line_rect};
use crate::ui::list::render_scrollbar;

/// One visual row in the help modal.
#[derive(Debug, Clone, Copy)]
pub(crate) enum HelpRow {
    Section(&'static str),
    Entry {
        key: &'static str,
        label: &'static str,
        /// Longer description shown when the row is selected.
        detail: &'static str,
    },
    Blank,
}

/// Full cheatsheet content (sectioned keyboard help).
pub(crate) const HELP_ROWS: &[HelpRow] = &[
    HelpRow::Section("General"),
    HelpRow::Entry {
        key: "ctrl+p",
        label: "Command palette",
        detail: "Fuzzy-search every action and slash-style command, then run it.",
    },
    HelpRow::Entry {
        key: "ctrl+.  /  ctrl+x  /  ?",
        label: "Open this help",
        detail: "Keyboard shortcuts cheatsheet. Ctrl+X is a classic-control fallback when Ctrl+. needs Kitty/tmux extended-keys. ? works when the input is empty.",
    },
    HelpRow::Entry {
        key: "ctrl+c",
        label: "Quit NAVI",
        detail: "Exit the terminal UI.",
    },
    HelpRow::Entry {
        key: "esc",
        label: "Cancel / close",
        detail: "Close the active modal, dismiss overlays, or cancel pending prompts.",
    },
    HelpRow::Blank,
    HelpRow::Section("Composer"),
    HelpRow::Entry {
        key: "ctrl+enter",
        label: "Send prompt",
        detail: "Submit the current message (and any attached images) to the model.",
    },
    HelpRow::Entry {
        key: "enter  /  ctrl+j",
        label: "Insert newline",
        detail: "Multiline draft by default — Enter inserts a newline; send with ctrl+enter.",
    },
    HelpRow::Entry {
        key: "ctrl+a",
        label: "Select all input",
        detail: "Select the entire composer buffer (including [Image N] chips).",
    },
    HelpRow::Entry {
        key: "ctrl+v  /  ctrl+i",
        label: "Paste image",
        detail: "Attach a clipboard image as an [Image N] chip in the prompt.",
    },
    HelpRow::Entry {
        key: "/",
        label: "Commands (empty input)",
        detail: "With an empty composer, / opens the command palette.",
    },
    HelpRow::Entry {
        key: "@",
        label: "Mention file or folder",
        detail: "Type @ to pick a project path. On send, file contents / dir listings are attached for the model.",
    },
    HelpRow::Blank,
    HelpRow::Section("Scrollback"),
    HelpRow::Entry {
        key: "↑  /  ↓",
        label: "Select block",
        detail: "Move selection across user messages, assistant replies, and tool cards.",
    },
    HelpRow::Entry {
        key: "j  /  k",
        label: "Select block (vim)",
        detail: "Same as arrows when the input is empty.",
    },
    HelpRow::Entry {
        key: "y",
        label: "Copy selected block",
        detail: "Copy the selected message or tool body to the clipboard.",
    },
    HelpRow::Entry {
        key: "enter",
        label: "Activate selected block",
        detail: "Expand/collapse a tool body, or open nested content when available.",
    },
    HelpRow::Entry {
        key: "mouse drag",
        label: "Select text in block",
        detail: "Text selection stays constrained to the active message/tool block.",
    },
    HelpRow::Entry {
        key: "ctrl+↓  /  shift+g  /  ctrl+end",
        label: "Jump to latest",
        detail: "Scroll to the last message. Same as the ↓ Latest button when scrolled up.",
    },
    HelpRow::Blank,
    HelpRow::Section("Tools & permissions"),
    HelpRow::Entry {
        key: "ctrl+o",
        label: "Smart / expand-all tools",
        detail: "Toggle expand-all vs smart defaults without closing a tool you just opened.",
    },
    HelpRow::Entry {
        key: "alt+t",
        label: "Show / hide thinking",
        detail: "Toggle reasoning text visibility in chat (also available as Show Reasoning).",
    },
    HelpRow::Entry {
        key: "shift+tab",
        label: "Cycle permission mode",
        detail: "Restricted → Accept edits → Auto → … for tool approvals.",
    },
    HelpRow::Entry {
        key: "ctrl+g",
        label: "Toggle YOLO",
        detail: "Most permissive mode: auto-approve tools (use carefully).",
    },
    HelpRow::Entry {
        key: "ctrl+t",
        label: "Shell tasks",
        detail: "Open running/finished bash background jobs — ▸ open output, ✕ cancel.",
    },
    HelpRow::Entry {
        key: "ctrl+b",
        label: "Model routing",
        detail: "Unified Chat / Agents / Attachments model routing (tabs with ←/→).",
    },
    HelpRow::Entry {
        key: "ctrl+,",
        label: "Settings",
        detail: "Open the settings hub (appearance, routing, accounts, updates).",
    },
    HelpRow::Blank,
    HelpRow::Section("Sessions & models"),
    HelpRow::Entry {
        key: "ctrl+n",
        label: "New session",
        detail: "Start a fresh conversation layer for the current project.",
    },
    HelpRow::Entry {
        key: "ctrl+s",
        label: "Sessions",
        detail: "Resume or browse saved sessions.",
    },
    HelpRow::Entry {
        key: "ctrl+m",
        label: "Models",
        detail: "Open the model picker; tab refreshes the selected provider.",
    },
    HelpRow::Entry {
        key: "ctrl+d",
        label: "Debug",
        detail: "Show log path, session id, model/provider, and recent diagnostics.",
    },
    HelpRow::Entry {
        key: "ctrl+q",
        label: "Message queue",
        detail: "Inspect and edit prompts queued while a turn is running.",
    },
    HelpRow::Blank,
    HelpRow::Section("Tips"),
    HelpRow::Entry {
        key: "ctrl+p → Help",
        label: "Also open this modal",
        detail: "Type “help” in the command palette if you forget the key.",
    },
    HelpRow::Entry {
        key: "tab",
        label: "Provider actions",
        detail: "In the model picker, tab refreshes models for the selected provider.",
    },
];

pub(crate) fn help_entry_count() -> usize {
    HELP_ROWS.len()
}

pub(crate) fn clamp_help_selection(app: &mut TuiApp) {
    let max = help_entry_count().saturating_sub(1);
    if app.selected_help > max {
        app.selected_help = max;
    }
    // Prefer landing on an entry, not a section/blank, when opening.
    while matches!(
        HELP_ROWS.get(app.selected_help),
        Some(HelpRow::Section(_) | HelpRow::Blank)
    ) && app.selected_help < max
    {
        app.selected_help += 1;
    }
}

pub(crate) fn move_help_selection(app: &mut TuiApp, delta: isize) {
    let len = help_entry_count();
    if len == 0 {
        return;
    }
    let step = if delta >= 0 { 1isize } else { -1 };
    let mut remaining = delta.abs().max(1);
    let mut idx = app.selected_help as isize;
    while remaining > 0 {
        let next = idx + step;
        if next < 0 || next >= len as isize {
            break;
        }
        idx = next;
        // Skip blank spacer rows so selection always lands on content.
        if matches!(HELP_ROWS.get(idx as usize), Some(HelpRow::Blank)) {
            continue;
        }
        remaining -= 1;
    }
    app.selected_help = idx as usize;
    ensure_help_visible(app);
}

pub(crate) fn ensure_help_visible(app: &mut TuiApp) {
    // Visible body height is refreshed each render; use a conservative default
    // and clamp scroll so selected row stays in view when possible.
    let visible = app.help_visible_rows.get().max(3);
    if app.selected_help < app.help_scroll {
        app.help_scroll = app.selected_help;
    } else if app.selected_help >= app.help_scroll.saturating_add(visible) {
        app.help_scroll = app.selected_help.saturating_add(1).saturating_sub(visible);
    }
    let max_scroll = help_entry_count().saturating_sub(visible);
    if app.help_scroll > max_scroll {
        app.help_scroll = max_scroll;
    }
}

pub(crate) fn open_help(app: &mut TuiApp) {
    crate::keybindings::replace_modal(app, crate::state::ModalKind::Help);
    app.help_scroll = 0;
    app.selected_help = 0;
    clamp_help_selection(app);
    ensure_help_visible(app);
}

pub(crate) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    frame.render_widget(modal_block("Help · Keyboard Shortcuts"), area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "NAVI TUI cheatsheet",
                Style::default().fg(muted()).bg(modal_bg()),
            ),
            Span::styled(
                "  ·  bindings are built-in",
                Style::default().fg(ghost()).bg(modal_bg()),
            ),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[0],
    );

    let list_area = rows[1];
    let visible = list_area.height as usize;
    app.help_visible_rows.set(visible.max(3));
    let start = app
        .help_scroll
        .min(HELP_ROWS.len().saturating_sub(visible.max(1)));
    let end = (start + visible).min(HELP_ROWS.len());

    let key_col = key_column_width();
    for (row_i, help_row) in HELP_ROWS[start..end].iter().enumerate() {
        let absolute = start + row_i;
        let selected = absolute == app.selected_help;
        let row_area = line_rect(list_area, row_i);
        let bg = if selected { selection_bg() } else { modal_bg() };
        let line = match help_row {
            HelpRow::Section(title) => Line::from(vec![
                Span::styled(
                    format!(" {title} "),
                    Style::default()
                        .fg(accent())
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "─".repeat(list_area.width.saturating_sub(title.len() as u16 + 3) as usize),
                    Style::default().fg(ghost()).bg(bg),
                ),
            ]),
            HelpRow::Blank => Line::from(Span::styled(" ", Style::default().bg(bg))),
            HelpRow::Entry { key, label, .. } => {
                let key_style = Style::default()
                    .fg(if selected { signal() } else { signal() })
                    .bg(bg)
                    .add_modifier(Modifier::BOLD);
                let label_style = Style::default()
                    .fg(if selected { text() } else { text() })
                    .bg(bg)
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    });
                let key_pad = format!("{key:<key_col$}");
                Line::from(vec![
                    Span::styled(key_pad, key_style),
                    Span::styled("  ", Style::default().bg(bg)),
                    Span::styled(*label, label_style),
                ])
            }
        };
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(bg)),
            row_area,
        );
        if matches!(help_row, HelpRow::Entry { .. }) {
            app.register_hit(
                row_area,
                25,
                format!("help row {absolute}"),
                HitAction::HelpRow(absolute),
            );
        }
    }

    render_scrollbar(
        frame,
        app,
        list_area,
        HELP_ROWS.len(),
        start,
        ScrollTarget::Help,
    );

    let detail = match HELP_ROWS.get(app.selected_help) {
        Some(HelpRow::Entry { detail, .. }) => *detail,
        Some(HelpRow::Section(title)) => *title,
        _ => "Browse shortcuts with ↑↓ · Enter or ? closes",
    };
    let footer_lines = vec![
        Line::from(Span::styled(
            truncate_line(detail, list_area.width as usize),
            Style::default().fg(muted()).bg(modal_bg()),
        )),
        Line::from(Span::styled(
            "↑↓/j k select   pgup/pgdn   esc/? close",
            Style::default().fg(ghost()).bg(modal_bg()),
        )),
    ];
    frame.render_widget(
        Paragraph::new(footer_lines)
            .style(Style::default().bg(modal_bg()))
            .wrap(Wrap { trim: false }),
        rows[2],
    );
}

fn key_column_width() -> usize {
    HELP_ROWS
        .iter()
        .filter_map(|row| match row {
            HelpRow::Entry { key, .. } => Some(display_width(key)),
            _ => None,
        })
        .max()
        .unwrap_or(12)
        .max(12)
}

fn truncate_line(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if display_width(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = display_width(&ch.to_string());
        if used + w + 1 > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// Remember visible list height for scroll clamping.
pub(crate) fn set_help_visible_rows(app: &mut TuiApp, rows: usize) {
    app.help_visible_rows.set(rows.max(3));
}
