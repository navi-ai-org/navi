use crate::event::AgentEvent;
use crate::goal::runtime::GoalRuntimeHandle;
use crate::goal::types::SessionGoal;
use crate::tool::{
    Tool, ToolDefinition, ToolInvocation, ToolInvocationContext, ToolKind, ToolMetadata, ToolResult,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

// ── Helpers ──────────────────────────────────────────────────

fn make_result(invocation_id: &str, ok: bool, output: Value) -> ToolResult {
    ToolResult {
        invocation_id: invocation_id.to_string(),
        ok,
        output,
    }
}

fn goal_with_status(goal: &SessionGoal, status: super::types::GoalStatus) -> SessionGoal {
    let mut new_goal = goal.clone();
    new_goal.status = status;
    new_goal.updated_at = crate::session::current_unix_timestamp();
    new_goal
}

fn limit_short_description(value: &str) -> String {
    value.trim().chars().take(40).collect()
}

/// Notify the session event stream so runtime `record_event` fans out GoalUpdated.
fn emit_goal_updated(context: &ToolInvocationContext, goal: &SessionGoal) {
    if let Some(tx) = &context.event_tx {
        let _ = tx.send(AgentEvent::GoalUpdated {
            session_id: goal.session_id.clone(),
            goal_id: goal.goal_id.as_str().to_string(),
            objective: goal.objective.clone(),
            short_description: goal.short_description.clone(),
            status: goal.status,
            tokens_used: goal.tokens_used,
            token_budget: goal.token_budget,
        });
    }
}

// ── get_goal ─────────────────────────────────────────────────

pub struct GetGoalTool {
    runtime: Arc<GoalRuntimeHandle>,
}

impl GetGoalTool {
    pub fn new(runtime: Arc<GoalRuntimeHandle>) -> Self {
        Self { runtime }
    }

    pub fn definition_static() -> ToolDefinition {
        ToolDefinition {
            name: "get_goal".to_string(),
            description:
                "Read the current session goal. Returns the objective, status, tokens used, \
                 remaining budget, time elapsed, and blocked-turn count. Use this to check \
                 progress before continuing work or marking the goal complete."
                    .to_string(),
            kind: ToolKind::Read,
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            metadata: ToolMetadata {
                tags: vec!["goal".to_string(), "session".to_string()],
                capabilities: vec!["goal.read".to_string()],
                exposure: crate::tool::ToolExposure::Direct,
                ..ToolMetadata::default()
            },
        }
    }
}

#[async_trait]
impl Tool for GetGoalTool {
    fn definition(&self) -> ToolDefinition {
        Self::definition_static()
    }

    async fn invoke(&self, invocation: ToolInvocation) -> anyhow::Result<ToolResult> {
        let result = match self.runtime.get_goal() {
            Some(goal) => make_result(
                &invocation.id,
                true,
                json!({
                    "goal_id": goal.goal_id.as_str(),
                    "objective": goal.objective,
                    "short_description": goal.short_description,
                    "status": goal.status.as_str(),
                    "tokens_used": goal.tokens_used,
                    "token_budget": goal.token_budget,
                    "remaining_budget": goal.remaining_budget(),
                    "time_used_seconds": goal.time_used_seconds,
                    "consecutive_blocked_turns": goal.consecutive_blocked_turns,
                    "block_reason": goal.block_reason,
                    "created_at": goal.created_at,
                    "updated_at": goal.updated_at
                }),
            ),
            None => make_result(
                &invocation.id,
                true,
                json!({
                    "active": false,
                    "message": "No goal is currently set for this session."
                }),
            ),
        };
        Ok(result)
    }

    async fn invoke_with_context(
        &self,
        invocation: ToolInvocation,
        _context: ToolInvocationContext,
    ) -> anyhow::Result<ToolResult> {
        self.invoke(invocation).await
    }
}

// ── create_goal ──────────────────────────────────────────────

pub struct CreateGoalTool {
    runtime: Arc<GoalRuntimeHandle>,
}

impl CreateGoalTool {
    pub fn new(runtime: Arc<GoalRuntimeHandle>) -> Self {
        Self { runtime }
    }

    pub fn definition_static() -> ToolDefinition {
        ToolDefinition {
            name: "create_goal".to_string(),
            description:
                "Create a new goal for this session. Only call this when the user explicitly \
                 asks you to define a goal. The goal will persist across turns and the agent \
                 will auto-continue making progress."
                    .to_string(),
            kind: ToolKind::Write,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "objective": {
                        "type": "string",
                        "description": "The textual objective of the goal."
                    },
                    "short_description": {
                        "type": "string",
                        "maxLength": 40,
                        "description": "Short TUI label for this goal. Summarize the goal in a few words; maximum 40 characters."
                    },
                    "token_budget": {
                        "type": "integer",
                        "description": "Optional maximum number of tokens to spend on this goal."
                    }
                },
                "required": ["objective"],
                "additionalProperties": false
            }),
            metadata: ToolMetadata {
                tags: vec!["goal".to_string(), "session".to_string()],
                capabilities: vec!["goal.create".to_string()],
                exposure: crate::tool::ToolExposure::Direct,
                ..ToolMetadata::default()
            },
        }
    }
}

