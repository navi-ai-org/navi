mod chat;
mod command_palette;
mod debug;
mod input;
mod modals;
mod model_picker;
mod notification;
mod plugins;
mod provider_settings;
mod sessions;
mod skills;
mod welcome;

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::prelude::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::TuiApp;
use crate::render::modal_rect;
use crate::state::Mode;
use crate::theme;
use crate::theme::{accent, bg, ghost, muted, signal, text};
use crate::ui::layout::{split_left_right, viewport_rect};

pub(crate) fn render(frame: &mut Frame<'_>, app: &mut TuiApp) {
    theme::with_palette(&app.theme_palette(), || render_inner(frame, app));
}

fn render_inner(frame: &mut Frame<'_>, app: &mut TuiApp) {
    let area = frame.area();
    frame.render_widget(Block::new().style(Style::default().bg(theme::bg())), area);
    let content_area = viewport_rect(area);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(6),
            Constraint::Length(5),
        ])
        .split(content_area);

    render_header(frame, app, vertical[0]);
    chat::render_chat_area(frame, app, vertical[1]);
    input::render_input(frame, app, vertical[2]);

    match app.mode {
        Mode::Commands => command_palette::render(frame, app, modal_rect(area, 68, 15)),
        Mode::Models => model_picker::render(frame, app, modal_rect(area, 72, 22)),
        Mode::ApiKeyEntry => modals::render_api_key_entry(frame, app, modal_rect(area, 72, 11)),
        Mode::Thinking => modals::render_thinking_picker(frame, app, modal_rect(area, 40, 10)),
        Mode::Sessions => sessions::render(frame, app, modal_rect(area, 72, 16)),
        Mode::Settings => modals::render_settings(frame, app, modal_rect(area, 52, 12)),
        Mode::Providers => provider_settings::render(frame, app, modal_rect(area, 76, 20)),
        Mode::Debug => debug::render(frame, app, modal_rect(area, 76, 18)),
        Mode::Help => modals::render_help_modal(frame, modal_rect(area, 62, 16)),
        Mode::Skills => skills::render(frame, app, modal_rect(area, 72, 20)),
        Mode::Plugins => plugins::render(frame, app, modal_rect(area, 76, 22)),
        Mode::PluginApproval => {
            modals::render_plugin_approval(frame, app, modal_rect(area, 84, 24))
        }
        Mode::Normal => {}
    }

    if !app.pending_approvals.is_empty() {
        modals::render_tool_approval(frame, app, modal_rect(area, 72, 12));
    }

    notification::render_notification(frame, app, area);
}

fn render_header(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let (left, right) = split_left_right(area, 20, 42);

    let branch = app.git_branch.as_deref().unwrap_or("project");
    if left.width > 0 {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" ", Style::default().fg(ghost()).bg(bg())),
                Span::styled(branch.to_string(), Style::default().fg(text()).bg(bg())),
                Span::styled("  ", Style::default().fg(ghost()).bg(bg())),
                Span::styled(
                    project_path_label(app),
                    Style::default().fg(muted()).bg(bg()),
                ),
            ]))
            .style(Style::default().bg(bg())),
            left,
        );
    }

    if right.width == 0 {
        return;
    }

    let context = header_context_label(app);
    let tool_count = app.running_tools.len();
    let approval_count = app.pending_approvals.len() + app.pending_plugin_approvals.len();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("│ ", Style::default().fg(ghost()).bg(bg())),
            Span::styled(
                context,
                Style::default()
                    .fg(signal())
                    .bg(bg())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" │ ", Style::default().fg(ghost()).bg(bg())),
            Span::styled(
                tool_count.to_string(),
                Style::default().fg(muted()).bg(bg()),
            ),
            Span::styled(" ", Style::default().fg(muted()).bg(bg())),
            Span::styled(
                approval_count.to_string(),
                Style::default()
                    .fg(accent())
                    .bg(bg())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default().fg(muted()).bg(bg())),
            Span::styled(
                "✓",
                Style::default()
                    .fg(text())
                    .bg(bg())
                    .add_modifier(Modifier::BOLD),
            ),
        ]))
        .alignment(Alignment::Right)
        .style(Style::default().bg(bg())),
        right,
    );
}

fn project_path_label(app: &TuiApp) -> String {
    let path = &app.project_dir;
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::PathBuf::from(home);
        if let Ok(stripped) = path.strip_prefix(&home) {
            let stripped = stripped.to_string_lossy();
            return if stripped.is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", stripped)
            };
        }
    }
    path.to_string_lossy().to_string()
}

fn header_context_label(app: &TuiApp) -> String {
    let total = app.compact_state.total_estimated_tokens(app.input.len());
    format!(
        "{} / {}",
        compact_token_label(total),
        compact_token_label(app.compact_state.context_window)
    )
}

fn compact_token_label(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{}K", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

#[cfg(test)]
pub(crate) fn build_chat_lines(
    app: &TuiApp,
    chat_width: usize,
) -> Vec<ratatui::prelude::Line<'static>> {
    chat::build_chat_lines(app, chat_width)
}
