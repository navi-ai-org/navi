mod background_commands;
mod chat;
mod command_palette;
mod debug;
mod image_preview;
mod input;
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
use crate::theme::{self, bg, ghost, muted, text};
use crate::ui::layout::viewport_rect;

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
    let image_preview_height = if app.pending_images.is_empty() {
        0
    } else if compact_viewport {
        4
    } else {
        image_preview::IMAGE_PREVIEW_HEIGHT
    };
    let input_activity_height = input::composer_activity_height(app);
    let chat_min_height = if compact_viewport { 1 } else { 6 };
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(chat_min_height),
            Constraint::Length(image_preview_height),
            Constraint::Length(input_activity_height),
            Constraint::Length(input_height),
            Constraint::Length(input_hint_height),
        ])
        .split(content_area);

    render_header(frame, app, vertical[0]);
    chat::render_chat_area(frame, app, vertical[1]);

    // Render image previews above input
    let input_area = if !app.pending_images.is_empty() {
        image_preview::render_image_previews(frame, app, vertical[2]);
        vertical[4]
    } else {
        vertical[4]
    };

    input::render_input_activity(frame, app, vertical[3]);
    input::render_input(frame, app, input_area);
    input::render_input_hint(frame, app, vertical[5]);

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
        Mode::Usage => modals::render_usage(frame, app, modal_rect(area, 78, 18)),
        Mode::Debug => debug::render(frame, app, modal_rect(area, 76, 18)),
        Mode::Help => modals::render_help_modal(frame, app, modal_rect(area, 62, 19)),
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
        Mode::BackgroundCommandOutput => {
            background_commands::render_output(frame, app, modal_rect(area, 110, 30))
        }
        Mode::BackgroundModels => {
            modals::render_background_models(frame, app, modal_rect(area, 70, 14))
        }
        Mode::BgModelPicker => model_picker::render(frame, app, modal_rect(area, 72, 22)),
        Mode::Normal => {}
        Mode::Setup => setup::render_setup(frame, app, content_area),
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
    let mut spans = vec![
        Span::styled(" ", Style::default().fg(ghost()).bg(bg())),
        Span::styled(branch.to_string(), Style::default().fg(text()).bg(bg())),
        Span::styled("  ", Style::default().fg(ghost()).bg(bg())),
        Span::styled(
            project_path_label(app),
            Style::default().fg(muted()).bg(bg()),
        ),
    ];

    if let Some(goal) = &app.goal_state {
        spans.push(Span::styled("  │  Goal: ", Style::default().fg(ghost()).bg(bg())));
        let status_color = if goal.active {
            crate::theme::accent()
        } else if goal.status == navi_sdk::GoalStatus::Blocked || goal.status == navi_sdk::GoalStatus::BudgetLimited {
            crate::theme::red()
        } else {
            muted()
        };
        spans.push(Span::styled(goal.objective.clone(), Style::default().fg(status_color).bg(bg())));
        
        if let Some(budget) = goal.token_budget {
            let percent = (goal.tokens_used as f64 / budget as f64 * 100.0).round() as i32;
            spans.push(Span::styled(format!(" ({}%)", percent), Style::default().fg(muted()).bg(bg())));
        }
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans))
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
