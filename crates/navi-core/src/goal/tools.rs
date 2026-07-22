use crate::event::AgentEvent;
use crate::goal::runtime::GoalRuntimeHandle;
use crate::goal::types::{GoalStatus, SessionGoal};
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

fn goal_with_status(goal: &SessionGoal, status: GoalStatus) -> SessionGoal {
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

fn is_unfinished(status: GoalStatus) -> bool {
    !status.is_terminal()
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
            description: "Get the current goal for this thread, including status, budgets, token \
                 and elapsed-time usage, and remaining token budget."
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
                    "goal": {
                        "goal_id": goal.goal_id.as_str(),
                        "objective": goal.objective,
                        "short_description": goal.short_description,
                        "status": goal.status.as_str(),
                        "tokens_used": goal.tokens_used,
                        "token_budget": goal.token_budget,
                        "time_used_seconds": goal.time_used_seconds,
                        "created_at": goal.created_at,
                        "updated_at": goal.updated_at
                    },
                    "remaining_tokens": goal.remaining_budget(),
                }),
            ),
            None => make_result(
                &invocation.id,
                true,
                json!({
                    "goal": null,
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
            description: format!(
                "Create a goal only when explicitly requested by the user or system/developer \
instructions; do not infer goals from ordinary tasks.\n\
Set token_budget only when an explicit token budget is requested. Fails if an unfinished goal \
exists; use `{UPDATE_GOAL}` only for status.",
                UPDATE_GOAL = "update_goal"
            ),
            kind: ToolKind::Write,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "objective": {
                        "type": "string",
                        "description": "Required. The concrete objective to start pursuing. This starts a new active goal when no goal exists or replaces the current goal when it is complete."
                    },
                    "short_description": {
                        "type": "string",
                        "maxLength": 40,
                        "description": "Optional short UI label (max 40 characters)."
                    },
                    "token_budget": {
                        "type": "integer",
                        "description": "Positive token budget for the new goal. Omit unless explicitly requested."
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
            .map(str::trim)
            .unwrap_or("")
            .to_string();
        if objective.is_empty() {
            return Ok(make_result(
                &invocation.id,
                false,
                json!({"error": "objective is required"}),
            ));
        }

        if let Some(existing) = self.runtime.get_goal() {
            if is_unfinished(existing.status) {
                return Ok(make_result(
                    &invocation.id,
                    false,
                    json!({
                        "error": "cannot create a new goal because this thread has an unfinished goal; complete the existing goal first",
                        "existing_goal_id": existing.goal_id.as_str(),
                        "existing_status": existing.status.as_str(),
                    }),
                ));
            }
        }

        let token_budget = args.get("token_budget").and_then(|v| v.as_i64());
        if let Some(budget) = token_budget {
            if budget <= 0 {
                return Ok(make_result(
                    &invocation.id,
                    false,
                    json!({"error": "token_budget must be a positive integer when set"}),
                ));
            }
        }

        let short_description = args
            .get("short_description")
            .and_then(|v| v.as_str())
            .map(limit_short_description);

        // Always create a fresh goal id (do not mutate a terminal goal in place).
        let goal = self.runtime.create_new_goal(
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
                "goal": {
                    "goal_id": goal.goal_id.as_str(),
                    "objective": goal.objective,
                    "short_description": goal.short_description,
                    "status": goal.status.as_str(),
                    "token_budget": goal.token_budget,
                    "tokens_used": goal.tokens_used,
                },
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
            description: r#"Update the existing goal.
Use this tool only to mark the goal achieved or genuinely blocked.
Set status to `complete` only when the objective has actually been achieved and no required work remains.
Set status to `blocked` only when the same blocking condition has repeated for at least three consecutive goal turns, counting the original/user-triggered turn and any automatic continuations, and the agent cannot make meaningful progress without user input or an external-state change.
If the user resumes a goal that was previously marked `blocked`, treat the resumed run as a fresh blocked audit. If the same blocking condition then repeats for at least three consecutive resumed goal turns, set status to `blocked` again.
Once the blocked threshold is satisfied, do not keep reporting that you are still blocked while leaving the goal active; set status to `blocked`.
Do not use `blocked` merely because the work is hard, slow, uncertain, incomplete, or would benefit from clarification.
Do not mark a goal complete merely because its budget is nearly exhausted or because you are stopping work.
You cannot use this tool to pause, resume, budget-limit, or usage-limit a goal; those status changes are controlled by the user or system.
When marking a budgeted goal achieved with status `complete`, report the final token usage from the tool result to the user."#
                .to_string(),
            kind: ToolKind::Write,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["complete", "blocked"],
                        "description": "Required. Set to `complete` only when the objective is achieved and no required work remains. Set to `blocked` only after the same blocking condition has recurred for at least three consecutive goal turns and the agent is at an impasse."
                    }
                },
                "required": ["status"],
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
        let status = args.get("status").and_then(|v| v.as_str()).unwrap_or("");

        let goal = match self.runtime.get_goal() {
            Some(g) => g,
            None => {
                return Ok(make_result(
                    &invocation.id,
                    false,
                    json!({"error": "cannot update goal because this thread has no goal"}),
                ));
            }
        };

        match status {
            "complete" => {
                let updated = goal_with_status(&goal, GoalStatus::Complete);
                self.runtime.update_goal(updated.clone());
                self.runtime.set_auto_continue(false);
                emit_goal_updated(&context, &updated);
                Ok(make_result(
                    &invocation.id,
                    true,
                    json!({
                        "goal": {
                            "goal_id": updated.goal_id.as_str(),
                            "objective": updated.objective,
                            "status": "complete",
                            "tokens_used": updated.tokens_used,
                            "token_budget": updated.token_budget,
                            "time_used_seconds": updated.time_used_seconds,
                        },
                        "remaining_tokens": updated.remaining_budget(),
                        "completion_budget_report": updated.token_budget.map(|b| {
                            format!(
                                "Goal complete. Tokens used: {} / budget: {}.",
                                updated.tokens_used, b
                            )
                        }),
                        "message": "Goal marked as complete."
                    }),
                ))
            }
            "blocked" => {
                let updated = goal_with_status(&goal, GoalStatus::Blocked);
                self.runtime.update_goal(updated.clone());
                self.runtime.set_auto_continue(false);
                emit_goal_updated(&context, &updated);
                Ok(make_result(
                    &invocation.id,
                    true,
                    json!({
                        "goal": {
                            "goal_id": updated.goal_id.as_str(),
                            "objective": updated.objective,
                            "status": "blocked",
                            "tokens_used": updated.tokens_used,
                            "token_budget": updated.token_budget,
                        },
                        "message": "Goal marked as blocked."
                    }),
                ))
            }
            other => Ok(make_result(
                &invocation.id,
                false,
                json!({
                    "error": format!(
                        "update_goal can only mark the existing goal complete or blocked; got `{other}`. Pause, resume, budget-limited, and usage-limited status changes are controlled by the user or system"
                    )
                }),
            )),
        }
    }
}

// ── update_goal_checklist (host/API only; not exposed to the model) ──

/// Host-facing checklist tool kept for SDK/server compatibility. Not registered
/// on the model tool surface.
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
            description: "Host/API checklist management for a session goal (not model-facing)."
                .to_string(),
            kind: ToolKind::Write,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["set", "update"]
                    },
                    "tasks": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "task_id": { "type": "integer" },
                    "status": {
                        "type": "string",
                        "enum": ["pending", "in_progress", "done", "verified", "skipped"]
                    },
                    "verification": { "type": "string" }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
            metadata: ToolMetadata {
                tags: vec!["goal".to_string(), "session".to_string(), "host".to_string()],
                capabilities: vec!["goal.update".to_string()],
                exposure: crate::tool::ToolExposure::Internal,
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
        use super::types::{GoalTask, TaskStatus};

        let args = invocation.input.clone();
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");

        let result = match action {
            "set" => {
                let tasks = args.get("tasks").and_then(|v| v.as_array()).cloned();
                let Some(tasks) = tasks else {
                    return Ok(make_result(
                        &invocation.id,
                        false,
                        json!({"error": "`tasks` array is required for action `set`"}),
                    ));
                };
                let goal_tasks: Vec<GoalTask> = tasks
                    .iter()
                    .enumerate()
                    .filter_map(|(i, v)| {
                        v.as_str()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .map(|desc| GoalTask::new(i, desc))
                    })
                    .collect();
                match self.runtime.update_checklist(goal_tasks.clone()) {
                    Some(goal) => {
                        emit_goal_updated(&context, &goal);
                        make_result(
                            &invocation.id,
                            true,
                            json!({
                                "updated": true,
                                "task_count": goal_tasks.len(),
                                "message": "Checklist set."
                            }),
                        )
                    }
                    None => make_result(
                        &invocation.id,
                        false,
                        json!({"error": "No goal is currently set."}),
                    ),
                }
            }
            "update" => {
                let task_id = args.get("task_id").and_then(|v| v.as_u64()).map(|v| v as usize);
                let status_str = args.get("status").and_then(|v| v.as_str());
                let (Some(task_id), Some(status_str)) = (task_id, status_str) else {
                    return Ok(make_result(
                        &invocation.id,
                        false,
                        json!({"error": "`task_id` and `status` are required for action `update`"}),
                    ));
                };
                let status = match status_str {
                    "pending" => TaskStatus::Pending,
                    "in_progress" => TaskStatus::InProgress,
                    "done" => TaskStatus::Done,
                    "verified" => TaskStatus::Verified,
                    "skipped" => TaskStatus::Skipped,
                    other => {
                        return Ok(make_result(
                            &invocation.id,
                            false,
                            json!({"error": format!("Unknown status `{other}`")}),
                        ));
                    }
                };
                match self.runtime.update_task_status(task_id, status) {
                    Some(mut goal) => {
                        if let Some(verification) =
                            args.get("verification").and_then(|v| v.as_str())
                        {
                            if let Some(task) = goal.checklist.get_mut(task_id) {
                                task.verification = Some(verification.to_string());
                                task.verified = status == TaskStatus::Verified;
                            }
                            self.runtime.update_goal(goal.clone());
                        }
                        emit_goal_updated(&context, &goal);
                        make_result(
                            &invocation.id,
                            true,
                            json!({
                                "updated": true,
                                "task_id": task_id,
                                "status": status_str,
                            }),
                        )
                    }
                    None => make_result(
                        &invocation.id,
                        false,
                        json!({"error": "No goal is set or task_id is out of range."}),
                    ),
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

/// Model-facing goal tool definitions (get / create / update).
pub fn goal_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        GetGoalTool::definition_static(),
        CreateGoalTool::definition_static(),
        UpdateGoalTool::definition_static(),
    ]
}
