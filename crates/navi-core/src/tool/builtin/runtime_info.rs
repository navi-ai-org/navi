use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

use super::helpers;
use crate::security::SecurityPolicy;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

pub(crate) struct RuntimeInfoTool {
    policy: SecurityPolicy,
    harness_profile: String,
}

impl RuntimeInfoTool {
    pub(crate) fn new(policy: SecurityPolicy, harness_profile: String) -> Self {
        Self {
            policy,
            harness_profile,
        }
    }
}

#[async_trait]
impl Tool for RuntimeInfoTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "runtime_info",
            "Show NAVI runtime state: harness profile, project root, and security config. Useful for understanding the execution environment.",
            ToolKind::Read,
            helpers::json_schema(&[], &[]),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let project_root = self.policy.project_root().display().to_string();

        Ok(helpers::ok(
            invocation.id,
            json!({
                "project_root": project_root,
                "harness_profile": self.harness_profile,
            }),
        ))
    }
}
