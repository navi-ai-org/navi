pub(crate) mod about;
pub(crate) mod background_commands;
pub(crate) mod chat;
pub(crate) mod command_palette;
pub(crate) mod debug;
pub(crate) mod help;
pub(crate) mod image_preview;
pub(crate) mod input;
pub(crate) mod modals;
pub(crate) mod model_picker;
pub(crate) mod notification;
pub(crate) mod plugins;
pub(crate) mod provider_settings;
pub(crate) mod sessions;
pub(crate) mod skills;
pub(crate) mod terminal_graphics;
pub(crate) mod update_modal;
pub(crate) mod welcome;

use ratatui::layout::Rect;
use ratatui::prelude::Frame;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Setup-specific rendering: welcome text or interview chat.
pub(crate) mod setup;

use crate::TuiApp;
use crate::render::{fill_modal_scrim, modal_rect, opaque_fill};
use crate::state::Mode;
use crate::theme::{self, bg, ghost, muted, text};
use crate::ui::viewport_rect;

pub(crate) fn render(frame: &mut Frame<'_>, app: &mut TuiApp) {
    theme::with_palette(&app.theme_palette(), || render_inner(frame, app));
}

fn render_inner(frame: &mut Frame<'_>, app: &mut TuiApp) {
    app.clear_interactions();
    let area = frame.area();
    opaque_fill(frame, area, Style::default().bg(theme::bg()));
    let content_area = viewport_rect(area);

    // Load plugin-registered TUI panels once after the session is started.
    if !app.plugin_panels_loaded && !app.session_id.as_str().is_empty() {
        crate::panels::load_plugin_panels(app);
        app.plugin_panels_loaded = true;
    }

    // Render region panels (header, chat, input, etc.) via the PanelManager.
    crate::panels::render_regions(frame, app, content_area);

    if modal_backdrop_active(app) {
        fill_modal_scrim(frame, content_area);
    }

    // Render overlay panels (modals, plugin panels) via the PanelManager.
    crate::panels::render_overlays(frame, app, area);

    if !app.pending_approvals.is_empty() {
        modals::render_tool_approval(frame, app, modal_rect(area, 72, 12));
    }

    notification::render_notification(frame, app, area);
    // Image hover preview sits above chat/composer (Kitty/Sixel when available).
    image_preview::render_image_hover_modal(frame, app, content_area);
}

fn modal_backdrop_active(app: &TuiApp) -> bool {
    app.mode != Mode::Normal || !app.pending_approvals.is_empty()
}

pub(crate) fn render_header(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let branch = app.git_branch.as_deref().unwrap_or("project");
    let spans = vec![
        Span::styled(" ", Style::default().fg(ghost()).bg(bg())),
        Span::styled(branch.to_string(), Style::default().fg(text()).bg(bg())),
        Span::styled("  ", Style::default().fg(ghost()).bg(bg())),
        Span::styled(
            project_path_label(app),
            Style::default().fg(muted()).bg(bg()),
        ),
    ];

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(bg())),
        area,
    );
}

fn project_path_label(app: &TuiApp) -> String {
    let path = &app.project_dir;
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::PathBuf::from(home);
        if let Ok(stripped) = path.strip_prefix(&home) {
            let stripped = stripped.to_string_lossy();
            return if stripped.is_empty() {
                "~".to_string()
            } else {
                format!("~/{stripped}")
            };
        }
    }
    path.to_string_lossy().to_string()
}

#[cfg(test)]
pub(crate) fn build_chat_lines(
    app: &mut TuiApp,
    chat_width: usize,
) -> Vec<ratatui::prelude::Line<'static>> {
    chat::build_chat_lines(app, chat_width)
}
