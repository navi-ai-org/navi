use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

use super::helpers;
use crate::event::AgentEvent;
use crate::tool::{
    Tool, ToolDefinition, ToolInvocation, ToolInvocationContext, ToolKind, ToolResult,
};

pub(crate) struct SetGoalTool;

#[async_trait]
impl Tool for SetGoalTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "set_goal",
            "Set a long-running goal. The agent will continue running turns autonomously until the goal is achieved, blocked, or the budget is exhausted. Use this for complex, multi-step tasks that require no human intervention.",
            ToolKind::Command,
            json!({
                "type": "object",
                "properties": {
                    "objective": {
                        "type": "string",
                        "description": "A clear, actionable description of the goal to achieve."
                    },
                    "token_budget": {
                        "type": "integer",
                        "description": "Optional token limit for the goal. If omitted, a default budget is applied."
                    }
                },
                "required": ["objective"],
                "additionalProperties": false
            }),
        )
    }

    async fn invoke(&self, _invocation: ToolInvocation) -> Result<ToolResult> {
        unreachable!("SetGoalTool requires context")
    }

    async fn invoke_with_context(
        &self,
        invocation: ToolInvocation,
        context: ToolInvocationContext,
    ) -> Result<ToolResult> {
        let objective = invocation
            .input
            .get("objective")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let token_budget = invocation
            .input
            .get("token_budget")
            .and_then(|v| v.as_i64());

        if let Some(tx) = context.event_tx {
            let _ = tx.send(AgentEvent::SetGoalRequested {
                objective: objective.clone(),
                token_budget,
            });
        }

        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: true,
            output: json!({
                "message": format!("Goal set: {}. The agent will now run autonomously.", objective)
            }),
        })
    }
}
