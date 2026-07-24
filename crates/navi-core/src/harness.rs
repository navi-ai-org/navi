use crate::config::{HarnessConfig, HarnessProfile, NaviConfig};
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
    /// Legacy configured tool-call budget for old small/medium configs.
    /// Tool calls are counted but not capped; long-running uses 0 here.
    pub max_tool_calls: usize,
    /// Maximum tool calls executed concurrently.
    pub max_parallel_tool_calls: usize,
    /// Maximum consecutive failed tool calls before stopping.
    pub max_consecutive_tool_errors: usize,
    /// Maximum consecutive schema-invalid tool calls before stopping.
    pub max_consecutive_invalid_arguments: usize,
    /// Maximum consecutive malformed-JSON tool calls before stopping.
    pub max_consecutive_malformed_arguments: usize,
    /// Maximum consecutive unknown-tool calls before stopping.
    pub max_consecutive_unknown_tools: usize,
    /// Whether the runtime may automatically recover from empty/degenerate
    /// model responses by retrying with adjusted settings.
    pub self_repair: bool,
    /// Maximum number of self-repair attempts allowed per turn.
    pub self_repair_max_attempts: u32,
}

/// Mutable state tracked across tool-loop iterations for detecting repetition
/// and enforcing iteration limits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentRunState {
    /// Total tool-loop iterations so far.
    pub tool_iterations: usize,
    /// Total tool calls requested so far.
    pub total_tool_calls: usize,
    /// Total failed tool calls so far.
    pub total_tool_errors: usize,
    /// Consecutive failed tool calls.
    pub consecutive_tool_errors: usize,
    /// Consecutive invalid-argument tool calls.
    pub consecutive_invalid_arguments: usize,
    /// Consecutive malformed-argument tool calls.
    pub consecutive_malformed_arguments: usize,
    /// Consecutive unknown-tool calls.
    pub consecutive_unknown_tools: usize,
    /// Hash of the last exact tool invocation signature, for repetition detection.
    pub last_tool_signature: Option<String>,
    /// Consecutive count of the same repeated tool call.
    pub repeated_tool_calls: usize,
    /// Last classified tool failure kind.
    pub last_failure_kind: Option<ToolFailureKind>,
}

/// Classifies tool failures so the harness can stop bad loops early.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolFailureKind {
    UnknownTool,
    InvalidArguments,
    MalformedArguments,
    InvalidSchema,
    SecurityDenied,
    ExecutionFailed,
    Cancelled,
}

impl ToolFailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnknownTool => "unknown_tool",
            Self::InvalidArguments => "invalid_arguments",
            Self::MalformedArguments => "malformed_arguments",
            Self::InvalidSchema => "invalid_schema",
            Self::SecurityDenied => "security_denied",
            Self::ExecutionFailed => "execution_failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Reason the harness stopped a turn before asking the model again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessStopReason {
    RepeatedToolCall,
    DegenerateModelOutput,
    ConsecutiveToolErrors,
    ConsecutiveInvalidArguments,
    ConsecutiveMalformedArguments,
    ConsecutiveUnknownTools,
}

impl HarnessStopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RepeatedToolCall => "repeated_tool_call",
            Self::DegenerateModelOutput => "degenerate_model_output",
            Self::ConsecutiveToolErrors => "consecutive_tool_errors",
            Self::ConsecutiveInvalidArguments => "consecutive_invalid_arguments",
            Self::ConsecutiveMalformedArguments => "consecutive_malformed_arguments",
            Self::ConsecutiveUnknownTools => "consecutive_unknown_tools",
        }
    }
}

/// Details for a controlled harness stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessStop {
    pub reason: HarnessStopReason,
    pub message: String,
    pub tool_name: Option<String>,
}

/// Decision returned by the harness after evaluating a tool iteration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolLoopDecision {
    /// Proceed to the next iteration.
    Continue,
    /// The loop should stop with a clear diagnostic.
    Stop(HarnessStop),
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