#[async_trait]
impl Tool for CreateGoalTool {
    fn definition(&self) -> ToolDefinition {
        Self::definition_static()
    }

    async fn invoke(&self, invocation: ToolInvocation) -> anyhow::Result<ToolResult> {
        self.invoke_with_context(invocation, ToolInvocationContext::default())
            .await
    }

    async fn invoke_with_context(
        &self,
        invocation: ToolInvocation,
        context: ToolInvocationContext,
    ) -> anyhow::Result<ToolResult> {
        let args = invocation.input.clone();

        let objective = args
            .get("objective")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if objective.is_empty() {
            return Ok(make_result(
                &invocation.id,
                false,
                json!({"error": "objective is required"}),
            ));
        }

        let token_budget = args.get("token_budget").and_then(|v| v.as_i64());
        let short_description = args
            .get("short_description")
            .and_then(|v| v.as_str())
            .map(limit_short_description);
        let goal = self.runtime.set_objective_with_short_description(
            objective,
            short_description,
            token_budget,
        );
        self.runtime.set_auto_continue(true);
        emit_goal_updated(&context, &goal);

        Ok(make_result(
            &invocation.id,
            true,
            json!({
                "created": true,
                "goal_id": goal.goal_id.as_str(),
                "objective": goal.objective,
                "short_description": goal.short_description,
                "status": goal.status.as_str(),
                "token_budget": goal.token_budget,
                "message": "Goal created successfully. The agent will auto-continue working toward this goal."
            }),
        ))
    }
}

// ── update_goal ──────────────────────────────────────────────

pub struct UpdateGoalTool {
    runtime: Arc<GoalRuntimeHandle>,
}

impl UpdateGoalTool {
    pub fn new(runtime: Arc<GoalRuntimeHandle>) -> Self {
        Self { runtime }
    }

    pub fn definition_static() -> ToolDefinition {
        ToolDefinition {
            name: "update_goal".to_string(),
            description: "Update the current goal status. Use this to:\n\
                 - Mark the goal as `complete` when the objective is verified and done.\n\
                 - Mark the goal as `blocked` when the same blocker has persisted for 3+ \
                   consecutive turns (include the blocker description).\n\
                 - Pause or resume the goal."
                .to_string(),
            kind: ToolKind::Write,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["complete", "blocked", "pause", "resume"],
                        "description": "The action to perform on the goal."
                    },
                    "reason": {
                        "type": "string",
                        "description": "Description of the blocker (required for 'blocked' action)."
                    }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
            metadata: ToolMetadata {
                tags: vec!["goal".to_string(), "session".to_string()],
                capabilities: vec!["goal.update".to_string()],
                exposure: crate::tool::ToolExposure::Direct,
                ..ToolMetadata::default()
            },
        }
    }
}

#[async_trait]
impl Tool for UpdateGoalTool {
    fn definition(&self) -> ToolDefinition {
        Self::definition_static()
    }

