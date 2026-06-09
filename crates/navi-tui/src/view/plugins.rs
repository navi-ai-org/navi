use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::Style;
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::TuiApp;
use crate::plugin_approval::count_installed_plugins;
use crate::plugins::{PluginPickerRow, plugin_picker_rows};
use crate::render::{clear_modal_area, modal_block, modal_list_highlight_style};
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};
use crate::ui::list::render_scrollbar;

pub(super) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let block = modal_block("Plugins");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(6),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let status = if app.plugin_catalog_loading {
        "Loading marketplace catalog…".to_string()
    } else if !app.plugin_catalog_error.is_empty() {
        format!("Catalog error: {}", app.plugin_catalog_error)
    } else {
        format!(
            "Marketplace: {} plugin(s) • install dir: {}",
            app.plugin_catalog.len(),
            app.loaded_config.data_dir.join("plugins").display()
        )
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("◇ ", Style::default().fg(signal())),
            Span::styled(status, Style::default().fg(muted())),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[0],
    );

    let picker_rows = plugin_picker_rows(app);
    let selected = app
        .selected_plugin_row
        .min(picker_rows.len().saturating_sub(1));

    let items: Vec<ListItem> = picker_rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let is_selected = index == selected;
            let is_hovered = app.hover_index == Some(index);
            let base = if is_hovered || is_selected {
                active_item_style()
            } else {
                inactive_item_style()
            };

            let label = match row {
                PluginPickerRow::Catalog(entry) => {
                    format!(" [market] {} v{} — {}", entry.id, entry.version, entry.name)
                }
                PluginPickerRow::Installed {
                    id,
                    version,
                    publisher,
                    tool_count,
                } => format!(" [installed] {id} v{version} ({publisher}) — {tool_count} tool(s)"),
            };

            ListItem::new(Span::styled(label, base)).style(base)
        })
        .collect();

    let offset = app
        .plugin_row_scroll
        .min(picker_rows.len().saturating_sub(rows[1].height as usize));
    let mut list_state = ListState::default()
        .with_offset(offset)
        .with_selected((!picker_rows.is_empty()).then_some(app.hover_index.unwrap_or(selected)));
    frame.render_stateful_widget(
        List::new(items)
            .style(Style::default().bg(modal_bg()))
            .highlight_style(modal_list_highlight_style()),
        rows[1],
        &mut list_state,
    );
    render_scrollbar(
        frame,
        app,
        rows[1],
        picker_rows.len(),
        offset,
        crate::ui::interaction::ScrollTarget::Plugins,
    );
    for (row_offset, index) in (offset..picker_rows.len())
        .take(rows[1].height as usize)
        .enumerate()
    {
        app.register_hit(
            line_rect(rows[1], row_offset),
            20,
            "plugin row",
            HitAction::PluginInstallOrUpdate(index),
        );
    }

    let installed = count_installed_plugins(app);
    frame.render_widget(
        Paragraph::new(format!(" {installed} installed "))
            .style(Style::default().fg(text()).bg(modal_bg())),
        rows[2],
    );
    frame.render_widget(
        Paragraph::new("i install  •  u update  •  r refresh")
            .style(Style::default().fg(text()).bg(modal_bg())),
        rows[3],
    );
    app.register_hit(rows[3], 20, "refresh plugins", HitAction::PluginRefresh);
}
