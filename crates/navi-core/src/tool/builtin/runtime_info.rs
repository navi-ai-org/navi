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
        let security = self.policy.config();
        // Effective jail status (Restricted always true, even if config flag is false).
        let restrict_paths_to_project = self.policy.paths_restricted_to_project();

        Ok(helpers::ok(
            invocation.id,
            json!({
                "project_root": project_root,
                "harness_profile": self.harness_profile,
                "permission_mode": security.permission_mode,
                "allow_tools": security.allow_tools,
                "allow_tool_regex": security.allow_tool_regex,
                "ask_tools": security.ask_tools,
                "ask_tool_regex": security.ask_tool_regex,
                "deny_tools": security.deny_tools,
                "deny_tool_regex": security.deny_tool_regex,
                "restrict_paths_to_project": restrict_paths_to_project,
                "protect_git_metadata": security.protect_git_metadata,
                "redact_secrets_in_sessions": security.redact_secrets_in_sessions,
                "blocked_commands": security.blocked_commands,
            }),
        ))
    }
}
