//! Live plan progress strip — keeps `app.active_plan` in sync with plan tools.

use navi_sdk::{ToolInvocation, ToolResult};

use crate::TuiApp;
use crate::state::{ActivePlanStepUi, ActivePlanUiState};

/// Update the composer plan strip from a completed `plan` tool call.
pub(crate) fn sync_from_plan_tool(
    app: &mut TuiApp,
    invocation: &ToolInvocation,
    result: &ToolResult,
) {
    if !result.ok {
        return;
    }
    let action = invocation
        .input
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match action {
        // A new plan always replaces the previous topbar plan (including done ones).
        "create" => {
            if let Some(mut plan) = plan_ui_from_tool_output(result) {
                plan.note_completed_if_needed();
                app.active_plan = Some(plan);
            }
        }
        "update" | "get" | "active" => {
            if let Some(mut plan) = plan_ui_from_tool_output(result) {
                // Don't overwrite a richer active plan with empty steps.
                if plan.steps.is_empty() && app.active_plan.is_some() {
                    if let Some(existing) = app.active_plan.as_mut() {
                        if !plan.plan_id.is_empty() {
                            existing.plan_id = plan.plan_id;
                        }
                        if !plan.title.is_empty() {
                            existing.title = plan.title;
                        }
                        if !plan.status.is_empty() {
                            existing.status = plan.status;
                        }
                        existing.note_completed_if_needed();
                    }
                } else {
                    // Preserve completed_at when refreshing the same finished plan.
                    if let Some(prev) = app.active_plan.as_ref() {
                        if prev.plan_id == plan.plan_id && prev.completed_at.is_some() {
                            plan.completed_at = prev.completed_at;
                        }
                    }
                    plan.note_completed_if_needed();
                    app.active_plan = Some(plan);
                }
            }
        }
        "complete_step" => {
            let Some(active) = app.active_plan.as_mut() else {
                return;
            };
            if let Some(idx) = result.output.get("step_index").and_then(|v| v.as_u64()) {
                active.mark_step_completed(idx as usize);
            }
            if result.output.get("all_complete").and_then(|v| v.as_bool()) == Some(true) {
                active.status = "completed".into();
                active.note_completed_if_needed();
            }
        }
        _ => {}
    }
}

fn plan_ui_from_tool_output(result: &ToolResult) -> Option<ActivePlanUiState> {
    let obj = result.output.as_object()?;
    // Nested `plan` (get/active) or top-level create fields.
    let plan = obj
        .get("plan")
        .and_then(|v| v.as_object())
        .unwrap_or(obj);

    let plan_id = plan
        .get("plan_id")
        .or_else(|| plan.get("id"))
        .or_else(|| obj.get("plan_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let title = plan
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Plan")
        .to_string();
    let status = plan
        .get("status")
        .or_else(|| obj.get("status"))
        .and_then(|v| v.as_str())
        .unwrap_or("active")
        .to_string();

    let steps: Vec<ActivePlanStepUi> = plan
        .get("steps")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    let desc = s
                        .get("description")
                        .or_else(|| s.get("content"))
                        .and_then(|v| v.as_str())?;
                    Some(ActivePlanStepUi {
                        description: desc.to_string(),
                        completed: s
                            .get("completed")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if title.is_empty() && steps.is_empty() {
        return None;
    }

    let mut ui = ActivePlanUiState {
        plan_id,
        title,
        steps,
        status,
        // Compact topbar by default (Grok-style N/M); click expands checklist.
        expanded: false,
        show_all_steps: false,
        completed_at: None,
    };
    ui.note_completed_if_needed();
    Some(ui)
}

/// Toggle expanded checklist in the plan topbar.
pub(crate) fn toggle_plan_expanded(app: &mut TuiApp) {
    if let Some(plan) = app.active_plan.as_mut() {
        plan.expanded = !plan.expanded;
        if !plan.expanded {
            // Collapse also resets the "+N more" full list.
            plan.show_all_steps = false;
        }
    }
}

/// Expand the remaining steps after "+N more" was clicked.
pub(crate) fn expand_plan_all_steps(app: &mut TuiApp) {
    if let Some(plan) = app.active_plan.as_mut() {
        plan.expanded = true;
        plan.show_all_steps = true;
    }
}

/// Drop a finished plan from the topbar after [`ActivePlanUiState::DONE_DISMISS_AFTER`].
///
/// Returns `true` when the active plan was cleared (caller should redraw).
pub(crate) fn maybe_dismiss_completed_plan(app: &mut TuiApp) -> bool {
    let should_dismiss = app
        .active_plan
        .as_ref()
        .is_some_and(ActivePlanUiState::should_auto_dismiss);
    if should_dismiss {
        app.active_plan = None;
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::test_app;
    use serde_json::json;
    use std::time::{Duration, Instant};

    fn inv(action: &str, extra: serde_json::Value) -> ToolInvocation {
        let mut input = json!({ "action": action });
        if let Some(obj) = extra.as_object() {
            if let Some(map) = input.as_object_mut() {
                for (k, v) in obj {
                    map.insert(k.clone(), v.clone());
                }
            }
        }
        ToolInvocation {
            id: "p1".into(),
            tool_name: "plan".into(),
            input,
        }
    }

    fn ok(output: serde_json::Value) -> ToolResult {
        ToolResult {
            invocation_id: "p1".into(),
            ok: true,
            output,
        }
    }

    #[test]
    fn create_replaces_previous_done_plan() {
        let mut app = test_app("");
        app.active_plan = Some(ActivePlanUiState {
            plan_id: "old".into(),
            title: "Old plan".into(),
            steps: vec![ActivePlanStepUi {
                description: "done step".into(),
                completed: true,
            }],
            status: "completed".into(),
            expanded: false,
            show_all_steps: false,
            completed_at: Some(Instant::now()),
        });

        sync_from_plan_tool(
            &mut app,
            &inv("create", json!({})),
            &ok(json!({
                "plan_id": "new",
                "title": "New plan",
                "status": "active",
                "steps": [
                    { "description": "first", "completed": false }
                ]
            })),
        );

        let plan = app.active_plan.expect("new plan");
        assert_eq!(plan.plan_id, "new");
        assert_eq!(plan.title, "New plan");
        assert!(!plan.is_done());
        assert!(plan.completed_at.is_none());
    }

    #[test]
    fn completed_plan_stamps_completed_at_and_dismisses_after_timeout() {
        let mut app = test_app("");
        sync_from_plan_tool(
            &mut app,
            &inv("create", json!({})),
            &ok(json!({
                "plan_id": "p",
                "title": "Ship",
                "status": "active",
                "steps": [
                    { "description": "a", "completed": false },
                    { "description": "b", "completed": false }
                ]
            })),
        );
        assert!(app.active_plan.is_some());

        // Finish all steps.
        let plan = app.active_plan.as_mut().unwrap();
        plan.mark_step_completed(0);
        plan.mark_step_completed(1);
        assert!(plan.is_done());
        assert!(plan.completed_at.is_some());

        // Not yet expired.
        assert!(!maybe_dismiss_completed_plan(&mut app));
        assert!(app.active_plan.is_some());

        // Backdate completion past the dismiss window.
        app.active_plan.as_mut().unwrap().completed_at =
            Some(Instant::now() - ActivePlanUiState::DONE_DISMISS_AFTER - Duration::from_secs(1));
        assert!(maybe_dismiss_completed_plan(&mut app));
        assert!(app.active_plan.is_none());
    }
}
