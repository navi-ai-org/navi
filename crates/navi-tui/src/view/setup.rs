//! Setup wizard overlay: structured list steps + normal chat during interview.

use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};

use crate::TuiApp;
use crate::render::{clear_modal_area, modal_block};
use crate::state::SetupPhase;
use crate::theme::*;

const APPROVAL_OPTIONS: &[&str] = &[
    "Restricted — approve every tool",
    "Accept edits — auto-approve reads/writes; commands need approval",
    "Yolo — auto-approve tools (most permissive)",
];

const MARKETPLACE_OPTIONS: &[&str] = &[
    "Continue — preference interview",
    "Skip interview — finish setup with current settings",
];

pub(crate) fn render_setup(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    match app.setup_phase {
        Some(SetupPhase::Approvals) => render_list_step(
            frame,
            area,
            "Setup · Permission mode",
            "Choose how NAVI treats tool calls by default. ↑/↓ then Enter.",
            APPROVAL_OPTIONS,
            app.setup_list_selected,
        ),
        Some(SetupPhase::MarketplaceTip) => render_list_step(
            frame,
            area,
            "Setup · Marketplace",
            "Extensions install as WASM packages (navi plugin search). ↑/↓ then Enter.",
            MARKETPLACE_OPTIONS,
            app.setup_list_selected,
        ),
        Some(SetupPhase::ProviderLogin) => render_banner(
            frame,
            area,
            "Setup · Provider",
            "Choose a provider and enter your API key in the model picker (ctrl+m).",
        ),
        Some(SetupPhase::MemoryModel) => render_banner(
            frame,
            area,
            "Setup · Memory model",
            "Pick the dedicated model for automatic memory extraction (Agents tab).",
        ),
        Some(SetupPhase::Interview) | None => {
            // Interview uses the normal chat UI underneath; show a thin banner.
            render_banner(
                frame,
                area,
                "Setup · Interview",
                "Answer the questions in chat. Setup finishes when the wizard marks complete.",
            );
        }
    }
}

fn render_banner(frame: &mut Frame<'_>, area: Rect, title: &str, body: &str) {
    // Compact top banner rather than full-screen modal during picker phases.
    let banner_h = 4u16.min(area.height.saturating_sub(1));
    if banner_h < 2 {
        return;
    }
    let banner = Rect {
        x: area.x.saturating_add(1),
        y: area.y,
        width: area.width.saturating_sub(2),
        height: banner_h,
    };
    clear_modal_area(frame, banner);
    frame.render_widget(modal_block(title), banner);
    let inner = banner.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    frame.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(muted()).bg(modal_bg())),
        inner,
    );
}

fn render_list_step(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    help: &str,
    options: &[&str],
    selected: usize,
) {
    clear_modal_area(frame, area);
    let width = area.width.min(72);
    let height = (options.len() as u16 + 6)
        .min(area.height.saturating_sub(2))
        .max(8);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let box_area = Rect {
        x,
        y,
        width,
        height,
    };
    frame.render_widget(modal_block(title), box_area);
    let inner = box_area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(help)
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(muted()).bg(modal_bg())),
        rows[0],
    );

    let items: Vec<ListItem> = options
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let marker = if i == selected { "▸ " } else { "  " };
            let style = if i == selected {
                Style::default()
                    .fg(text())
                    .bg(modal_bg())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(muted()).bg(modal_bg())
            };
            ListItem::new(Line::from(Span::styled(format!("{marker}{label}"), style))).style(style)
        })
        .collect();
    let mut state =
        ListState::default().with_selected(Some(selected.min(options.len().saturating_sub(1))));
    frame.render_stateful_widget(
        List::new(items).style(Style::default().bg(modal_bg())),
        rows[1],
        &mut state,
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("↑/↓", Style::default().fg(signal()).bg(modal_bg())),
            Span::styled(" move  ·  ", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled("enter", Style::default().fg(signal()).bg(modal_bg())),
            Span::styled(" select", Style::default().fg(muted()).bg(modal_bg())),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[2],
    );
}
