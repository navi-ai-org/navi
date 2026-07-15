use ratatui::prelude::{Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::text::Text;

use crate::TuiApp;
use crate::providers::selected_provider_label;
use crate::render::project_label;
use crate::render::text::display_width;
use crate::theme::*;

const FULL_WELCOME_MIN_WIDTH: usize = 74;
const FULL_WELCOME_MIN_HEIGHT: usize = 13;

pub(super) fn welcome_text(app: &TuiApp, width: usize, height: usize) -> Text<'static> {
    if width < FULL_WELCOME_MIN_WIDTH || height < FULL_WELCOME_MIN_HEIGHT {
        return compact_welcome_text(app, width, height);
    }

    let mut lines = Vec::new();
    let logo_width = NAVI_COMPACT_LOGO
        .iter()
        .map(|line| display_width(line))
        .max()
        .unwrap_or(0);
    let project = project_label();
    let model = app.loaded_config.config.model.name.clone();
    let provider = selected_provider_label(app).to_string();
    let binary_effort =
        crate::state::ThinkingLevel::is_binary_for_model(app.models.get(app.selected_model));
    let effort = app.thinking_level.display_label(binary_effort);
    let context = app.compact_state.usage_label(0);
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
        effort.len() + 11,
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
            let color = animated_logo_color(app.tick(), index);
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
            index, &project, &provider, &model, effort, &context, &mode, &router, &tools, &session,
            &cost,
        ) {
            spans.push(Span::raw("      "));
            spans.extend(status);
        }
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));

    Text::from(lines)
}

fn compact_welcome_text(app: &TuiApp, width: usize, height: usize) -> Text<'static> {
    let width = width.max(1);
    let project = project_label();
    let model = app.loaded_config.config.model.name.clone();
    let provider = selected_provider_label(app).to_string();
    let binary_effort =
        crate::state::ThinkingLevel::is_binary_for_model(app.models.get(app.selected_model));
    let effort = app.thinking_level.display_label(binary_effort);
    let context = app.compact_state.usage_label(0);
    let session = if app.conversation_history.len() <= 1 {
        "new"
    } else {
        "resumed"
    };

    let rows = [
        compact_title_line(app.tick()),
        compact_status_line("project", &project, width),
        compact_status_line("model", &model, width),
        compact_status_line("via", &provider, width),
        compact_status_line("effort", effort, width),
        compact_status_line("context", &context, width),
        compact_status_line("session", session, width),
    ];

    let max_rows = if height <= 7 {
        height.saturating_sub(1).max(1)
    } else {
        rows.len()
    };
    Text::from(rows.into_iter().take(max_rows).collect::<Vec<_>>())
}

fn compact_title_line(tick: u64) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "NAVI",
            Style::default()
                .fg(animated_logo_color(tick, 0))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" local agent", Style::default().fg(muted())),
    ])
}

fn compact_status_line(label: &str, value: &str, width: usize) -> Line<'static> {
    let label_text = format!("{label} ");
    let label_width = display_width(&label_text);
    let value_width = width.saturating_sub(label_width).max(1);
    let value = fit_display_width(value, value_width);
    Line::from(vec![
        Span::styled(label_text, Style::default().fg(muted())),
        Span::styled(
            value,
            Style::default().fg(text()).add_modifier(Modifier::BOLD),
        ),
    ])
}

fn fit_display_width(value: &str, width: usize) -> String {
    if display_width(value) <= width {
        return value.to_string();
    }
    if width <= 1 {
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

fn animated_logo_color(tick: u64, row: usize) -> ratatui::style::Color {
    let frame = (tick / 6) as usize;
    let band = frame % NAVI_COMPACT_LOGO.len().max(1);
    let trail = band.saturating_sub(1);
    if row == band || row == trail {
        signal()
    } else {
        accent()
    }
}

#[allow(clippy::too_many_arguments)]
fn welcome_status_line(
    index: usize,
    project: &str,
    provider: &str,
    model: &str,
    effort: &str,
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
            Span::styled("effort  ", Style::default().fg(muted())),
            Span::styled(effort.to_string(), Style::default().fg(text())),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn compact_welcome_avoids_large_logo_on_phone_sized_viewport() {
        let app = crate::tests::test_app("");
        let text = welcome_text(&app, 34, 6);
        let lines = text.lines.iter().map(line_text).collect::<Vec<_>>();

        assert!(!lines.iter().any(|line| line.contains("██")));
        assert!(lines.iter().any(|line| line.starts_with("NAVI")));
        assert!(
            lines
                .iter()
                .all(|line| crate::render::text::display_width(line) <= 34)
        );
    }
}
