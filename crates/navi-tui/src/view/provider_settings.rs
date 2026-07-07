use crate::TuiApp;
use crate::providers::{ProviderListRow, provider_auth_status};
use crate::render::{clear_modal_area, modal_block};
use crate::runtime::provider_supports_oauth;
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};
use crate::ui::list::render_scrollbar;
use crate::ui::{TextInputRenderSpec, render_text_input_line};
use navi_sdk::provider_catalog;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Span};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{List, ListItem, Paragraph};

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

    render_text_input_line(
        frame,
        rows[0],
        TextInputRenderSpec {
            value: &app.provider_filter,
            cursor: app.provider_filter_cursor,
            placeholder: "Type to filter providers...",
            prefix: "> ",
            focused: true,
            text_style: Style::default().fg(text()).bg(modal_bg()),
            placeholder_style: Style::default().fg(muted()).bg(modal_bg()),
            prefix_style: Style::default().fg(signal()).bg(modal_bg()),
            cursor_style: Style::default().fg(bg()).bg(signal()),
            background_style: Style::default().bg(modal_bg()),
        },
    );

    let list_rows = app.filtered_providers();
    let catalog = provider_catalog(&app.loaded_config.config);
    let height = rows[1].height as usize;
    let start = app.provider_settings_scroll.min(list_rows.len());
    let end = (start + height).min(list_rows.len());

    let items = list_rows[start..end]
        .iter()
        .enumerate()
        .map(|(offset, row)| {
            let index = start + offset;
            match row {
                ProviderListRow::Header { label } => {
                    let header_style = Style::default()
                        .fg(text())
                        .bg(modal_bg())
                        .add_modifier(Modifier::BOLD);
                    ListItem::new(Span::styled(format!("  {label}"), header_style))
                        .style(header_style)
                }
                ProviderListRow::Provider { index: catalog_idx } => {
                    let Some(provider) = catalog.get(*catalog_idx) else {
                        return ListItem::new(Span::styled("", inactive_item_style()))
                            .style(inactive_item_style());
                    };
                    let selected = index == app.selected_provider_setting;
                    let status = provider_auth_status(app, provider);
                    let oauth = if provider_supports_oauth(&provider.id) {
                        "OAuth"
                    } else {
                        ""
                    };
                    let line = format!("{:<30} {:<12} {:<10}", provider.label, status.label, oauth);
                    let style = if app.hover_index == Some(index) || selected {
                        active_item_style()
                    } else if status.configured {
                        Style::default().fg(signal()).bg(modal_bg())
                    } else {
                        inactive_item_style()
                    };
                    ListItem::new(Span::styled(line, style)).style(style)
                }
                ProviderListRow::Account {
                    label, selected, ..
                } => {
                    let marker = if *selected { "●" } else { "○" };
                    let line = format!("  {marker} account  {label}");
                    let style = if app.hover_index == Some(index)
                        || index == app.selected_provider_setting
                    {
                        active_item_style()
                    } else if *selected {
                        Style::default().fg(signal()).bg(modal_bg())
                    } else {
                        inactive_item_style()
                    };
                    ListItem::new(Span::styled(line, style)).style(style)
                }
            }
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
        list_rows.len(),
        start,
        crate::ui::interaction::ScrollTarget::Providers,
    );

    // Hit tests: only register for Provider rows (headers are non-selectable).
    for (offset, row) in list_rows[start..end].iter().enumerate() {
        let index = start + offset;
        match row {
            ProviderListRow::Provider { index: catalog_idx } => {
                let Some(provider) = catalog.get(*catalog_idx) else {
                    continue;
                };
                let row_rect = line_rect(rows[1], offset);
                app.register_hit(
                    row_rect,
                    20,
                    format!("provider {} api key", provider.label),
                    HitAction::ProviderApiKey(index),
                );
                if provider_supports_oauth(&provider.id) {
                    app.register_hit(
                        Rect::new(
                            row_rect.x + 43,
                            row_rect.y,
                            10.min(row_rect.width.saturating_sub(43)),
                            1,
                        ),
                        21,
                        format!("provider {} oauth", provider.label),
                        HitAction::ProviderOAuth(index),
                    );
                }
            }
            ProviderListRow::Account { .. } => {
                let row_rect = line_rect(rows[1], offset);
                app.register_hit(
                    row_rect,
                    20,
                    "select provider account",
                    HitAction::ProviderApiKey(index),
                );
            }
            ProviderListRow::Header { .. } => {}
        }
    }

    frame.render_widget(
        Paragraph::new(
            "Enter select account/key     ctrl-o OAuth     ctrl-r sync models     ctrl-d delete",
        )
        .style(Style::default().fg(text()).bg(modal_bg())),
        rows[3],
    );
}
