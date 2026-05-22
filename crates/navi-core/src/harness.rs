use crate::config::{HarnessConfig, HarnessProfile, ModelTaskSize, NaviConfig};
use crate::model::ModelRequest;
use crate::tool::{ToolInvocation, ToolResult};
use serde_json::{Value, json};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessPolicy {
    pub profile: HarnessProfile,
    pub observation_max_bytes: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentRunState {
    pub tool_iterations: usize,
    pub last_tool_signature: Option<String>,
    pub repeated_tool_calls: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolLoopDecision {
    Continue,
    RepeatedCall(String),
}

pub fn select_harness_policy(config: &NaviConfig) -> HarnessPolicy {
    let profile = match config.harness.profile {
        HarnessProfile::Auto => infer_profile(config),
        fixed => fixed,
    };
    policy_for_profile(&config.harness, profile)
}

pub fn policy_for_profile(config: &HarnessConfig, profile: HarnessProfile) -> HarnessPolicy {
    match profile {
        HarnessProfile::Auto => policy_for_profile(config, HarnessProfile::Medium),
        HarnessProfile::Small => HarnessPolicy {
            profile,
            observation_max_bytes: config.observation_bytes_small,
        },
        HarnessProfile::Medium => HarnessPolicy {
            profile,
            observation_max_bytes: config.observation_bytes_medium,
        },
    }
}

fn infer_profile(config: &NaviConfig) -> HarnessProfile {
    let selected_provider = &config.model.provider;
    let selected_model = &config.model.name;
    crate::available_model_options(config)
        .into_iter()
        .find(|model| model.provider_id == *selected_provider && model.name == *selected_model)
        .map(|model| match model.task_size {
            ModelTaskSize::Small => HarnessProfile::Small,
            ModelTaskSize::Large => HarnessProfile::Medium,
        })
        .unwrap_or(HarnessProfile::Medium)
}

pub fn build_system_prompt(config: &NaviConfig, cwd: &Path) -> String {
    build_system_prompt_with_memory(config, cwd, None)
}

pub fn build_system_prompt_with_memory(
    config: &NaviConfig,
    cwd: &Path,
    memory_injection: Option<&str>,
) -> String {
    let policy = select_harness_policy(config);
    let profile = match policy.profile {
        HarnessProfile::Auto => "medium",
        HarnessProfile::Small => "small",
        HarnessProfile::Medium => "medium",
    };
    let mut prompt = format!(
        concat!(
            "You are NAVI, an autonomous code agent running in a terminal.\n",
            "Harness profile: {profile}. Current project: {cwd}.\n",
            "\n",
            "Workflow contract:\n",
            "1. Understand the task and inspect relevant files before editing.\n",
            "2. Use tools for facts. Do not guess file contents, APIs, or command results.\n",
            "3. Keep edits narrow and explain only decisions that affect the task.\n",
            "4. After writes, verify with the smallest relevant command or explain why verification was not run.\n",
            "5. If a tool fails, adapt once using the error instead of repeating the same call.\n",
            "\n",
            "Tool rules:\n",
            "- Prefer read_file, list_files, and grep for inspection.\n",
            "- Prefer apply_patch for targeted edits; write_file is for whole-file replacement.\n",
            "- Use bash for tests, builds, and commands that cannot be expressed by other tools.\n",
            "- File paths should be project-relative when possible.\n",
            "- Writes and commands may require approval.\n",
            "\n",
            "Response rules:\n",
            "- Be concise.\n",
            "- Use markdown for readable summaries and fenced code blocks for code.\n",
            "- Do not claim success until the requested change is implemented or a blocker is clear.\n"
        ),
        profile = profile,
        cwd = cwd.display(),
    );
    if let Some(memory) = memory_injection {
        prompt.push_str("\n");
        prompt.push_str(memory);
        prompt.push_str("\n");
    }
    prompt
}

pub fn record_tool_call(
    state: &mut AgentRunState,
    _policy: HarnessPolicy,
    invocation: &ToolInvocation,
) -> ToolLoopDecision {
    let signature = tool_signature(invocation);
    if state.last_tool_signature.as_deref() == Some(signature.as_str()) {
        state.repeated_tool_calls += 1;
    } else {
        state.repeated_tool_calls = 0;
    }
    state.last_tool_signature = Some(signature);

    if state.repeated_tool_calls >= 1 {
        return ToolLoopDecision::RepeatedCall(format!(
            "repeated identical tool call `{}`; use the previous observation or change strategy",
            invocation.tool_name
        ));
    }

    state.tool_iterations += 1;
    ToolLoopDecision::Continue
}

pub fn compact_tool_observation(
    invocation: &ToolInvocation,
    result: &ToolResult,
    policy: HarnessPolicy,
) -> String {
    let output = truncate_string(
        serde_json::to_string_pretty(&result.output).unwrap_or_else(|_| result.output.to_string()),
        policy.observation_max_bytes,
    );
    let status = if result.ok { "success" } else { "error" };
    format!(
        "tool: {}\ncall_id: {}\nstatus: {}\nobservation:\n{}",
        invocation.tool_name, invocation.id, status, output
    )
}

pub fn tool_error_result(invocation: &ToolInvocation, reason: impl Into<String>) -> ToolResult {
    ToolResult {
        invocation_id: invocation.id.clone(),
        ok: false,
        output: json!({ "error": reason.into() }),
    }
}

pub fn trace_request_summary(request: &ModelRequest, policy: HarnessPolicy) -> Value {
    json!({
        "model": request.model,
        "profile": format!("{:?}", policy.profile).to_lowercase(),
        "messages": request.messages.len(),
        "tools": request.tools.len(),
        "observation_max_bytes": policy.observation_max_bytes,
    })
}

fn tool_signature(invocation: &ToolInvocation) -> String {
    format!("{}:{}", invocation.tool_name, invocation.input)
}

fn truncate_string(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    value.truncate(max_bytes);
    while !value.is_char_boundary(value.len()) {
        value.pop();
    }
    value.push_str("\n<truncated>");
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HarnessConfig, HarnessProfile, NaviConfig};

    #[test]
    fn auto_profile_infers_small_from_selected_model() {
        let mut config = NaviConfig::default();
        config.model.provider = "openai".to_string();
        config.model.name = "gpt-5-mini".to_string();

        let policy = select_harness_policy(&config);

        assert_eq!(policy.profile, HarnessProfile::Small);
    }

    #[test]
    fn profile_policy_uses_configured_limits() {
        let config = HarnessConfig {
            observation_bytes_small: 10,
            observation_bytes_medium: 20,
            ..HarnessConfig::default()
        };

        let small = policy_for_profile(&config, HarnessProfile::Small);
        let medium = policy_for_profile(&config, HarnessProfile::Medium);

        assert_eq!(small.observation_max_bytes, 10);
        assert_eq!(medium.observation_max_bytes, 20);
    }

    #[test]
    fn repeated_tool_call_is_flagged() {
        let policy = HarnessPolicy {
            profile: HarnessProfile::Small,
            observation_max_bytes: 100,
        };
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({ "path": "Cargo.toml" }),
        };
        let mut state = AgentRunState::default();

        assert_eq!(
            record_tool_call(&mut state, policy, &invocation),
            ToolLoopDecision::Continue
        );
        assert!(matches!(
            record_tool_call(&mut state, policy, &invocation),
            ToolLoopDecision::RepeatedCall(_)
        ));
    }

    #[test]
    fn compact_observation_is_bounded() {
        let policy = HarnessPolicy {
            profile: HarnessProfile::Small,
            observation_max_bytes: 16,
        };
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({ "path": "Cargo.toml" }),
        };
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: json!({ "content": "abcdefghijklmnopqrstuvwxyz" }),
        };

        let observation = compact_tool_observation(&invocation, &result, policy);

        assert!(observation.contains("<truncated>"));
        assert!(observation.contains("status: success"));
    }
}
