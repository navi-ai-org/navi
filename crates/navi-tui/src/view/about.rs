//! About NAVI modal — product blurb + external links.

use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Paragraph, Wrap};

use crate::TuiApp;
use crate::render::{clear_modal_area, modal_block};
use crate::state::ModalKind;
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};

pub(crate) const GITHUB_URL: &str = "https://github.com/navi-ai-org/navi";
pub(crate) const HUGGINGFACE_URL: &str = "https://huggingface.co/navi-org";
pub(crate) const RELEASES_URL: &str = "https://github.com/navi-ai-org/navi/releases";

const ABOUT_BLURB: &str = "NAVI is the coding agent engine that lives in your terminal — \
same harness for TUI, headless, edge, and apps. Multi-provider. Built in Rust. \
Low memory. Local-first agent workflows with tools, memory, plugins, and voice.";

#[derive(Debug, Clone, Copy)]
pub(crate) enum AboutLink {
    GitHub,
    HuggingFace,
    Releases,
}

impl AboutLink {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::HuggingFace => "Hugging Face",
            Self::Releases => "Releases",
        }
    }

    pub(crate) fn url(self) -> &'static str {
        match self {
            Self::GitHub => GITHUB_URL,
            Self::HuggingFace => HUGGINGFACE_URL,
            Self::Releases => RELEASES_URL,
        }
    }

    pub(crate) fn all() -> &'static [AboutLink] {
        &[Self::GitHub, Self::HuggingFace, Self::Releases]
    }
}

pub(crate) fn open_about(app: &mut TuiApp) {
    app.selected_about_link = 0;
    crate::keybindings::replace_modal(app, ModalKind::About);
}

pub(crate) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let version = env!("CARGO_PKG_VERSION");
    frame.render_widget(modal_block(&format!("About NAVI  v{version}")), area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(4),
            Constraint::Length(4),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(ABOUT_BLURB)
            .style(Style::default().fg(text()).bg(modal_bg()))
            .wrap(Wrap { trim: true }),
        rows[0],
    );

    let links = AboutLink::all();
    for (i, link) in links.iter().enumerate() {
        let selected = i == app.selected_about_link;
        let style = if selected {
            Style::default()
                .fg(signal())
                .bg(modal_bg())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(muted()).bg(modal_bg())
        };
        let marker = if selected { "› " } else { "  " };
        let line_area = line_rect(rows[1], i);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(marker, style),
                Span::styled(format!("{} — {}", link.label(), link.url()), style),
            ]))
            .style(Style::default().bg(modal_bg())),
            line_area,
        );
        app.register_hit(
            line_area,
            25,
            format!("about {}", link.label()),
            HitAction::AboutLink(i),
        );
    }

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("enter", Style::default().fg(red()).bg(modal_bg())),
            Span::styled(
                " open link  ·  ",
                Style::default().fg(muted()).bg(modal_bg()),
            ),
            Span::styled("↑↓", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(
                " select  ·  ",
                Style::default().fg(muted()).bg(modal_bg()),
            ),
            Span::styled("esc", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(" close", Style::default().fg(muted()).bg(modal_bg())),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[2],
    );
}

pub(crate) fn open_selected_link(app: &mut TuiApp) {
    let links = AboutLink::all();
    let Some(link) = links.get(app.selected_about_link) else {
        return;
    };
    open_link(app, *link);
}

pub(crate) fn open_link(app: &mut TuiApp, link: AboutLink) {
    match navi_core::open_url(link.url()) {
        Ok(()) => {
            crate::notifications::show_notification(
                app,
                "Opened",
                format!("Opening {}…", link.label()),
            );
        }
        Err(err) => {
            crate::notifications::show_notification(
                app,
                "Open failed",
                format!("{err} — {url}", url = link.url()),
            );
        }
    }
}
