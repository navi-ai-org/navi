use navi_sdk::{canonical_provider_id, model_can_run_publicly};
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::Style;
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::TuiApp;
use crate::providers::{ListRow, build_model_rows, selected_model_in_rows};
use crate::render::text::display_width;
use crate::render::{clear_modal_area, modal_block, modal_list_highlight_style};
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};
use crate::ui::list::render_scrollbar;
use crate::ui::{TextInputRenderSpec, render_text_input_line};

const LIST_RIGHT_PADDING_COLUMNS: u16 = 2;

pub(crate) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let title: &str = if app.mode == crate::state::Mode::BgModelPicker {
        "Background Model"
    } else {
        ""
    };
    let block = modal_block("");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(inner);

    let title_width = rows[0].width as usize;
    let right = fit_display_width("esc", title_width);
    let left_width = title_width.saturating_sub(display_width(&right) + 1);
    let left = fit_display_width(&format!("  {title}"), left_width);
    let gap = title_width
        .saturating_sub(display_width(&left) + display_width(&right))
        .max(1);
    let title_line = Line::from(vec![
        Span::styled(left, Style::default().fg(text()).bg(modal_bg())),
        Span::styled(" ".repeat(gap), Style::default().bg(modal_bg())),
        Span::styled(right, Style::default().fg(muted()).bg(modal_bg())),
    ]);
    frame.render_widget(
        Paragraph::new(title_line).style(Style::default().bg(modal_bg())),
        rows[0],
    );

    render_text_input_line(
        frame,
        Rect::new(rows[1].x, rows[1].y, rows[1].width, 1),
        TextInputRenderSpec {
            value: &app.model_filter,
            cursor: app.model_filter_cursor,
            placeholder: "Choose a model or provider",
            prefix: "> ",
            focused: true,
            text_style: Style::default().fg(text()).bg(modal_bg()),
            placeholder_style: Style::default().fg(muted()).bg(modal_bg()),
            prefix_style: Style::default().fg(signal()).bg(modal_bg()),
            cursor_style: Style::default().fg(bg()).bg(signal()),
            background_style: Style::default().bg(modal_bg()),
        },
    );

    let list_rows = build_model_rows(app);
    let list_area = rows[2];
    let has_scrollbar = list_rows.len() > list_area.height as usize;
    let list_content_area = model_list_content_area(list_area, has_scrollbar);
    let row_width = list_content_area.width as usize;

    if list_rows.is_empty() {
        // No authenticated providers — show instructional empty state.
        let dim = Style::default().fg(muted()).bg(modal_bg());
        let accent = Style::default().fg(signal()).bg(modal_bg());
        let hint = Style::default().fg(ghost()).bg(modal_bg());
        let empty_lines = vec![
            Line::from(""),
            Line::from(Span::styled("  No models available.", accent)),
            Line::from(""),
            Line::from(Span::styled("  Configure a provider to get started:", dim)),
            Line::from(""),
            Line::from(vec![
                Span::styled("  1. ", hint),
                Span::styled("ctrl+p", accent),
                Span::styled(" → ", dim),
                Span::styled("Providers", accent),
                Span::styled(" to add an API key", dim),
            ]),
            Line::from(vec![
                Span::styled("  2. Set an env var like ", dim),
                Span::styled("OPENAI_API_KEY", accent),
            ]),
            Line::from(Span::styled(
                "  3. Or add a [providers] entry in config.toml",
                dim,
            )),
        ];
        frame.render_widget(
            Paragraph::new(empty_lines).style(Style::default().bg(modal_bg())),
            list_area,
        );
        frame.render_widget(
            Paragraph::new(model_picker_footer_line(rows[3].width as usize))
                .style(Style::default().fg(text()).bg(modal_bg())),
            rows[3],
        );
        return;
    }

    let selected_model = if app.mode == crate::state::Mode::BgModelPicker {
        app.bg_model_picker_selected
    } else {
        app.selected_model
    };
    let selected_row = selected_model_in_rows(&list_rows, selected_model).unwrap_or(0);
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
                let header_style = Style::default().fg(signal()).bg(modal_bg());
                let label = clean_section_label(label);
                ListItem::new(provider_header_line(&label, row_width)).style(header_style)
            }
            ListRow::Spacer => ListItem::new(Line::from("")).style(Style::default().bg(modal_bg())),
            ListRow::Model { index } => {
                let model = &app.models[*index];
                let selected = *index == selected_model;
                let hovered = app.hover_index == Some(*index);
                let configured = if app.mode == crate::state::Mode::BgModelPicker {
                    // In bg model picker, show which model is the current override.
                    bg_model_is_current_override(app, model)
                } else {
                    model.name == app.loaded_config.config.model.name
                        && canonical_provider_id(&model.provider_id)
                            == canonical_provider_id(&app.loaded_config.config.model.provider)
                };
                let style = if hovered || selected {
                    active_item_style()
                } else {
                    inactive_item_style()
                };
                let recent = model_is_recent(app, model);
                ListItem::new(model_row_line(model, configured, recent, row_width, style))
                    .style(style)
            }
        })
        .collect::<Vec<_>>();

    frame.render_stateful_widget(
        List::new(items)
            .style(Style::default().bg(modal_bg()))
            .highlight_style(modal_list_highlight_style()),
        list_content_area,
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
        let rect = line_rect(
            list_content_area,
            row_offset.saturating_sub(app.model_scroll),
        );
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
            ListRow::Spacer => {}
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
        Paragraph::new(model_picker_footer_line(rows[3].width as usize))
            .style(Style::default().fg(text()).bg(modal_bg())),
        rows[3],
    );
}

