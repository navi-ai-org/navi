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
                    "short_description": {
                        "type": "string",
                        "maxLength": 40,
                        "description": "Short TUI label for this goal. Summarize the goal in a few words; maximum 40 characters."
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

        let short_description = invocation
            .input
            .get("short_description")
            .and_then(|v| v.as_str())
            .map(limit_short_description);

        let token_budget = invocation
            .input
            .get("token_budget")
            .and_then(|v| v.as_i64());

        if let Some(tx) = context.event_tx {
            let _ = tx.send(AgentEvent::SetGoalRequested {
                objective: objective.clone(),
                short_description: short_description.clone(),
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

fn limit_short_description(value: &str) -> String {
    value.trim().chars().take(40).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::AgentEvent;
    use serde_json::json;

    #[tokio::test]
    async fn set_goal_emits_short_description_limited_to_40_chars() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tool = SetGoalTool;

        let result = tool
            .invoke_with_context(
                ToolInvocation {
                    id: "goal-call".to_string(),
                    tool_name: "set_goal".to_string(),
                    input: json!({
                        "objective": "Fix the TUI layout",
                        "short_description": "1234567890123456789012345678901234567890extra",
                    }),
                },
                ToolInvocationContext {
                    event_tx: Some(tx),
                    ..Default::default()
                },
            )
            .await
            .expect("set goal");

        assert!(result.ok);
        let event = rx.recv().await.expect("goal event");
        let AgentEvent::SetGoalRequested {
            short_description, ..
        } = event
        else {
            panic!("unexpected event");
        };
        assert_eq!(
            short_description.as_deref(),
            Some("1234567890123456789012345678901234567890")
        );
    }
}