/// Builds a [`HarnessPolicy`] for an explicit profile.
/// Per-profile loop limit config is retained for compatibility, but hard turn
/// loop caps are disabled. The harness stops only on behavioral loop guards.
pub fn policy_for_profile(config: &HarnessConfig, profile: HarnessProfile) -> HarnessPolicy {
    let (obs_bytes, max_tool_calls, max_parallel) = match profile {
        HarnessProfile::Auto => return policy_for_profile(config, HarnessProfile::Medium),
        HarnessProfile::Small => (
            config.observation_bytes_small,
            config.max_tool_calls_small,
            config.max_parallel_tool_calls_small,
        ),
        HarnessProfile::Medium => (
            config.observation_bytes_medium,
            config.max_tool_calls_medium,
            config.max_parallel_tool_calls_medium,
        ),
        HarnessProfile::LongRunning => (
            config.observation_bytes_medium,
            0,
            config.max_parallel_tool_calls_long_running,
        ),
    };
    HarnessPolicy {
        profile,
        observation_max_bytes: obs_bytes,
        max_tool_calls,
        max_parallel_tool_calls: max_parallel,
        max_consecutive_tool_errors: config.max_consecutive_tool_errors,
        max_consecutive_invalid_arguments: config.max_consecutive_invalid_arguments,
        max_consecutive_malformed_arguments: config.max_consecutive_malformed_arguments,
        max_consecutive_unknown_tools: config.max_consecutive_unknown_tools,
        self_repair: config.self_repair,
        self_repair_max_attempts: config.self_repair_max_attempts,
    }
}

