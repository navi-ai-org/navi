mod chat;
mod input;
mod modals;
mod notification;
mod welcome;

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::prelude::Frame;
use ratatui::style::Style;
use ratatui::widgets::Block;

use crate::TuiApp;
use crate::render::modal_rect;
use crate::state::Mode;
use crate::theme::BG;

// ─── rendering ─────────────────────────────────────────────────────────────────
pub(crate) fn render(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = frame.area();
    frame.render_widget(Block::new().style(Style::default().bg(BG)), area);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(6),
            Constraint::Length(1),
            Constraint::Length(7),
        ])
        .split(area);

    chat::render_chat_area(frame, app, vertical[0]);
    input::render_input(frame, app, vertical[2]);

    match app.mode {
        Mode::Commands => modals::render_command_palette(frame, app, modal_rect(area, 68, 15)),
        Mode::Models => modals::render_model_picker(frame, app, modal_rect(area, 72, 22)),
        Mode::ApiKeyEntry => modals::render_api_key_entry(frame, app, modal_rect(area, 72, 11)),
        Mode::Thinking => modals::render_thinking_picker(frame, app, modal_rect(area, 40, 10)),
        Mode::Sessions => modals::render_sessions_picker(frame, app, modal_rect(area, 72, 16)),
        Mode::Settings => modals::render_settings(frame, app, modal_rect(area, 50, 10)),
        Mode::Providers => modals::render_provider_settings(frame, app, modal_rect(area, 76, 20)),
        Mode::Debug => modals::render_debug_modal(frame, app, modal_rect(area, 76, 18)),
        Mode::Help => modals::render_help_modal(frame, modal_rect(area, 62, 16)),
        Mode::Normal => {}
    }

    if !app.pending_approvals.is_empty() {
        modals::render_tool_approval(frame, app, modal_rect(area, 72, 12));
    }

    notification::render_notification(frame, app, area);
}

#[cfg(test)]
pub(crate) fn build_chat_lines(
    app: &TuiApp,
    chat_width: usize,
) -> Vec<ratatui::prelude::Line<'static>> {
    chat::build_chat_lines(app, chat_width)
}