    async fn invoke(&self, invocation: ToolInvocation) -> anyhow::Result<ToolResult> {
        self.invoke_with_context(invocation, ToolInvocationContext::default())
            .await
    }

    async fn invoke_with_context(
        &self,
        invocation: ToolInvocation,
        context: ToolInvocationContext,
    ) -> anyhow::Result<ToolResult> {
        let args = invocation.input.clone();
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");

        let goal = match self.runtime.get_goal() {
            Some(g) => g,
            None => {
                return Ok(make_result(
                    &invocation.id,
                    false,
                    json!({"error": "No goal is currently set. Use create_goal to set one first."}),
                ));
            }
        };

        use super::types::GoalStatus;
        if goal.status.is_terminal() && matches!(action, "blocked" | "pause" | "resume") {
            return Ok(make_result(
                &invocation.id,
                false,
                json!({
                    "error": format!(
                        "Goal is terminal with status `{}` and cannot be changed with `{}`.",
                        goal.status.as_str(),
                        action
                    )
                }),
            ));
        }

        let result = match action {
            "complete" => {
                // Enforce checklist completion before allowing the goal to be marked complete.
                if !goal.checklist.is_empty() && !goal.is_checklist_complete() {
                    let unfinished: Vec<String> = goal
                        .checklist
                        .iter()
                        .filter(|t| !t.status.is_finished())
                        .map(|t| format!("  [{}] {}", t.status, t.description))
                        .collect();
                    return Ok(make_result(
                        &invocation.id,
                        false,
                        json!({
                            "error": "Cannot mark goal as complete: checklist has unfinished tasks.",
                            "unfinished_tasks": unfinished,
                            "finished": goal.finished_count(),
                            "total": goal.checklist.len(),
                            "message": "Complete or skip all checklist tasks before marking the goal as complete. Use update_goal_checklist to update task statuses."
                        }),
                    ));
                }
                if goal.checklist.is_empty() {
                    return Ok(make_result(
                        &invocation.id,
                        false,
                        json!({
                            "error": "Cannot mark goal as complete: no checklist has been defined.",
                            "message": "Use update_goal_checklist to define a task checklist first. Decompose the objective into concrete, verifiable tasks."
                        }),
                    ));
                }
                let updated = goal_with_status(&goal, GoalStatus::Complete);
                self.runtime.update_goal(updated.clone());
                emit_goal_updated(&context, &updated);
                make_result(
                    &invocation.id,
                    true,
                    json!({
                        "updated": true,
                        "status": "complete",
                        "verified_tasks": goal.verified_count(),
                        "total_tasks": goal.checklist.len(),
                        "message": "Goal marked as complete. All checklist tasks verified."
                    }),
                )
            }
            "blocked" => {
                let reason = args
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let became_blocked = self.runtime.record_blocked_turn(reason);
                let mut new_goal = self.runtime.get_goal().unwrap_or_else(|| goal.clone());
                if became_blocked {
                    new_goal.status = GoalStatus::Blocked;
                }
                new_goal.block_reason = Some(reason.to_string());
                self.runtime.update_goal(new_goal.clone());
                emit_goal_updated(&context, &new_goal);
                make_result(
                    &invocation.id,
                    true,
                    json!({
                        "updated": true,
                        "status": new_goal.status.as_str(),
                        "consecutive_blocked_turns": new_goal.consecutive_blocked_turns,
                        "block_reason": reason,
                        "message": if became_blocked {
                            "Goal blocked after 3+ consecutive blocked turns."
                        } else {
                            "Blocked turn recorded."
                        }
                    }),
                )
            }
            "pause" => {
                let updated = goal_with_status(&goal, GoalStatus::Paused);
                self.runtime.update_goal(updated.clone());
                self.runtime.set_auto_continue(false);
                emit_goal_updated(&context, &updated);
                make_result(
                    &invocation.id,
                    true,
                    json!({
                        "updated": true,
                        "status": "paused",
                        "message": "Goal paused. Auto-continuation disabled."
                    }),
                )
            }
            "resume" => {
                let updated = goal_with_status(&goal, GoalStatus::Active);
                self.runtime.update_goal(updated.clone());
                self.runtime.set_auto_continue(true);
                emit_goal_updated(&context, &updated);
                make_result(
                    &invocation.id,
                    true,
                    json!({
                        "updated": true,
                        "status": "active",
                        "message": "Goal resumed. Auto-continuation enabled."
                    }),
                )
            }
            other => make_result(
                &invocation.id,
                false,
                json!({"error": format!("Unknown action `{other}`. Valid actions: complete, blocked, pause, resume.")}),
            ),
        };
        Ok(result)
    }
}