fn provider_header_line(label: &str, width: usize) -> Line<'static> {
    let label = fit_display_width(label, width.saturating_sub(4).max(1));
    let left = format!("  {label} ");
    let separator_width = width.saturating_sub(display_width(&left));
    Line::from(vec![
        Span::styled(left, Style::default().fg(muted()).bg(modal_bg())),
        Span::styled(
            "─".repeat(separator_width),
            Style::default().fg(ghost()).bg(modal_bg()),
        ),
    ])
}

fn model_picker_footer_line(width: usize) -> Line<'static> {
    let full_value = "↑/↓ choose  ·  tab refresh  ·  connect provider ctrl+e  ·  favorite ctrl+f";
    if display_width(full_value) <= width {
        return Line::from(vec![
            Span::styled("↑/↓", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(" choose", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled("  ·  ", Style::default().fg(ghost()).bg(modal_bg())),
            Span::styled("tab", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(" refresh", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled("  ·  ", Style::default().fg(ghost()).bg(modal_bg())),
            Span::styled(
                "connect provider",
                Style::default().fg(muted()).bg(modal_bg()),
            ),
            Span::styled(" ctrl+e", Style::default().fg(text()).bg(modal_bg())),
            Span::styled("  ·  ", Style::default().fg(ghost()).bg(modal_bg())),
            Span::styled("favorite", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled(" ctrl+f", Style::default().fg(text()).bg(modal_bg())),
        ]);
    }

    let compact_value = "↑/↓ choose  ·  connect provider ctrl+e  ·  favorite ctrl+f";
    if display_width(compact_value) > width {
        return Line::from(Span::styled(
            fit_display_width(compact_value, width),
            Style::default().fg(muted()).bg(modal_bg()),
        ));
    }
    Line::from(vec![
        Span::styled("↑/↓", Style::default().fg(text()).bg(modal_bg())),
        Span::styled(" choose", Style::default().fg(muted()).bg(modal_bg())),
        Span::styled("  ·  ", Style::default().fg(ghost()).bg(modal_bg())),
        Span::styled(
            "connect provider",
            Style::default().fg(muted()).bg(modal_bg()),
        ),
        Span::styled(" ctrl+e", Style::default().fg(text()).bg(modal_bg())),
        Span::styled("  ·  ", Style::default().fg(ghost()).bg(modal_bg())),
        Span::styled("favorite", Style::default().fg(muted()).bg(modal_bg())),
        Span::styled(" ctrl+f", Style::default().fg(text()).bg(modal_bg())),
    ])
}

fn model_list_content_area(list_area: Rect, has_scrollbar: bool) -> Rect {
    let reserved = LIST_RIGHT_PADDING_COLUMNS + u16::from(has_scrollbar);
    Rect {
        width: list_area.width.saturating_sub(reserved).max(1),
        ..list_area
    }
}

fn clean_section_label(label: &str) -> String {
    let cleaned = label.trim().trim_matches('—').trim().to_string();
    if cleaned.eq_ignore_ascii_case("recent models") {
        "Recent".to_string()
    } else {
        cleaned
    }
}

fn model_is_recent(app: &TuiApp, model: &navi_sdk::ModelOption) -> bool {
    app.loaded_config
        .config
        .tui
        .recent_model_ids
        .iter()
        .any(|recent| {
            recent
                .split_once(':')
                .is_some_and(|(provider_id, model_name)| {
                    canonical_provider_id(provider_id) == canonical_provider_id(&model.provider_id)
                        && model_name == model.name
                })
        })
}

fn model_row_line(
    model: &navi_sdk::ModelOption,
    configured: bool,
    recent: bool,
    width: usize,
    style: Style,
) -> Line<'static> {
    let marker = if configured { "● " } else { "  " };
    let name = display_model_name(&model.name);
    let left = format!("  {marker}{name}");
    let right = if model_can_run_publicly(&model.provider_id, &model.name) {
        "Free".to_string()
    } else if recent {
        model.provider_label.clone()
    } else {
        String::new()
    };
    let reserved = display_width(&right).saturating_add(1);
    let left_width = width.saturating_sub(reserved).max(1);
    let left = fit_display_width(&left, left_width);
    let used = display_width(&left) + display_width(&right);
    let gap = width.saturating_sub(used).max(1);
    Line::from(vec![
        Span::styled(left, style),
        Span::styled(" ".repeat(gap), style),
        Span::styled(
            right,
            Style::default()
                .fg(muted())
                .bg(style.bg.unwrap_or(modal_bg())),
        ),
    ])
}

fn fit_display_width(value: &str, width: usize) -> String {
    if display_width(value) <= width {
        return value.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_string();
    }
    let mut result = String::new();
    let mut used = 0usize;
    for ch in value.chars() {
        let char_width = display_width(&ch.to_string());
        if used + char_width >= width {
            break;
        }
        result.push(ch);
        used += char_width;
    }
    result.push('…');
    result
}

fn display_model_name(name: &str) -> String {
    name.split('/')
        .last()
        .unwrap_or(name)
        .split('-')
        .map(|part| {
            if part.chars().all(|ch| ch.is_ascii_uppercase()) {
                part.to_string()
            } else {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => {
                        first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                    }
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn bg_model_is_current_override(app: &TuiApp, model: &navi_sdk::ModelOption) -> bool {
    let bg = &app.loaded_config.config.background_models;
    let task = app.bg_model_picker_task.as_deref().unwrap_or("");
    let entry = match task {
        "naming" => bg.naming.as_ref(),
        "compaction" => bg.compaction.as_ref(),
        "repo_search" => bg.repo_search.as_ref(),
        "subagent_research" => bg.subagent_research.as_ref(),
        "simple_code_edit" => bg.simple_code_edit.as_ref(),
        _ => bg.default.as_ref(),
    };
    if let Some(entry) = entry {
        entry.provider.as_deref() == Some(&model.provider_id)
            && entry.model.as_deref() == Some(&model.name)
    } else {
        false
    }
}
