use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};

use crate::TuiApp;
use crate::plugin_approval::count_installed_plugins;
use crate::plugins::{PluginPickerRow, plugin_picker_rows};
use crate::render::{command_scroll_offset, modal_block};
use crate::theme::*;

pub(super) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
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
        .style(Style::default().bg(panel())),
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
            let selected_style = index == selected;
            let base = if selected_style {
                Style::default()
                    .fg(Color::White)
                    .bg(accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(text()).bg(panel())
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

    let mut list_state = ListState::default()
        .with_offset(command_scroll_offset(selected, rows[1].height as usize))
        .with_selected((!picker_rows.is_empty()).then_some(selected));
    frame.render_stateful_widget(
        List::new(items).style(Style::default().bg(panel())),
        rows[1],
        &mut list_state,
    );

    let installed = count_installed_plugins(app);
    frame.render_widget(
        Paragraph::new(format!(" {installed} installed "))
            .style(Style::default().fg(muted()).bg(panel())),
        rows[2],
    );
    frame.render_widget(
        Paragraph::new("↑↓ select  •  i install  •  u update  •  r refresh  •  enter  •  esc")
            .style(Style::default().fg(muted()).bg(panel())),
        rows[3],
    );
}
