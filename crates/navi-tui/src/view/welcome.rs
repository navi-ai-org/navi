use ratatui::prelude::{Line, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;

use crate::TuiApp;
use crate::providers::selected_provider_label;
use crate::render::project_label;
use crate::theme::*;

pub(super) fn welcome_text(app: &TuiApp, width: usize) -> Text<'static> {
    let mut lines = Vec::new();
    let logo_width = NAVI_COMPACT_LOGO
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let project = project_label();
    let model = app.loaded_config.config.model.name.clone();
    let provider = selected_provider_label(app).to_string();
    let thinking = app.thinking_level.label();
    let context = app.compact_state.usage_label(app.input.len());
    let mode = format!("{:?}", app.loaded_config.config.harness.profile).to_lowercase();
    let router = "auto".to_string();
    let tools = "shell read write grep patch".to_string();
    let session = if app.conversation_history.len() <= 1 {
        "new"
    } else {
        "resumed"
    }
    .to_string();
    let cost = "$0.00".to_string();

    let status_width = [
        project.chars().count() + 10,
        model.chars().count() + provider.chars().count() + 9,
        thinking.len() + 13,
        context.len() + 9,
        mode.len() + 9,
        router.len() + 9,
        tools.len() + 9,
        session.len() + 9,
        cost.len() + 9,
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    let content_width = logo_width + 6 + status_width;
    let left_pad = width.saturating_sub(content_width) / 2;

    lines.push(Line::from(""));

    let total_lines = std::cmp::max(NAVI_COMPACT_LOGO.len(), 10);

    for index in 0..total_lines {
        let mut spans = Vec::new();

        if let Some(logo_line) = NAVI_COMPACT_LOGO.get(index) {
            let color = match (app.tick() / 5 + index as u64) % 4 {
                0 => pink(),
                1 => accent(),
                2 => Color::Rgb(236, 218, 255),
                _ => Color::Rgb(132, 20, 204),
            };
            spans.push(Span::styled(
                format!("{}{logo_line}", " ".repeat(left_pad)),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::raw(format!(
                "{}{}",
                " ".repeat(left_pad),
                " ".repeat(logo_width)
            )));
        }

        if let Some(status) = welcome_status_line(
            index, &project, &provider, &model, thinking, &context, &mode, &router, &tools,
            &session, &cost,
        ) {
            spans.push(Span::raw("      "));
            spans.extend(status);
        }
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        format!(
            "{}NAVI · wired code agent for local-first builders",
            " ".repeat(left_pad)
        ),
        Style::default().fg(muted()),
    )]));

    Text::from(lines)
}

fn welcome_status_line(
    index: usize,
    project: &str,
    provider: &str,
    model: &str,
    thinking: &str,
    context: &str,
    mode: &str,
    router: &str,
    tools: &str,
    session: &str,
    cost: &str,
) -> Option<Vec<Span<'static>>> {
    match index {
        0 => Some(vec![
            Span::styled("project ", Style::default().fg(muted())),
            Span::styled(
                project.to_string(),
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
        ]),
        1 => Some(vec![
            Span::styled("model   ", Style::default().fg(muted())),
            Span::styled(
                model.to_string(),
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
        ]),
        2 => Some(vec![
            Span::styled("via     ", Style::default().fg(muted())),
            Span::styled(
                provider.to_string(),
                Style::default().fg(accent()).add_modifier(Modifier::BOLD),
            ),
        ]),
        3 => Some(vec![
            Span::styled("thinking ", Style::default().fg(muted())),
            Span::styled(thinking.to_string(), Style::default().fg(text())),
        ]),
        4 => Some(vec![
            Span::styled("context ", Style::default().fg(muted())),
            Span::styled(context.to_string(), Style::default().fg(text())),
        ]),
        5 => Some(vec![
            Span::styled("mode    ", Style::default().fg(muted())),
            Span::styled(mode.to_string(), Style::default().fg(text())),
        ]),
        6 => Some(vec![
            Span::styled("router  ", Style::default().fg(muted())),
            Span::styled(router.to_string(), Style::default().fg(text())),
        ]),
        7 => Some(vec![
            Span::styled("tools   ", Style::default().fg(muted())),
            Span::styled(tools.to_string(), Style::default().fg(text())),
        ]),
        8 => Some(vec![
            Span::styled("session ", Style::default().fg(muted())),
            Span::styled(session.to_string(), Style::default().fg(text())),
        ]),
        9 => Some(vec![
            Span::styled("cost    ", Style::default().fg(muted())),
            Span::styled(cost.to_string(), Style::default().fg(text())),
        ]),
        _ => None,
    }
}