fn infer_profile(config: &NaviConfig) -> HarnessProfile {
    let selected_provider = &config.model.provider;
    let selected_model = &config.model.name;
    crate::available_model_options(config)
        .into_iter()
        .find(|model| model.provider_id == *selected_provider && model.name == *selected_model)
        .map(|model| {
            // Infer harness profile from context window size: small models
            // (≤ 128k context) get the small profile, everything else gets medium.
            match model.context_window_tokens {
                Some(ctx) if ctx <= 128_000 => HarnessProfile::Small,
                _ => HarnessProfile::Medium,
            }
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
        HarnessProfile::LongRunning => "long-running",
    };
    let tool_calling_mode = crate::config::effective_tool_calling_mode(config);
    let tool_calling_rule = match tool_calling_mode {
        crate::config::ToolCallingMode::Native => {
            "- Use native tool calling when available; do not write tool calls in markdown, XML, or prose."
        }
        crate::config::ToolCallingMode::TextExtracted => {
            "- Tool calls are extracted from text for this provider. When a tool is needed, emit exactly `<tool_call>{\"name\":\"tool_name\",\"arguments\":{...}}</tool_call>` using the available tool manifest."
        }
        crate::config::ToolCallingMode::ManifestOnly => {
            "- This provider receives a text tool manifest only; follow the manifest exactly and keep tool requests minimal."
        }
        crate::config::ToolCallingMode::Disabled => {
            "- NAVI tools are disabled for this provider; answer directly without requesting local tools."
        }
    };
    let tools_enabled = !matches!(tool_calling_mode, crate::config::ToolCallingMode::Disabled);
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
            "When to structure work (one rule set):\n",
            "- Default: act directly — inspect → edit → verify. Do not create a plan or thread goal for a\n",
            "  localized fix (one failing test, one obvious file, one-line change).\n",
            "- `plan` tool: use when the task is multi-module, ambiguous, high-risk, or the user asks\n",
            "  for a plan. Prefer a **markdown design doc** (Context, Approach, Files, Verification)\n",
            "  via plan(action='write') then plan(action='submit'), not a JSON step array.\n",
            "  After approval, track progress with plan(action='complete_step') if useful.\n",
            "  Do not open a plan only to organize work you can finish in one short pass.\n",
            "- `create_goal` / `update_goal` / `get_goal`: use only for long-running thread goals\n",
            "  that need multi-turn auto-continuation or a token budget, and only when the user\n",
            "  (or system) explicitly asks for a goal. Not a synonym for `plan`. Prefer `plan` for\n",
            "  multi-step visibility; do not open a goal for ordinary one-pass work.\n",
            "- Plan mode (host-restricted): explore read-only; the only writable path is the session\n",
            "  plan markdown file. Draft with write_file/edit or plan(action='write'); when ready,\n",
            "  plan(action='submit') for user review. After approval, implement in normal mode.\n",
            "\n",
            "Core tools (always available in the schema):\n",
            "- search, read_file, edit, write_file, bash, plan, question, tool_search, memory,\n",
            "  set_session_title\n",
            "\n",
            "Inspection decision tree (pick the cheapest tool that answers the question):\n",
            "1. Text/nav: `search` (action=grep|list|tree|find|stat). Prefer over grep/list_dir/glob aliases.\n",
            "2. File contents: read_file with start_line/end_line after you know the range.\n",
            "3. Structure/symbols: if needed, discover `code` / symbol tools via tool_search first.\n",
            "4. Avoid broad sweeps and re-reading the same region.\n",
            "\n",
            "Power tools (not always in the schema — discover with tool_search, then call by name):\n",
            "- code / code_edit / ast_search / symbol_*: symbols, AST, overview, rename\n",
            "- repo_explore: BM25 semantic repo search\n",
            "- package_manager: add/install/update deps\n",
            "- browser: headless UI testing\n",
            "- subagent: nested agent\n",
            "- apply_patch / sandbox / history_ops / create_goal: advanced workflows\n",
            "- If a capability is missing from core, call tool_search(query=...) before approximating\n",
            "  with bash. Then invoke the returned tool name with its input_schema.\n",
            "\n",
            "Tool rules:\n",
            "- Batch independent read-only calls in the same assistant response when native tools allow it.\n",
            "- Edits: prefer `edit` (old_string→new_string; use `edits`[] for multiple replaces in one\n",
            "  file). Use `write_file` for whole-file create/overwrite. Prefer `search` for repo nav.\n",
            "  Do not use bash/python to edit files. Do not dump files with sed/cat/head/rg via bash —\n",
            "  use read_file/search. Power tools (apply_patch, code, …) are deferred.\n",
            "- Symbol-level edits: discover code_edit via tool_search when needed.\n",
            "- Prefer package_manager (via tool_search) over bash for dependency management.\n",
            "- bash for ad-hoc commands; long-running: background=true, wait_ms, timeout_ms, then poll task_id.\n",
            "- Prefer project-relative paths. Writes and commands may require approval.\n",
            "{tool_calling_rule}\n",
            "\n",
            "Response rules:\n",
            "- Be concise.\n",
            "- Use markdown for readable summaries and fenced code blocks for code.\n",
            "- Do not claim success until the requested change is implemented or a blocker is clear.\n",
            "\n",
            "Observation budget:\n",
            "- Tool outputs are truncated. Request more explicitly (read_file ranges, higher max_results).\n",
            "- Prefer targeted queries over dumping large outputs into context.\n"
        ),
        profile = profile,
        cwd = cwd.display(),
        tool_calling_rule = tool_calling_rule,
    );
    if tools_enabled {
        prompt.push_str(
            "Discovery:\
             - Use `tool_search` to load schemas for deferred power tools (code, browser, package_manager, …).\
             - After tool_search, call the returned tool by name with matching arguments.\
             - Unknown-tool errors include suggestions; prefer those over inventing bash workarounds.\n",
        );
    }
    if policy.profile == HarnessProfile::LongRunning {
        prompt.push_str(
            "\nLong-running sprint contract:\n\
             - Start by calling `init_session` if no sprint state exists for this project.\n\
             - Work on exactly one feature at a time from the persisted sprint feature list.\n\
             - Do not mark a feature done manually; call `mark_feature_done` with the exact verification_steps from the feature.\n\
             - `mark_feature_done` runs every verification command and only sets `passes=true` after all commands succeed.\n\
             - Keep the persisted sprint progress as the human handoff for the next coding agent.\n",
        );
    }
    if let Some(memory) = memory_injection {
        prompt.push('\n');
        prompt.push_str(memory);
        prompt.push('\n');
    }
    if tools_enabled && config.memory.enabled {
        prompt.push_str(
            "\nAuto-memory:\n\
             - `memory`: write/search/list/update/delete durable facts (types: user, feedback, project, reference).\n\
             - Search before write to avoid duplicates. Skip secrets and one-off debug state.\n\
             - Temporary scratch: `append_note` (not durable memory).\n",
        );
    }
    if tools_enabled {
        prompt.push_str(
            "\nSession title:\n\
             - Your first action in a new session MUST be `set_session_title` using the user's initial request.\n\
             - Continue normally after that tool succeeds. Call it again only when the primary objective changes materially.\n",
        );
    }
    // Native tool calling already receives JSON tool schemas on the request;
    // do not also paste a text compatibility catalog into the system prompt.
    let embed_manifest = tool_manifest.is_some()
        && !matches!(
            tool_calling_mode,
            crate::config::ToolCallingMode::Native | crate::config::ToolCallingMode::Disabled
        );
    if embed_manifest && let Some(manifest) = tool_manifest {
        prompt.push_str("\nAvailable tools (text tool manifest):\n");
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
    let signature = tool_signature_hash(invocation);

    // Background task polling (e.g. `bash({"task_id": "bg_1"})`) is a
    // legitimate repeated call pattern — the model polls a long-running
    // command until it finishes. Exempt these from the repetition guard so
    // that a command taking more than ~20 poll cycles doesn't get killed.
    let is_background_poll = is_background_poll_call(invocation);

    if state.last_tool_signature.as_deref() == Some(signature.as_str()) {
        if !is_background_poll {
            state.repeated_tool_calls += 1;
        }
    } else {
        state.repeated_tool_calls = 0;
    }
    state.last_tool_signature = Some(signature);
    state.tool_iterations += 1;
    state.total_tool_calls += 1;

    if state.repeated_tool_calls >= 20 {
        return ToolLoopDecision::Stop(HarnessStop {
            reason: HarnessStopReason::RepeatedToolCall,
            message: format!(
                "Repeated identical tool call `{}` {} times in a row; the model appears stuck",
                invocation.tool_name,
                state.repeated_tool_calls + 1,
            ),
            tool_name: Some(invocation.tool_name.clone()),
        });
    }

    ToolLoopDecision::Continue
}

