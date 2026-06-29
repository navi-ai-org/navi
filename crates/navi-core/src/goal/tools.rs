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
        let goal = self.runtime.set_objective(objective, token_budget);

        Ok(make_result(
            &invocation.id,
            true,
            json!({
                "created": true,
                "goal_id": goal.goal_id.as_str(),
                "objective": goal.objective,
                "status": goal.status.as_str(),
                "token_budget": goal.token_budget,
                "message": "Goal created successfully. The agent will auto-continue working toward this goal."
            }),
        ))
    }

    async fn invoke_with_context(
        &self,
        invocation: ToolInvocation,
        _context: ToolInvocationContext,
    ) -> anyhow::Result<ToolResult> {
        self.invoke(invocation).await
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
                self.runtime
                    .update_goal(goal_with_status(&goal, GoalStatus::Complete));
                make_result(
                    &invocation.id,
                    true,
                    json!({
                        "updated": true,
                        "status": "complete",
                        "message": "Goal marked as complete. No further auto-continuation."
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
                self.runtime
                    .update_goal(goal_with_status(&goal, GoalStatus::Paused));
                self.runtime.set_auto_continue(false);
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
                self.runtime
                    .update_goal(goal_with_status(&goal, GoalStatus::Active));
                self.runtime.set_auto_continue(true);
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

    async fn invoke_with_context(
        &self,
        invocation: ToolInvocation,
        _context: ToolInvocationContext,
    ) -> anyhow::Result<ToolResult> {
        self.invoke(invocation).await
    }
}

/// Convenience: returns a vec of all three goal tool definitions (without constructing tools).
pub fn goal_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        GetGoalTool::definition_static(),
        CreateGoalTool::definition_static(),
        UpdateGoalTool::definition_static(),
    ]
}
