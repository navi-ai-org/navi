//! Plan progress topbar — sits under the project header, above the chat.
//!
//! Collapsed (default): one compact line with `N/M` (Grok-style `4/4 ✓`), clickable.
//! Expanded: checklist drops down into the space above chat.

use ratatui::layout::Rect;
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::TuiApp;
use crate::render::text::display_width;
use crate::theme::*;
use crate::ui::interaction::HitAction;

fn fit_display_width(value: &str, width: usize) -> String {
    if display_width(value) <= width {
        return value.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
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

/// Rows occupied by the plan topbar (0 when no active plan).
pub(crate) fn plan_topbar_height(app: &TuiApp) -> u16 {
    let Some(plan) = app.active_plan.as_ref() else {
        return 0;
    };
    if plan.steps.is_empty() && plan.title.is_empty() {
        return 0;
    }
    // Outer border (top+bottom) + summary, plus optional step rows.
    let mut body = 1u16; // summary
    if plan.expanded {
        let steps = plan.steps.len().min(8) as u16;
        body = body.saturating_add(steps);
        if plan.steps.len() > 8 {
            body = body.saturating_add(1); // "+N more"
        }
    }
    // top border + body + bottom border
    body.saturating_add(2).min(12)
}

pub(crate) fn render_plan_topbar(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let Some(plan) = app.active_plan.clone() else {
        return;
    };

    let fill = Style::default().bg(panel()).fg(text());
    let done = plan.completed_count();
    let total = plan.total_count();
    let all_done = (total > 0 && done >= total) || plan.status == "completed";
    let progress = if total == 0 {
        "—".to_string()
    } else if all_done {
        format!("{done}/{total} ✓")
    } else {
        format!("{done}/{total}")
    };
    let chevron = if plan.expanded { "▾" } else { "▸" };
    let status_tag = match plan.status.as_str() {
        "proposed" => "review",
        "completed" => "done",
        "abandoned" => "abandoned",
        _ => "",
    };

    let left_prefix = if status_tag.is_empty() {
        format!(" {chevron} ")
    } else {
        format!(" {chevron} {status_tag} · ")
    };
    let right = format!(" {progress} ");
    let progress_color = if all_done {
        signal()
    } else if plan.status == "proposed" {
        accent()
    } else {
        text()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ghost()).bg(panel()))
        .style(fill)
        .title(Span::styled(
            " plan ",
            Style::default().fg(muted()).bg(panel()),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Whole bar is clickable to expand/collapse (like Grok's 4/4 chip).
    app.register_hit(
        area,
        40,
        "toggle plan topbar",
        HitAction::TogglePlanTopbar,
    );

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let right_w = display_width(&right);
    let left_budget = (inner.width as usize).saturating_sub(right_w).max(4);
    // Collapsed: show title; if room, hint the current step after an em-dash.
    let mut title_text = plan.title.clone();
    if !plan.expanded
        && let Some(cur) = plan.current_step()
        && !cur.description.is_empty()
    {
        title_text = format!("{} — {}", plan.title, cur.description);
    }
    let title = fit_display_width(
        &title_text,
        left_budget.saturating_sub(display_width(&left_prefix)),
    );
    let left = format!("{left_prefix}{title}");
    let left_fitted = fit_display_width(&left, left_budget);
    let gap = (inner.width as usize)
        .saturating_sub(display_width(&left_fitted))
        .saturating_sub(right_w);

    let mut lines: Vec<Line<'static>> = vec![Line::from(vec![
        Span::styled(
            left_fitted,
            Style::default()
                .fg(text())
                .bg(panel())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ".repeat(gap), Style::default().bg(panel())),
        Span::styled(
            right,
            Style::default()
                .fg(progress_color)
                .bg(panel())
                .add_modifier(Modifier::BOLD),
        ),
    ])];

    if plan.expanded && inner.height > 1 {
        let show_n = plan
            .steps
            .len()
            .min(inner.height.saturating_sub(1) as usize)
            .min(8);
        let current_idx = plan.steps.iter().position(|s| !s.completed);
        for (i, step) in plan.steps.iter().enumerate().take(show_n) {
            let mark = if step.completed { "✓" } else { "○" };
            let is_current = current_idx == Some(i);
            let prefix = if is_current { "›" } else { " " };
            let color = if step.completed {
                signal()
            } else if is_current {
                accent()
            } else {
                muted()
            };
            let row = format!(" {prefix} {mark} {}. {}", i + 1, step.description);
            lines.push(Line::from(Span::styled(
                fit_display_width(&row, inner.width as usize),
                Style::default().fg(color).bg(panel()),
            )));
        }
        if plan.steps.len() > show_n && lines.len() < inner.height as usize {
            lines.push(Line::from(Span::styled(
                format!("    … +{} more", plan.steps.len() - show_n),
                Style::default().fg(ghost()).bg(panel()),
            )));
        }
    }

    let body_h = (lines.len() as u16).min(inner.height);
    let body = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: body_h,
    };
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(panel())),
        body,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ActivePlanStepUi, ActivePlanUiState};
    use crate::tests::test_app;

    #[test]
    fn topbar_height_collapsed_is_compact() {
        let mut app = test_app("");
        assert_eq!(plan_topbar_height(&app), 0);
        app.active_plan = Some(ActivePlanUiState {
            plan_id: "p1".into(),
            title: "Ship it".into(),
            steps: vec![
                ActivePlanStepUi {
                    description: "a".into(),
                    completed: true,
                },
                ActivePlanStepUi {
                    description: "b".into(),
                    completed: false,
                },
            ],
            status: "active".into(),
            expanded: false,
        });
        // borders(2) + summary(1)
        assert_eq!(plan_topbar_height(&app), 3);
        app.active_plan.as_mut().unwrap().expanded = true;
        // borders(2) + summary(1) + 2 steps
        assert_eq!(plan_topbar_height(&app), 5);
    }
}