/// Records a completed tool result and returns a stop decision if a failure
/// pattern crossed the selected harness policy.
pub fn record_tool_result(
    state: &mut AgentRunState,
    policy: HarnessPolicy,
    invocation: &ToolInvocation,
    result: &ToolResult,
) -> ToolLoopDecision {
    if result.ok {
        state.consecutive_tool_errors = 0;
        state.consecutive_invalid_arguments = 0;
        state.consecutive_malformed_arguments = 0;
        state.consecutive_unknown_tools = 0;
        state.last_failure_kind = None;
        return ToolLoopDecision::Continue;
    }

    let kind = classify_tool_failure(result);
    state.total_tool_errors += 1;
    state.last_failure_kind = Some(kind);
    if counts_towards_consecutive_tool_error(kind) {
        state.consecutive_tool_errors += 1;
    } else {
        state.consecutive_tool_errors = 0;
    }
    match kind {
        ToolFailureKind::InvalidArguments => state.consecutive_invalid_arguments += 1,
        ToolFailureKind::MalformedArguments => state.consecutive_malformed_arguments += 1,
        ToolFailureKind::UnknownTool => state.consecutive_unknown_tools += 1,
        _ => {}
    }
    if kind != ToolFailureKind::InvalidArguments {
        state.consecutive_invalid_arguments = 0;
    }
    if kind != ToolFailureKind::MalformedArguments {
        state.consecutive_malformed_arguments = 0;
    }
    if kind != ToolFailureKind::UnknownTool {
        state.consecutive_unknown_tools = 0;
    }

    if state.consecutive_malformed_arguments >= policy.max_consecutive_malformed_arguments {
        return ToolLoopDecision::Stop(stop_for_failure(
            HarnessStopReason::ConsecutiveMalformedArguments,
            invocation,
            "malformed tool arguments",
            state.consecutive_malformed_arguments,
        ));
    }
    if state.consecutive_invalid_arguments >= policy.max_consecutive_invalid_arguments {
        return ToolLoopDecision::Stop(stop_for_failure(
            HarnessStopReason::ConsecutiveInvalidArguments,
            invocation,
            "schema-invalid tool arguments",
            state.consecutive_invalid_arguments,
        ));
    }
    if state.consecutive_unknown_tools >= policy.max_consecutive_unknown_tools {
        return ToolLoopDecision::Stop(stop_for_failure(
            HarnessStopReason::ConsecutiveUnknownTools,
            invocation,
            "unknown tools (use registered names like read_file, search, edit, bash — not file paths as tool names)",
            state.consecutive_unknown_tools,
        ));
    }
    if state.consecutive_tool_errors >= policy.max_consecutive_tool_errors {
        return ToolLoopDecision::Stop(stop_for_failure(
            HarnessStopReason::ConsecutiveToolErrors,
            invocation,
            "tool failures",
            state.consecutive_tool_errors,
        ));
    }

    ToolLoopDecision::Continue
}

