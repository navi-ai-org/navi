use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};

use super::helpers;
use crate::branch_race::{BranchRacePlanner, BranchRaceRequest, BranchStrategy};
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

pub(crate) struct BranchRaceTool;

#[async_trait]
impl Tool for BranchRaceTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "branch_race_start",
            "Plan branch-race hypotheses for uncertain implementation tasks. Returns strategies to execute in isolated snapshots/worktrees and score by verifier evidence.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "task": { "type": "string" },
                    "strategies": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["minimal-fix", "test-first", "refactor-safe", "rollback-revert"]
                        }
                    },
                    "verifier_commands": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "max_parallel": { "type": "integer" }
                },
                "required": ["task"],
                "additionalProperties": false
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let request = request_from_input(&invocation.input)?;
        let hypotheses = BranchRacePlanner::plan(&request);
        Ok(helpers::ok(
            invocation.id,
            json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "task": request.task,
                "max_parallel": request.max_parallel.max(1),
                "verifier_commands": request.verifier_commands,
                "hypotheses": hypotheses,
                "next_step": "execute each hypothesis in an isolated snapshot or worktree, run verifiers, review independently, then score with BranchRacePlanner::report",
            }),
        ))
    }
}

fn request_from_input(input: &Value) -> Result<BranchRaceRequest> {
    let task = helpers::required_string(input, "task")?.to_string();
    let strategies = input
        .get("strategies")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(parse_strategy)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let verifier_commands = input
        .get("verifier_commands")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let max_parallel = input
        .get("max_parallel")
        .and_then(Value::as_u64)
        .unwrap_or(3)
        .clamp(1, 8) as usize;
    Ok(BranchRaceRequest {
        task,
        strategies,
        verifier_commands,
        max_parallel,
    })
}

fn parse_strategy(value: &str) -> BranchStrategy {
    match value {
        "minimal-fix" => BranchStrategy::MinimalFix,
        "test-first" => BranchStrategy::TestFirst,
        "refactor-safe" => BranchStrategy::RefactorSafe,
        "rollback-revert" => BranchStrategy::RollbackRevert,
        other => BranchStrategy::Custom(other.to_string()),
    }
}