// ── update_goal_checklist ─────────────────────────────────────

pub struct UpdateGoalChecklistTool {
    runtime: Arc<GoalRuntimeHandle>,
}

impl UpdateGoalChecklistTool {
    pub fn new(runtime: Arc<GoalRuntimeHandle>) -> Self {
        Self { runtime }
    }

    pub fn definition_static() -> ToolDefinition {
        ToolDefinition {
            name: "update_goal_checklist".to_string(),
            description:
                "Manage the task checklist for the current goal. Use action `set` to define \
                 the full checklist (replaces existing), or `update` to change a single task's \
                 status. The checklist must be fully verified before the goal can be marked \
                 complete. Always decompose the objective into concrete, verifiable tasks."
                    .to_string(),
            kind: ToolKind::Write,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["set", "update"],
                        "description": "`set` replaces the entire checklist. `update` changes a single task's status."
                    },
                    "tasks": {
                        "type": "array",
                        "description": "Full task list (required for `set`). Each task is a string description.",
                        "items": { "type": "string" }
                    },
                    "task_id": {
                        "type": "integer",
                        "description": "The 0-based index of the task to update (required for `update`)."
                    },
                    "status": {
                        "type": "string",
                        "enum": ["pending", "in_progress", "done", "verified", "skipped"],
                        "description": "New status for the task (required for `update`). Use `verified` only after running tests/build and confirming they pass."
                    },
                    "verification": {
                        "type": "string",
                        "description": "Optional verification command that was run (e.g. \"cargo test -p navi-core\")."
                    }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
            metadata: ToolMetadata {
                tags: vec!["goal".to_string(), "session".to_string()],
                capabilities: vec!["goal.update".to_string()],
                exposure: crate::tool::ToolExposure::Direct,
                ..ToolMetadata::default()
            },
        }
    }
}

#[async_trait]
impl Tool for UpdateGoalChecklistTool {
    fn definition(&self) -> ToolDefinition {
        Self::definition_static()
    }

    async fn invoke(&self, invocation: ToolInvocation) -> anyhow::Result<ToolResult> {
        self.invoke_with_context(invocation, ToolInvocationContext::default())
            .await
    }