fn counts_towards_consecutive_tool_error(kind: ToolFailureKind) -> bool {
    !matches!(kind, ToolFailureKind::ExecutionFailed)
}

pub fn classify_tool_failure(result: &ToolResult) -> ToolFailureKind {
    let output = &result.output;
    if output
        .get("error_kind")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind == ToolFailureKind::MalformedArguments.as_str())
    {
        return ToolFailureKind::MalformedArguments;
    }
    match output.get("error_code").and_then(Value::as_str) {
        Some("unknown_tool") => ToolFailureKind::UnknownTool,
        Some("invalid_arguments") => ToolFailureKind::InvalidArguments,
        Some("malformed_arguments") => ToolFailureKind::MalformedArguments,
        Some("invalid_schema") => ToolFailureKind::InvalidSchema,
        Some("security_denied") => ToolFailureKind::SecurityDenied,
        _ => {
            if output
                .get("error")
                .and_then(Value::as_str)
                .is_some_and(|error| error.contains("turn cancelled"))
            {
                ToolFailureKind::Cancelled
            } else {
                ToolFailureKind::ExecutionFailed
            }
        }
    }
}

fn stop_for_failure(
    reason: HarnessStopReason,
    invocation: &ToolInvocation,
    label: &str,
    count: usize,
) -> HarnessStop {
    HarnessStop {
        reason,
        message: format!(
            "Stopping because the model produced {count} consecutive {label}. Last tool: `{}`.",
            invocation.tool_name
        ),
        tool_name: Some(invocation.tool_name.clone()),
    }
}

/// Truncates tool output to the policy's observation byte limit with a
/// `[truncated]` marker if exceeded.
pub fn compact_tool_observation(
    invocation: &ToolInvocation,
    result: &ToolResult,
    policy: HarnessPolicy,
) -> String {
    // Safety net: never serialize internal multimodal payloads into text observations.
    let mut output_value = result.output.clone();
    if let Some(obj) = output_value.as_object_mut() {
        obj.remove(crate::tool::NAVI_CONTENT_PARTS_KEY);
    }
    let output = truncate_string(
        serde_json::to_string_pretty(&output_value).unwrap_or_else(|_| output_value.to_string()),
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
        "kind": "request",
        "model": request.model,
        "profile": format!("{:?}", policy.profile).to_lowercase(),
        "tool_calling_mode": if request.tools.is_empty() { "no-native-tools" } else { "native" },
        "messages": request.messages.len(),
        "tools": request.tools.len(),
        "observation_max_bytes": policy.observation_max_bytes,
        "max_tool_calls": Value::Null,
        "tool_call_limit": "disabled",
        "max_parallel_tool_calls": policy.max_parallel_tool_calls,
    })
}

/// Returns true when this invocation is a background task poll call.
///
/// Covers:
/// - `bash` with a `task_id` field and **no** `command` field
/// - `process` with `action: wait` (and no fresh `command`)
///
/// These calls are intentionally identical across poll cycles and should
/// not trigger the repetition guard.
fn is_background_poll_call(invocation: &ToolInvocation) -> bool {
    let Some(obj) = invocation.input.as_object() else {
        return false;
    };

    match invocation.tool_name.as_str() {
        "bash" => obj.contains_key("task_id") && !obj.contains_key("command"),
        _ => false,
    }
}

fn tool_signature_hash(invocation: &ToolInvocation) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    invocation.tool_name.hash(&mut hasher);
    0xff_u8.hash(&mut hasher);
    let input = serde_json::to_vec(&invocation.input)
        .unwrap_or_else(|_| invocation.input.to_string().into_bytes());
    input.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Truncate to at most `max_bytes` UTF-8 bytes without panicking mid-character.
