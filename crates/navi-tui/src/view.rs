pub(crate) mod background_commands;
pub(crate) mod chat;
pub(crate) mod command_palette;
pub(crate) mod debug;
pub(crate) mod image_preview;
pub(crate) mod input;
pub(crate) mod modals;
pub(crate) mod model_picker;
pub(crate) mod notification;
pub(crate) mod plugins;
pub(crate) mod provider_settings;
pub(crate) mod sessions;
pub(crate) mod skills;
pub(crate) mod welcome;

use ratatui::layout::Rect;
use ratatui::prelude::Frame;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::TuiApp;
use crate::render::{fill_modal_scrim, modal_rect, opaque_fill};
use crate::state::Mode;
use crate::theme::{self, bg, ghost, muted, text};
use crate::ui::{RootLayoutHeights, root_layout, viewport_rect};

/// Setup-specific rendering: welcome text or interview chat.
mod setup;

pub(crate) fn render(frame: &mut Frame<'_>, app: &mut TuiApp) {
    theme::with_palette(&app.theme_palette(), || render_inner(frame, app));
}

fn render_inner(frame: &mut Frame<'_>, app: &mut TuiApp) {
    app.clear_interactions();
    let area = frame.area();
    opaque_fill(frame, area, Style::default().bg(theme::bg()));
    let content_area = viewport_rect(area);

    let input_width = composer_text_width(app, content_area.width);
    let compact_viewport = content_area.width < 64 || content_area.height < 18;
    let mut input_height = input::composer_height(app, input_width);
    if compact_viewport {
        input_height = input_height.min(3);
    }
    let input_hint_height = if compact_viewport {
        0
    } else {
        input::composer_hint_height(app)
    };
    let image_preview_height = 0;
    let input_activity_height = input::composer_activity_height(app);
    let layout = root_layout(
        content_area,
        RootLayoutHeights {
            header: 1,
            image_preview: image_preview_height,
            input_activity: input_activity_height,
            input: input_height,
            input_hint: input_hint_height,
        },
    );

    render_header(frame, app, layout.header);
    chat::render_chat_area(frame, app, layout.chat);

    // Render image previews above input
    let input_area = layout.input;

    input::render_input_activity(frame, app, layout.input_activity);
    input::render_input(frame, app, input_area);
    input::render_input_hint(frame, app, layout.input_hint);

    if modal_backdrop_active(app) {
        fill_modal_scrim(frame, content_area);
    }

    // Render modals via the PanelManager (copland) for all standard modes.
    // Special cases that need &mut TuiApp or extra params are handled below.
    crate::panels::render_overlays(frame, app, area);

    // Special-case modals that don't fit the ModalPanel pattern yet.
    match app.mode {
        Mode::Mcp => {
            let palette = app.theme_palette();
            crate::ui::mcp::draw_mcp_modal(frame, modal_rect(area, 90, 22), app, &palette)
        }
        Mode::Setup => setup::render_setup(frame, app, content_area),
        _ => {}
    }

    if !app.pending_approvals.is_empty() {
        modals::render_tool_approval(frame, app, modal_rect(area, 72, 12));
    }

    notification::render_notification(frame, app, area);
}

fn composer_text_width(_app: &TuiApp, width: u16) -> usize {
    width.saturating_sub(4) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composer_areas_use_full_width() {
        let app = crate::tests::test_app("");
        let width = composer_text_width(&app, 100);
        assert_eq!(width, 96);
    }
}

fn modal_backdrop_active(app: &TuiApp) -> bool {
    app.mode != Mode::Normal || !app.pending_approvals.is_empty()
}

fn render_header(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
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