    async fn invoke_with_context(
        &self,
        invocation: ToolInvocation,
        context: ToolInvocationContext,
    ) -> anyhow::Result<ToolResult> {
        let args = invocation.input.clone();
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");

        let result = match action {
            "set" => {
                let tasks = args.get("tasks").and_then(|v| v.as_array()).cloned();
                let Some(task_descs) = tasks else {
                    return Ok(make_result(
                        &invocation.id,
                        false,
                        json!({"error": "tasks array is required for `set` action"}),
                    ));
                };

                let tasks: Vec<super::types::GoalTask> = task_descs
                    .iter()
                    .enumerate()
                    .filter_map(|(i, v)| {
                        v.as_str()
                            .map(|s| super::types::GoalTask::new(i, s.to_string()))
                    })
                    .collect();

                if tasks.is_empty() {
                    return Ok(make_result(
                        &invocation.id,
                        false,
                        json!({"error": "tasks array must not be empty"}),
                    ));
                }

                match self.runtime.update_checklist(tasks.clone()) {
                    Some(goal) => {
                        emit_goal_updated(&context, &goal);
                        make_result(
                        &invocation.id,
                        true,
                        json!({
                            "updated": true,
                            "task_count": goal.checklist.len(),
                            "checklist": goal.checklist.iter().map(|t| {
                                json!({"id": t.id, "description": t.description, "status": t.status.as_str()})
                            }).collect::<Vec<_>>(),
                            "message": format!("Checklist set with {} tasks. Work through each task and mark it `verified` after running tests.", goal.checklist.len())
                        }),
                    )
                    }
                    None => make_result(
                        &invocation.id,
                        false,
                        json!({"error": "No goal is currently set. Use create_goal to set one first."}),
                    ),
                }
            }
            "update" => {
                let task_id = args.get("task_id").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let status_str = args.get("status").and_then(|v| v.as_str()).unwrap_or("");
                let status = match status_str {
                    "pending" => super::types::TaskStatus::Pending,
                    "in_progress" => super::types::TaskStatus::InProgress,
                    "done" => super::types::TaskStatus::Done,
                    "verified" => super::types::TaskStatus::Verified,
                    "skipped" => super::types::TaskStatus::Skipped,
                    _ => {
                        return Ok(make_result(
                            &invocation.id,
                            false,
                            json!({"error": format!("Unknown status `{status_str}`. Valid: pending, in_progress, done, verified, skipped.")}),
                        ));
                    }
                };

                let verification = args
                    .get("verification")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let goal = match self.runtime.get_goal() {
                    Some(g) => g,
                    None => {
                        return Ok(make_result(
                            &invocation.id,
                            false,
                            json!({"error": "No goal is currently set."}),
                        ));
                    }
                };

                if task_id >= goal.checklist.len() {
                    return Ok(make_result(
                        &invocation.id,
                        false,
                        json!({"error": format!("task_id {} is out of bounds (checklist has {} tasks)", task_id, goal.checklist.len())}),
                    ));
                }

                if let Some(ref ver) = verification {
                    let _ = self.runtime.update_task_status(task_id, status);
                    // Also record verification on the goal
                    if let Some(mut g) = self.runtime.get_goal() {
                        g.set_task_verification(
                            task_id,
                            ver.clone(),
                            status == super::types::TaskStatus::Verified,
                        );
                        self.runtime.update_goal(g.clone());
                        emit_goal_updated(&context, &g);
                        make_result(
                            &invocation.id,
                            true,
                            json!({
                                "updated": true,
                                "task_id": task_id,
                                "status": status.as_str(),
                                "verification": ver,
                                "finished": g.finished_count(),
                                "total": g.checklist.len(),
                                "message": if g.is_checklist_complete() {
                                    "All checklist tasks are finished. You can now mark the goal as complete with update_goal(complete)."
                                } else {
                                    "Task updated. Continue working on remaining tasks."
                                }
                            }),
                        )
                    } else {
                        make_result(
                            &invocation.id,
                            false,
                            json!({"error": "Failed to update task verification."}),
                        )
                    }
                } else {
                    match self.runtime.update_task_status(task_id, status) {
                        Some(g) => {
                            emit_goal_updated(&context, &g);
                            make_result(
                            &invocation.id,
                            true,
                            json!({
                                "updated": true,
                                "task_id": task_id,
                                "status": status.as_str(),
                                "finished": g.finished_count(),
                                "total": g.checklist.len(),
                                "message": if g.is_checklist_complete() {
                                    "All checklist tasks are finished. You can now mark the goal as complete with update_goal(complete)."
                                } else {
                                    "Task updated. Continue working on remaining tasks."
                                }
                            }),
                        )
                        }
                        None => make_result(
                            &invocation.id,
                            false,
                            json!({"error": "Failed to update task status."}),
                        ),
                    }
                }
            }
            other => make_result(
                &invocation.id,
                false,
                json!({"error": format!("Unknown action `{other}`. Valid actions: set, update.")}),
            ),
        };
        Ok(result)
    }
}

/// Convenience: returns a vec of all goal tool definitions (without constructing tools).
pub fn goal_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        GetGoalTool::definition_static(),
        CreateGoalTool::definition_static(),
        UpdateGoalTool::definition_static(),
        UpdateGoalChecklistTool::definition_static(),
    ]
}
