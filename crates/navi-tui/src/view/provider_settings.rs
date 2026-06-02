use navi_sdk::provider_catalog;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Clear, List, ListItem, Paragraph};

use crate::TuiApp;
use crate::providers::provider_auth_status;
use crate::render::modal_block;
use crate::runtime::provider_supports_oauth;
use crate::theme::*;

pub(super) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    frame.render_widget(modal_block("Provider Accounts"), area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(10),
            Constraint::Length(2),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new("Configure API keys or OAuth sign-in for supported providers.")
            .style(Style::default().fg(muted()).bg(panel())),
        rows[0],
    );

    let providers = provider_catalog(&app.loaded_config.config);
    let height = rows[1].height as usize;
    let start = app.provider_settings_scroll.min(providers.len());
    let end = (start + height).min(providers.len());
    let items = providers[start..end]
        .iter()
        .enumerate()
        .map(|(offset, provider)| {
            let index = start + offset;
            let selected = index == app.selected_provider_setting;
            let status = provider_auth_status(app, provider);
            let oauth = if provider_supports_oauth(&provider.id) {
                "OAuth"
            } else {
                "API key"
            };
            let line = format!(
                "{:<18} {:<12} {:<10} {}",
                provider.label, status.label, oauth, provider.description
            );
            let style = if selected {
                Style::default()
                    .fg(Color::White)
                    .bg(accent())
                    .add_modifier(Modifier::BOLD)
            } else if status.configured {
                Style::default().fg(signal()).bg(panel())
            } else {
                Style::default().fg(muted()).bg(panel())
            };
            ListItem::new(Span::styled(line, style)).style(style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        List::new(items).style(Style::default().bg(panel())),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new("enter/k API key  •  o OAuth  •  r sync models  •  esc close")
            .style(Style::default().fg(muted()).bg(panel())),
        rows[2],
    );
}
