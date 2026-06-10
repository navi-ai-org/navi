use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Span};
use ratatui::style::Style;
use ratatui::widgets::{List, ListItem, Paragraph};

use crate::TuiApp;
use crate::providers::provider_auth_status;
use crate::render::{clear_modal_area, modal_block};
use crate::runtime::provider_supports_oauth;
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};
use crate::ui::list::render_scrollbar;

pub(super) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
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
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let header_text = if app.provider_filter.is_empty() {
        "Type to filter providers...".to_string()
    } else {
        format!("Filter: {}_", app.provider_filter)
    };

    frame.render_widget(
        Paragraph::new(header_text).style(Style::default().fg(muted()).bg(modal_bg())),
        rows[0],
    );

    let providers = app.filtered_providers();
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
                ""
            };
            let line = format!(
                "{:<30} {:<12} {:<10} {}",
                provider.label, status.label, oauth, provider.description
            );
            let style = if app.hover_index == Some(index) || selected {
                active_item_style()
            } else if status.configured {
                Style::default().fg(signal()).bg(modal_bg())
            } else {
                inactive_item_style()
            };
            ListItem::new(Span::styled(line, style)).style(style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        List::new(items).style(Style::default().bg(modal_bg())),
        rows[1],
    );
    render_scrollbar(
        frame,
        app,
        rows[1],
        providers.len(),
        start,
        crate::ui::interaction::ScrollTarget::Providers,
    );
    for (offset, provider) in providers[start..end].iter().enumerate() {
        let index = start + offset;
        let row = line_rect(rows[1], offset);
        app.register_hit(
            row,
            20,
            format!("provider {} api key", provider.label),
            HitAction::ProviderApiKey(index),
        );
        if provider_supports_oauth(&provider.id) {
            app.register_hit(
                Rect::new(row.x + 43, row.y, 10.min(row.width.saturating_sub(43)), 1),
                21,
                format!("provider {} oauth", provider.label),
                HitAction::ProviderOAuth(index),
            );
        }
    }

    frame.render_widget(
        Paragraph::new(
            "Enter configure key     ctrl-o OAuth     ctrl-r sync models     ctrl-d delete",
        )
        .style(Style::default().fg(text()).bg(modal_bg())),
        rows[3],
    );
}
