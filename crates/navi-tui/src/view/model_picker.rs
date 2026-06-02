use navi_sdk::canonical_provider_id;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};

use crate::TuiApp;
use crate::providers::{ListRow, build_model_rows, selected_model_in_rows};
use crate::render::{modal_block, model_row_simple};
use crate::theme::*;

pub(super) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
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
        .style(Style::default().bg(panel())),
        rows[0],
    );

    let list_rows = build_model_rows(app);
    let list_area = rows[1];
    let row_width = list_area.width as usize;

    let selected_row = selected_model_in_rows(&list_rows, app.selected_model).unwrap_or(0);
    let mut list_state = ListState::default()
        .with_offset(app.model_scroll)
        .with_selected(Some(selected_row));

    let items = list_rows
        .iter()
        .map(|row| match row {
            ListRow::Header { label, .. } => {
                let header_style = Style::default()
                    .fg(text())
                    .bg(panel())
                    .add_modifier(Modifier::BOLD);
                let refresh_style = Style::default().fg(ghost()).bg(panel());

                let mut spans = vec![Span::styled(format!("  {}", label), header_style)];
                spans.push(Span::styled("  ↻ tab", refresh_style));
                ListItem::new(Line::from(spans)).style(header_style)
            }
            ListRow::Model { index } => {
                let model = &app.models[*index];
                let selected = *index == app.selected_model;
                let configured = model.name == app.loaded_config.config.model.name
                    && canonical_provider_id(&model.provider_id)
                        == canonical_provider_id(&app.loaded_config.config.model.provider);
                let style = if selected {
                    Style::default()
                        .fg(Color::White)
                        .bg(accent())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(text()).bg(panel())
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
        List::new(items).style(Style::default().bg(panel())),
        list_area,
        &mut list_state,
    );
    frame.render_widget(
        Paragraph::new(
            "type search  •  ↑↓ choose  •  ctrl+e edit setup  •  tab refresh provider  •  ctrl+r refresh all  •  enter confirm  •  esc exit",
        )
        .style(Style::default().fg(muted()).bg(panel())),
        rows[2],
    );
}
