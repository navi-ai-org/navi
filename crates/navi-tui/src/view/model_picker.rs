use navi_sdk::canonical_provider_id;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::TuiApp;
use crate::providers::{ListRow, build_model_rows, selected_model_in_rows};
use crate::render::{clear_modal_area, modal_block, modal_list_highlight_style, model_row_simple};
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};
use crate::ui::list::render_scrollbar;

pub(super) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let block = modal_block("Switch Protocol");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(inner);

    let filter_text = if app.model_filter.is_empty() {
        "search providers or models"
    } else {
        app.model_filter.as_str()
    };
    frame.render_widget(
        Paragraph::new(Text::from(vec![Line::from(vec![
            Span::styled("> ", Style::default().fg(signal())),
            Span::styled(
                filter_text,
                Style::default().fg(if app.model_filter.is_empty() {
                    muted()
                } else {
                    text()
                }),
            ),
        ])]))
        .style(Style::default().bg(modal_bg())),
        rows[0],
    );

    let list_rows = build_model_rows(app);
    let list_area = rows[1];
    let row_width = list_area.width as usize;

    let selected_row = selected_model_in_rows(&list_rows, app.selected_model).unwrap_or(0);
    let hover_row = app
        .hover_index
        .and_then(|idx| selected_model_in_rows(&list_rows, idx));
    let mut list_state = ListState::default()
        .with_offset(app.model_scroll)
        .with_selected(Some(hover_row.unwrap_or(selected_row)));

    let items = list_rows
        .iter()
        .map(|row| match row {
            ListRow::Header { label, .. } => {
                let header_style = Style::default()
                    .fg(text())
                    .bg(modal_bg())
                    .add_modifier(Modifier::BOLD);
                let refresh_style = Style::default().fg(ghost()).bg(modal_bg());

                let mut spans = vec![Span::styled(format!("  {}", label), header_style)];
                spans.push(Span::styled("  ↻ tab", refresh_style));
                ListItem::new(Line::from(spans)).style(header_style)
            }
            ListRow::Model { index } => {
                let model = &app.models[*index];
                let selected = *index == app.selected_model;
                let hovered = app.hover_index == Some(*index);
                let configured = model.name == app.loaded_config.config.model.name
                    && canonical_provider_id(&model.provider_id)
                        == canonical_provider_id(&app.loaded_config.config.model.provider);
                let style = if hovered || selected {
                    active_item_style()
                } else {
                    inactive_item_style()
                };

                ListItem::new(Span::styled(
                    model_row_simple(model.name.as_str(), configured, row_width),
                    style,
                ))
                .style(style)
            }
        })
        .collect::<Vec<_>>();

    frame.render_stateful_widget(
        List::new(items)
            .style(Style::default().bg(modal_bg()))
            .highlight_style(modal_list_highlight_style()),
        list_area,
        &mut list_state,
    );
    render_scrollbar(
        frame,
        app,
        list_area,
        list_rows.len(),
        app.model_scroll,
        crate::ui::interaction::ScrollTarget::Models,
    );
    for (row_offset, row) in list_rows
        .iter()
        .enumerate()
        .skip(app.model_scroll)
        .take(list_area.height as usize)
    {
        let rect = line_rect(list_area, row_offset.saturating_sub(app.model_scroll));
        match row {
            ListRow::Header {
                provider_id, label, ..
            } => {
                app.register_hit(
                    rect,
                    20,
                    format!("refresh provider {label}"),
                    HitAction::ModelProviderRefresh(provider_id.clone()),
                );
            }
            ListRow::Model { index } => {
                let label = app
                    .models
                    .get(*index)
                    .map(|model| model.name.clone())
                    .unwrap_or_else(|| "model".to_string());
                app.register_hit(rect, 20, format!("model {label}"), HitAction::Model(*index));
            }
        }
    }
    frame.render_widget(
        Paragraph::new("search  •  ctrl+e setup  •  tab refresh provider  •  ctrl+r refresh all")
            .style(Style::default().fg(text()).bg(modal_bg())),
        rows[2],
    );
}
