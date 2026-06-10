use crate::config::{HarnessConfig, HarnessProfile, ModelTaskSize, NaviConfig};
use crate::model::ModelRequest;
use crate::tool::{ToolDefinition, ToolInvocation, ToolResult, example_from_schema};
use serde_json::{Value, json};
use std::path::Path;

/// Runtime policy derived from the harness profile, controlling tool-loop and
/// observation limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessPolicy {
    /// The selected harness profile.
    pub profile: HarnessProfile,
    /// Maximum bytes of tool output captured per observation.
    pub observation_max_bytes: usize,
}

/// Mutable state tracked across tool-loop iterations for detecting repetition
/// and enforcing iteration limits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentRunState {
    /// Total tool-loop iterations so far.
    pub tool_iterations: usize,
    /// Serialized signature of the last tool invocation, for repetition detection.
    pub last_tool_signature: Option<String>,
    /// Consecutive count of the same repeated tool call.
    pub repeated_tool_calls: usize,
}

/// Decision returned by the harness after evaluating a tool iteration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolLoopDecision {
    /// Proceed to the next iteration.
    Continue,
    /// The same tool call was repeated; the loop should break with a warning message.
    RepeatedCall(String),
}

/// Selects a [`HarnessPolicy`] from the config, inferring `Auto` profile from
/// the selected model's task size.
pub fn select_harness_policy(config: &NaviConfig) -> HarnessPolicy {
    let profile = match config.harness.profile {
        HarnessProfile::Auto => infer_profile(config),
        fixed => fixed,
    };
    policy_for_profile(&config.harness, profile)
}

/// Builds a [`HarnessPolicy`] for an explicit profile (resolving `Auto` to `Medium`).
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

/// Builds the system prompt for the agent from the given config and working directory.
pub fn build_system_prompt(config: &NaviConfig, cwd: &Path) -> String {
    build_system_prompt_with_memory(config, cwd, None)
}

/// Builds the system prompt with an optional memory injection block appended.
pub fn build_system_prompt_with_memory(
    config: &NaviConfig,
    cwd: &Path,
    memory_injection: Option<&str>,
) -> String {
    build_system_prompt_inner(config, cwd, memory_injection, None)
}

/// Builds the system prompt with memory injection and an optional tool manifest
/// appended for provider compatibility fallback.
pub fn build_system_prompt_with_tools(
    config: &NaviConfig,
    cwd: &Path,
    memory_injection: Option<&str>,
    tools: &[ToolDefinition],
    include_tool_manifest: bool,
) -> String {
    let manifest = if include_tool_manifest && !tools.is_empty() {
        Some(tool_prompt_manifest(tools))
    } else {
        None
    };
    build_system_prompt_with_manifest_text(config, cwd, memory_injection, manifest.as_deref())
}

/// Builds the system prompt with a caller-provided tool manifest. This lets the
/// turn layer cache manifest rendering independently of the dynamic prompt body.
pub fn build_system_prompt_with_manifest_text(
    config: &NaviConfig,
    cwd: &Path,
    memory_injection: Option<&str>,
    tool_manifest: Option<&str>,
) -> String {
    build_system_prompt_inner(config, cwd, memory_injection, tool_manifest)
}

fn build_system_prompt_inner(
    config: &NaviConfig,
    cwd: &Path,
    memory_injection: Option<&str>,
    tool_manifest: Option<&str>,
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
            "- Prefer top_files for first-pass exploration of unfamiliar code areas before issuing many read_file calls.\n",
            "- Prefer read_file, fs_browser, and grep for focused follow-up inspection.\n",
            "- Prefer apply_patch for targeted edits; write_file is for whole-file replacement.\n",
            "- Prefer test_runner over bash for running tests — structured output is faster to process.\n",
            "- Prefer build_runner over bash for compilation — cached builds skip redundant work.\n",
            "- Prefer git_ops over bash for git operations — structured status, diff, log, and branch output.\n",
            "- Prefer package_manager over bash for dependency management — structured install, add, remove, update.\n",
            "- Use bash for genuinely ad-hoc commands that don't fit specialized tools.\n",
            "- For long-running commands, call bash with background=true, wait_ms, and timeout_ms; poll or cancel the returned task_id instead of waiting indefinitely.\n",
            "- File paths should be project-relative when possible.\n",
            "- Writes and commands may require approval.\n",
            "- Use native tool calling when available; do not write tool calls in markdown, XML, or prose.\n",
            "- Use runtime_info to inspect harness profile and project environment.\n",
            "\n",
            "Response rules:\n",
            "- Be concise.\n",
            "- Use markdown for readable summaries and fenced code blocks for code.\n",
            "- Do not claim success until the requested change is implemented or a blocker is clear.\n",
            "\n",
            "Long-horizon task protocol:\n",
            "- For multi-step tasks (refactors, migrations, implementations spanning 3+ files),\n",
            "  create a plan FIRST using the `plan` tool with clear, ordered steps.\n",
            "- Before each step, consult the active plan to understand what to do next.\n",
            "- After completing each step, mark it done with `plan(action='complete_step')`.\n",
            "- If a step fails or changes scope, update the plan accordingly.\n",
            "- When all steps are done, verify the result: build, test, and review the plan.\n",
            "\n",
            "Observation budget:\n",
            "- Tool outputs are truncated to save context. If you need more data from a truncated\n",
            "  result, request it explicitly: use start_line/end_line for read_file, or increase\n",
            "  max_results for grep/fs_browser.\n",
            "- Avoid dumping large outputs into context. Prefer targeted queries (specific grep\n",
            "  patterns, narrow file ranges) over broad sweeps.\n",
            "- For large file explorations, use top_files first, then read_file with line ranges.\n"
        ),
        profile = profile,
        cwd = cwd.display(),
    );
    if let Some(memory) = memory_injection {
        prompt.push('\n');
        prompt.push_str(memory);
        prompt.push('\n');
    }
    if let Some(manifest) = tool_manifest {
        prompt.push_str(
            "\nAvailable tools (compatibility manifest; still use native tool calling):\n",
        );
        prompt.push_str(manifest);
    }
    prompt
}

