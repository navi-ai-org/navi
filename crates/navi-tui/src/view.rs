mod background_commands;
mod chat;
mod command_palette;
mod debug;
mod image_preview;
mod input;
pub(crate) mod mascot;
mod modals;
mod model_picker;
mod notification;
mod plugins;
mod provider_settings;
mod sessions;
mod skills;
mod welcome;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::Frame;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::TuiApp;
use crate::render::{fill_modal_scrim, modal_rect, opaque_fill};
use crate::state::Mode;
use crate::theme;
use crate::theme::{bg, ghost, muted, text};
use crate::ui::layer::{LayerStack, z};
use crate::ui::layout::viewport_rect;

enum Overlay {
    Mascot(Rect),
}

pub(crate) fn render(frame: &mut Frame<'_>, app: &mut TuiApp) {
    theme::with_palette(&app.theme_palette(), || render_inner(frame, app));
}

fn render_inner(frame: &mut Frame<'_>, app: &mut TuiApp) {
    app.clear_interactions();
    let area = frame.area();
    opaque_fill(frame, area, Style::default().bg(theme::bg()));
    let content_area = viewport_rect(area);
    let mut overlays = LayerStack::default();

    let input_width = content_area.width.saturating_sub(4) as usize;
    let input_height = input::composer_height(app, input_width);
    let input_hint_height = input::composer_hint_height(app);
    let image_preview_height = if app.pending_images.is_empty() {
        0
    } else {
        image_preview::IMAGE_PREVIEW_HEIGHT
    };
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(6),
            Constraint::Length(image_preview_height),
            Constraint::Length(input_height),
            Constraint::Length(input_hint_height),
        ])
        .split(content_area);

    render_header(frame, app, vertical[0]);
    chat::render_chat_area(frame, app, vertical[1]);

    // Render image previews above input
    let input_area = if !app.pending_images.is_empty() {
        image_preview::render_image_previews(frame, app, vertical[2]);
        vertical[3]
    } else {
        vertical[3]
    };

    if let Some(mascot_area) = input::render_input(frame, app, input_area) {
        overlays.push(z::FLOATING, Overlay::Mascot(mascot_area));
    }
    input::render_input_hint(frame, app, vertical[4]);

    for overlay in overlays.into_paint_order() {
        match overlay.item {
            Overlay::Mascot(area) => mascot::render_mascot(frame, app, area),
        }
    }

    if modal_backdrop_active(app) {
        fill_modal_scrim(frame, content_area);
    }

    match app.mode {
        Mode::Commands => command_palette::render(frame, app, modal_rect(area, 68, 15)),
        Mode::Models => model_picker::render(frame, app, modal_rect(area, 72, 22)),
        Mode::ApiKeyEntry => modals::render_api_key_entry(frame, app, modal_rect(area, 72, 11)),
        Mode::Thinking => modals::render_thinking_picker(frame, app, modal_rect(area, 40, 10)),
        Mode::Sessions => sessions::render(frame, app, modal_rect(area, 72, 16)),
        Mode::Settings => modals::render_settings(frame, app, modal_rect(area, 52, 12)),
        Mode::Providers => provider_settings::render(frame, app, modal_rect(area, 110, 26)),
        Mode::Debug => debug::render(frame, app, modal_rect(area, 76, 18)),
        Mode::Help => modals::render_help_modal(frame, app, modal_rect(area, 62, 16)),
        Mode::Skills => skills::render(frame, app, modal_rect(area, 72, 20)),
        Mode::Plugins => plugins::render(frame, app, modal_rect(area, 76, 22)),
        Mode::PluginApproval => {
            modals::render_plugin_approval(frame, app, modal_rect(area, 84, 24))
        }
        Mode::Question => modals::render_question(frame, app, modal_rect(area, 78, 22)),
        Mode::ThemePicker => modals::render_theme_picker(frame, app, modal_rect(area, 40, 12)),
        Mode::MessageActions => {
            modals::render_message_actions(frame, app, modal_rect(area, 58, 10))
        }
        Mode::Mcp => {
            let palette = app.theme_palette();
            crate::ui::mcp::draw_mcp_modal(frame, modal_rect(area, 90, 22), app, &palette)
        }
        Mode::OAuth => modals::render_oauth(frame, app, modal_rect(area, 78, 12)),
        Mode::BackgroundCommands => {
            background_commands::render(frame, app, modal_rect(area, 80, 20))
        }
        Mode::Normal => {}
    }

    if !app.pending_approvals.is_empty() {
        modals::render_tool_approval(frame, app, modal_rect(area, 72, 12));
    }

    notification::render_notification(frame, app, area);
}

fn modal_backdrop_active(app: &TuiApp) -> bool {
    app.mode != Mode::Normal || !app.pending_approvals.is_empty()
}

fn render_header(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let branch = app.git_branch.as_deref().unwrap_or("project");
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ", Style::default().fg(ghost()).bg(bg())),
            Span::styled(branch.to_string(), Style::default().fg(text()).bg(bg())),
            Span::styled("  ", Style::default().fg(ghost()).bg(bg())),
            Span::styled(
                project_path_label(app),
                Style::default().fg(muted()).bg(bg()),
            ),
        ]))
        .style(Style::default().bg(bg())),
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
                format!("~/{}", stripped)
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