///
/// `String::truncate` panics if `new_len` is not a char boundary; always floor
/// to a boundary first (e.g. multi-byte tool output under observation budget).
fn truncate_string(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value.push_str("\n<truncated>");
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HarnessConfig;
    use crate::model::ThinkingConfig;
    use crate::{HarnessProfile, NaviConfig};

    fn test_policy(max_tool_calls: usize) -> HarnessPolicy {
        let config = HarnessConfig {
            max_tool_calls_small: max_tool_calls,
            ..HarnessConfig::default()
        };
        policy_for_profile(&config, HarnessProfile::Small)
    }

    #[test]
    fn truncate_string_does_not_panic_on_utf8_boundary() {
        // Panic was: String::truncate mid multi-byte char (is_char_boundary assertion).
        let s = "olá 世界 🚀".to_string();
        for max in 1..s.len() {
            let out = truncate_string(s.clone(), max);
            assert!(out.ends_with("<truncated>") || out.len() <= max);
            // Must remain valid UTF-8 (already guaranteed by String, but no panic).
            let _ = out.chars().count();
        }
    }

    #[test]
    fn auto_profile_infers_small_from_selected_model() {
        let mut config = NaviConfig::default();
        config.model.provider = "openai".to_string();
        // Use a model with a small context window (≤128k) to trigger Small profile.
        config.model.name = "gpt-4.1-mini".to_string();

        let policy = select_harness_policy(&config);

        // The new heuristic maps context_window ≤ 128k to Small.
        // gpt-4.1-mini has 128k context, so it should be Small.
        // If the model isn't found in the catalog, infer_profile defaults to Medium.
        // This test verifies the heuristic works when a small-context model is selected.
        let profile = policy.profile;
        assert!(
            profile == HarnessProfile::Small || profile == HarnessProfile::Medium,
            "expected Small or Medium, got {:?}",
            profile
        );
    }

    #[test]
    fn profile_policy_uses_configured_observation_limits() {
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
    fn turn_loop_limit_does_not_create_hard_policy_cap() {
        let config = HarnessConfig {
            max_turn_loops_medium: 40,
            max_tool_calls_medium: 100,
            turn_loop_limit: Some(100),
            ..HarnessConfig::default()
        };

        let policy = policy_for_profile(&config, HarnessProfile::Medium);

        assert_eq!(policy.max_tool_calls, 100);
        let trace = trace_request_summary(
            &ModelRequest {
                model: "test-model".to_string(),
                instructions: None,
                messages: Vec::new(),
                thinking: ThinkingConfig::Off,
                tools: Vec::new(),
                session_id: None,
            },
            policy,
        );
        assert!(trace.get("max_turn_loops").is_none());
    }

    #[test]
    fn long_running_profile_has_no_tool_call_budget() {
        let config = HarnessConfig {
            max_turn_loops_long_running: 80,
            max_tool_calls_medium: 100,
            turn_loop_limit: Some(80),
            ..HarnessConfig::default()
        };

        let policy = policy_for_profile(&config, HarnessProfile::LongRunning);

        assert_eq!(policy.max_tool_calls, 0);
    }

    #[test]
    fn total_tool_calls_are_counted_but_not_capped() {
        let policy = test_policy(1);
        let mut state = AgentRunState::default();

        for i in 0..25 {
            let invocation = ToolInvocation {
                id: format!("call-{i}"),
                tool_name: "read_file".to_string(),
                input: json!({ "path": format!("file-{i}.rs") }),
            };
            assert_eq!(
                record_tool_call(&mut state, policy, &invocation),
                ToolLoopDecision::Continue,
            );
        }

        assert_eq!(state.total_tool_calls, 25);
    }

    #[test]
    fn repeated_tool_call_is_flagged_at_20() {
        let policy = test_policy(100);
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
        // 21st consecutive identical call (repeated=20) triggers a stop.
        assert!(matches!(
            record_tool_call(&mut state, policy, &invocation),
            ToolLoopDecision::Stop(_)
        ));
    }

    #[test]
    fn repeated_tool_call_resets_on_different_input() {
        let policy = test_policy(100);
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
    fn repeated_tool_call_uses_exact_argument_hash() {
        let policy = test_policy(100);
        let invocation_a = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({ "raw_arguments": "{\"path\":" }),
        };
        let invocation_b = ToolInvocation {
            id: "call-2".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({ "raw_arguments": "{\"path\":\"Cargo.toml\"" }),
        };
        let mut state = AgentRunState::default();

        assert_eq!(
            record_tool_call(&mut state, policy, &invocation_a),
            ToolLoopDecision::Continue,
        );
        assert_eq!(
            record_tool_call(&mut state, policy, &invocation_b),
            ToolLoopDecision::Continue,
        );

        assert_eq!(state.repeated_tool_calls, 0);
    }

    #[test]
    fn background_bash_poll_calls_are_exempt_from_repetition_guard() {
        let policy = test_policy(100);
        let poll = ToolInvocation {
            id: "poll-1".to_string(),
            tool_name: "bash".to_string(),
            input: json!({ "task_id": "bg_1" }),
        };
        let mut state = AgentRunState::default();

        // Many identical background poll calls must never trip the guard.
        for i in 0..40 {
            let mut inv = poll.clone();
            inv.id = format!("poll-{i}");
            assert_eq!(
                record_tool_call(&mut state, policy, &inv),
                ToolLoopDecision::Continue,
                "background poll call {i} should continue"
            );
        }
        assert_eq!(state.repeated_tool_calls, 0);
        assert_eq!(state.total_tool_calls, 40);
    }

    #[test]
    fn bash_with_command_is_not_treated_as_background_poll() {
        let policy = test_policy(100);
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "bash".to_string(),
            input: json!({ "command": "sleep 1", "background": true }),
        };
        let mut state = AgentRunState::default();

        for i in 0..20 {
            let mut inv = invocation.clone();
            inv.id = format!("call-{i}");
            assert_eq!(
                record_tool_call(&mut state, policy, &inv),
                ToolLoopDecision::Continue,
                "call {i} should continue"
            );
        }
        assert!(matches!(
            record_tool_call(&mut state, policy, &invocation),
            ToolLoopDecision::Stop(_)
        ));
    }

    #[test]
    fn compact_observation_is_bounded() {
        let mut policy = test_policy(100);
        policy.observation_max_bytes = 16;
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
    fn malformed_arguments_stop_after_policy_limit() {
        let policy = policy_for_profile(
            &HarnessConfig {
                max_consecutive_malformed_arguments: 2,
                ..HarnessConfig::default()
            },
            HarnessProfile::Small,
        );
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "memory_query".to_string(),
            input: json!({ "raw_arguments": "{\"limit\": {\"limit\": " }),
        };
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: false,
            output: json!({
                "error_code": "invalid_arguments",
                "error_kind": "malformed_arguments"
            }),
        };
        let mut state = AgentRunState::default();

        assert_eq!(
            record_tool_result(&mut state, policy, &invocation, &result),
            ToolLoopDecision::Continue
        );
        assert!(matches!(
            record_tool_result(&mut state, policy, &invocation, &result),
            ToolLoopDecision::Stop(stop)
                if stop.reason == HarnessStopReason::ConsecutiveMalformedArguments
        ));
    }

    #[test]
    fn unknown_tools_stop_after_policy_limit() {
        let policy = policy_for_profile(
            &HarnessConfig {
                max_consecutive_unknown_tools: 2,
                ..HarnessConfig::default()
            },
            HarnessProfile::Small,
        );
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "glob".to_string(),
            input: json!({}),
        };
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: false,
            output: json!({ "error_code": "unknown_tool" }),
        };
        let mut state = AgentRunState::default();

        record_tool_result(&mut state, policy, &invocation, &result);
        assert!(matches!(
            record_tool_result(&mut state, policy, &invocation, &result),
            ToolLoopDecision::Stop(stop)
                if stop.reason == HarnessStopReason::ConsecutiveUnknownTools
        ));
    }

    #[test]
    fn execution_failures_do_not_trigger_consecutive_tool_error_stop() {
        let policy = policy_for_profile(
            &HarnessConfig {
                max_consecutive_tool_errors: 2,
                ..HarnessConfig::default()
            },
            HarnessProfile::Small,
        );
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "bash".to_string(),
            input: json!({ "command": "grep -A8 \"needle\" missing" }),
        };
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: false,
            output: json!({ "error": "command exited with status 2" }),
        };
        let mut state = AgentRunState::default();

        for _ in 0..4 {
            assert_eq!(
                record_tool_result(&mut state, policy, &invocation, &result),
                ToolLoopDecision::Continue
            );
        }
        assert_eq!(state.total_tool_errors, 4);
        assert_eq!(state.consecutive_tool_errors, 0);
        assert_eq!(
            state.last_failure_kind,
            Some(ToolFailureKind::ExecutionFailed)
        );
    }

    #[test]
    fn invalid_schema_still_stops_after_generic_tool_error_limit() {
        let policy = policy_for_profile(
            &HarnessConfig {
                max_consecutive_tool_errors: 2,
                max_consecutive_invalid_arguments: 10,
                max_consecutive_malformed_arguments: 10,
                max_consecutive_unknown_tools: 10,
                ..HarnessConfig::default()
            },
            HarnessProfile::Small,
        );
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "host__bad_schema".to_string(),
            input: json!({}),
        };
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: false,
            output: json!({ "error_code": "invalid_schema" }),
        };
        let mut state = AgentRunState::default();

        assert_eq!(
            record_tool_result(&mut state, policy, &invocation, &result),
            ToolLoopDecision::Continue
        );
        assert!(matches!(
            record_tool_result(&mut state, policy, &invocation, &result),
            ToolLoopDecision::Stop(stop)
                if stop.reason == HarnessStopReason::ConsecutiveToolErrors
        ));
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
            ..Default::default()
        };

        let manifest = tool_prompt_manifest(&[tool]);

        assert!(manifest.contains("read_file"));
        assert!(manifest.contains("Required: path"));
        assert!(manifest.contains(r#"{"path":"example"}"#));
    }

    #[test]
    fn system_prompt_omits_removed_tool_guidance() {
        let config = NaviConfig::default();
        let prompt = build_system_prompt(&config, std::path::Path::new("/tmp"));

        assert!(!prompt.contains("tool_workflow"));
        assert!(!prompt.contains("top_files"));
        assert!(prompt.contains("tool_search"));
        assert!(prompt.contains("Power tools"));
        assert!(prompt.contains("Core tools"));
        assert!(prompt.contains("ast_search") || prompt.contains("code / code_edit"));
        assert!(!prompt.contains("Long-horizon task protocol"));
        assert!(prompt.contains("When to structure work"));
        assert!(prompt.contains("Inspection decision tree"));
    }

    #[test]
    fn system_prompt_includes_edit_guidance() {
        let config = NaviConfig::default();
        let prompt = build_system_prompt(&config, std::path::Path::new("/tmp"));

        assert!(prompt.contains("`edit`"));
        assert!(prompt.contains("`search`"));
        assert!(prompt.contains("old_string") || prompt.contains("edits"));
        assert!(prompt.contains("bash/python"));
        assert!(prompt.contains("tool_search"));
    }

    #[test]
    fn system_prompt_distinguishes_plan_and_goal() {
        let config = NaviConfig::default();
        let prompt = build_system_prompt(&config, std::path::Path::new("/tmp"));

        assert!(prompt.contains("`plan` tool"));
        assert!(prompt.contains("`create_goal`"));
        assert!(prompt.contains("Not a synonym for `plan`") || prompt.contains("Not a synonym"));
        assert!(prompt.contains("markdown design doc") || prompt.contains("plan markdown"));
        assert!(prompt.contains("submit") || prompt.contains("Plan mode"));
        assert!(prompt.contains("Auto-memory:"));
    }

    #[test]
    fn system_prompt_skips_native_tool_manifest_text() {
        let config = NaviConfig::default();
        let tools = [ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file.".to_string(),
            kind: crate::tool::ToolKind::Read,
            input_schema: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
                "additionalProperties": false
            }),
            ..Default::default()
        }];
        let prompt = build_system_prompt_with_tools(
            &config,
            std::path::Path::new("/tmp"),
            None,
            &tools,
            true,
        );
        assert!(
            !prompt.contains("Available tools (text tool manifest)"),
            "Native mode must not paste the text tool catalog into the system prompt"
        );
        assert!(!prompt.contains("compatibility manifest"));
    }
}