/// Renders a text manifest of available tools for inclusion in the system prompt.
pub fn tool_prompt_manifest(tools: &[ToolDefinition]) -> String {
    let mut tools = tools.to_vec();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    tools
        .iter()
        .map(|tool| {
            let required = tool
                .input_schema
                .get("required")
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "none".to_string());
            let example = example_from_schema(&tool.input_schema);
            format!(
                "- {}: {} Required: {}. Example input: {}",
                tool.name, tool.description, required, example
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// Records a completed tool invocation in the run state, updating iteration
/// count and repetition tracking.
///
/// Returns `ToolLoopDecision::RepeatedCall` when the same tool has been
/// called consecutively with identical arguments 20+ times in a row,
/// indicating the model is likely hallucinating / stuck in a loop.
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
    state.tool_iterations += 1;

    if state.repeated_tool_calls >= 20 {
        return ToolLoopDecision::RepeatedCall(format!(
            "Repeated identical tool call `{}` {} times in a row — the model appears stuck",
            invocation.tool_name,
            state.repeated_tool_calls + 1,
        ));
    }

    ToolLoopDecision::Continue
}

/// Truncates tool output to the policy's observation byte limit with a
/// `[truncated]` marker if exceeded.
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

/// Creates a [`ToolResult`] representing an error, formatted with the
/// invocation name and a reason message.
pub fn tool_error_result(invocation: &ToolInvocation, reason: impl Into<String>) -> ToolResult {
    ToolResult {
        invocation_id: invocation.id.clone(),
        ok: false,
        output: json!({ "error": reason.into() }),
    }
}

/// Builds a JSON summary of a model request for diagnostic logging. Excludes
/// full message content (logged separately at debug level).
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
    use crate::config::HarnessConfig;
    use crate::{HarnessProfile, NaviConfig};

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
    fn repeated_tool_call_is_flagged_at_20() {
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

        // First 20 calls should be Continue (repeated goes 0..19).
        for i in 0..20 {
            let mut inv = invocation.clone();
            inv.id = format!("call-{i}");
            assert_eq!(
                record_tool_call(&mut state, policy, &inv),
                ToolLoopDecision::Continue,
                "call {i} should continue"
            );
        }
        // 21st consecutive identical call (repeated=20) triggers RepeatedCall.
        assert!(matches!(
            record_tool_call(&mut state, policy, &invocation),
            ToolLoopDecision::RepeatedCall(_)
        ));
    }

    #[test]
    fn repeated_tool_call_resets_on_different_input() {
        let policy = HarnessPolicy {
            profile: HarnessProfile::Small,
            observation_max_bytes: 100,
        };
        let invocation_a = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({ "path": "Cargo.toml" }),
        };
        let invocation_b = ToolInvocation {
            id: "call-2".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({ "path": "src/main.rs" }),
        };
        let mut state = AgentRunState::default();

        // Call A 20 times, then B — counter resets.
        for i in 0..20 {
            let mut inv = invocation_a.clone();
            inv.id = format!("a-{i}");
            record_tool_call(&mut state, policy, &inv);
        }
        assert_eq!(
            record_tool_call(&mut state, policy, &invocation_b),
            ToolLoopDecision::Continue,
        );
        // Back to A — counter started over, so still Continue.
        assert_eq!(
            record_tool_call(&mut state, policy, &invocation_a),
            ToolLoopDecision::Continue,
        );
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

    #[test]
    fn tool_prompt_manifest_lists_required_fields_and_examples() {
        let tool = ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file.".to_string(),
            kind: crate::tool::ToolKind::Read,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "start_line": { "type": "integer" }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        };

        let manifest = tool_prompt_manifest(&[tool]);

        assert!(manifest.contains("read_file"));
        assert!(manifest.contains("Required: path"));
        assert!(manifest.contains(r#"{"path":"example"}"#));
    }
}
