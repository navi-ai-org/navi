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
        "create" | "update" | "get" | "active" => {
            if let Some(plan) = plan_ui_from_tool_output(result) {
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
                    }
                } else {
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

    Some(ActivePlanUiState {
        plan_id,
        title,
        steps,
        status,
        expanded: true,
    })
}

/// Toggle expanded checklist under the composer.
#[allow(dead_code)]
pub(crate) fn toggle_plan_expanded(app: &mut TuiApp) {
    if let Some(plan) = app.active_plan.as_mut() {
        plan.expanded = !plan.expanded;
    }
}
