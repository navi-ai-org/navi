//! Concrete implementations of the swappable engine parts.
//!
//! These are the values the host installs into the typed slots when a component
//! activates. They are plain Rust implementations of `navi_core` traits — the
//! point is that *which one the engine uses* is decided by the composition
//! (kernel generations), not by the build.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use navi_core::{
    CompactOutcome, CompactState, CompactionStrategy, ModelMessage, ModelProvider, ModelRole,
    PromptBuilder, PromptCache, RenderedPrompt, SecurityDecision, SecurityPolicy, SessionHooks,
    SystemPromptInput, ToolDefinition, ToolInvocation, ToolResult, config::HarnessConfig,
    harness::{AgentRunState, HarnessPolicy, ToolLoopDecision},
    runtime_components::{HarnessDriver, ToolSecurityPolicy},
};

/// The values a component provides to the engine when it activates.
///
/// Every seam of `RuntimeComponents` is represented: activating a component
/// installs its value into the matching typed slot, and the engine's proxy
/// resolves that slot per call.
#[derive(Default, Clone)]
pub struct ComponentValues {
    /// `navi.compaction@1`
    pub compaction: Option<Arc<dyn CompactionStrategy>>,
    /// `navi.prompt@1`
    pub prompt: Option<Arc<dyn PromptBuilder>>,
    /// `navi.security@1`
    pub security: Option<Arc<dyn ToolSecurityPolicy>>,
    /// `navi.harness@1`
    pub harness: Option<Arc<dyn HarnessDriver>>,
    /// `navi.hooks@1`
    pub hooks: Option<Arc<dyn SessionHooks>>,
}

impl ComponentValues {
    /// True when the component provides nothing typed (a pure JSON capability).
    pub fn is_empty(&self) -> bool {
        self.compaction.is_none()
            && self.prompt.is_none()
            && self.security.is_none()
            && self.harness.is_none()
            && self.hooks.is_none()
    }
}

/// Security policy that **denies a configured set of tools** on top of the base
/// policy (`navi.security.no_network`): the base decision is kept unless it is
/// an allow for a denied tool.
pub struct DenyToolsPolicy {
    denied: Vec<String>,
    reason: String,
}

impl DenyToolsPolicy {
    /// Builds the policy for `denied` tool names.
    pub fn new(denied: Vec<String>) -> Self {
        Self {
            reason: format!(
                "denied by the active security component: {}",
                denied.join(", ")
            ),
            denied,
        }
    }

    /// Tool names this policy refuses.
    pub fn denied(&self) -> &[String] {
        &self.denied
    }
}

impl ToolSecurityPolicy for DenyToolsPolicy {
    fn validate_tool(
        &self,
        base_policy: &SecurityPolicy,
        definition: &ToolDefinition,
        invocation: &ToolInvocation,
    ) -> SecurityDecision {
        if self.denied.contains(&definition.name) {
            return SecurityDecision::Deny(self.reason.clone());
        }
        base_policy.validate_tool_invocation(definition, invocation)
    }
}

/// Harness driver that keeps the default loop behaviour but **truncates tool
/// observations** to a maximum length (`navi.harness.terse`).
#[derive(Default)]
pub struct TruncatingHarness {
    /// Maximum characters kept per observation.
    pub limit: usize,
    /// Driver used for everything else.
    pub fallback: Option<Arc<dyn HarnessDriver>>,
}

impl TruncatingHarness {
    /// Builds the driver with a per-observation character limit.
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            fallback: None,
        }
    }

    fn driver(&self) -> Arc<dyn HarnessDriver> {
        self.fallback
            .clone()
            .unwrap_or_else(|| Arc::new(navi_core::DefaultHarnessDriver))
    }
}

impl HarnessDriver for TruncatingHarness {
    fn filter_tools(
        &self,
        tools: Vec<ToolDefinition>,
        allowed_tool_names: Option<&[String]>,
    ) -> Vec<ToolDefinition> {
        self.driver().filter_tools(tools, allowed_tool_names)
    }

    fn record_tool_call(
        &self,
        state: &mut AgentRunState,
        policy: HarnessPolicy,
        invocation: &ToolInvocation,
    ) -> ToolLoopDecision {
        self.driver().record_tool_call(state, policy, invocation)
    }

    fn record_tool_result(
        &self,
        state: &mut AgentRunState,
        policy: HarnessPolicy,
        invocation: &ToolInvocation,
        result: &ToolResult,
    ) -> ToolLoopDecision {
        self.driver()
            .record_tool_result(state, policy, invocation, result)
    }

    fn compact_tool_observation(
        &self,
        invocation: &ToolInvocation,
        result: &ToolResult,
        policy: HarnessPolicy,
    ) -> String {
        let observation = self
            .driver()
            .compact_tool_observation(invocation, result, policy);
        if self.limit == 0 || observation.chars().count() <= self.limit {
            return observation;
        }
        let head: String = observation.chars().take(self.limit).collect();
        format!("{head}\n… [observation truncated by navi.harness.terse]")
    }
}

/// Session hooks that append an audit line per event (`navi.hooks.audit`).
#[derive(Debug, Default)]
pub struct AuditHooks {
    /// Where the audit lines go (`None` = nowhere, e.g. in tests).
    pub path: Option<std::path::PathBuf>,
}

impl AuditHooks {
    /// Builds the hooks writing to `path`.
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: Some(path.into()),
        }
    }

    fn record(&self, line: &str) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            use std::io::Write;
            let _ = writeln!(file, "{line}");
        }
    }
}

impl SessionHooks for AuditHooks {
    fn on_session_start(&self, session_id: &str) {
        self.record(&format!("session.start {session_id}"));
    }

    fn on_turn_start(&self, session_id: &str, task: &str) {
        self.record(&format!("turn.start {session_id} task={task}"));
    }

    fn on_tool_call(&self, invocation: &ToolInvocation) {
        self.record(&format!("tool.call {}", invocation.tool_name));
    }

    fn on_tool_result(&self, result: &ToolResult) {
        self.record(&format!("tool.result ok={}", result.ok));
    }

    fn on_turn_end(&self, session_id: &str, output: &str) {
        self.record(&format!(
            "turn.end {session_id} chars={}",
            output.chars().count()
        ));
    }

    fn on_session_end(&self, session_id: &str) {
        self.record(&format!("session.end {session_id}"));
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Aggressive micro-compaction: clears **every** tool result older than the gap.
///
/// The default strategy only clears read-only tool results; this one trades
/// detail for context, which is exactly the kind of policy a user may want to
/// swap in mid-session.
#[derive(Debug, Default)]
pub struct TerseCompaction;

#[async_trait::async_trait]
impl CompactionStrategy for TerseCompaction {
    fn micro_compact(&self, messages: &mut Vec<ModelMessage>, gap_threshold_minutes: u64) -> usize {
        let gap_ms = gap_threshold_minutes.saturating_mul(60_000);
        let last_assistant = messages
            .iter()
            .rev()
            .find(|message| message.role == ModelRole::Assistant)
            .and_then(|message| message.created_at);
        let Some(last_ts) = last_assistant else {
            return 0;
        };
        if now_millis().saturating_sub(last_ts) < gap_ms {
            return 0;
        }
        let mut cleared = 0;
        for message in messages.iter_mut() {
            if message.role == ModelRole::Tool && !message.content.contains("[terse compaction]") {
                message.content = "[terse compaction] old tool result cleared".to_string();
                message.content_parts.clear();
                cleared += 1;
            }
        }
        cleared
    }

    async fn auto_compact(
        &self,
        state: &mut CompactState,
        messages: &mut Vec<ModelMessage>,
        provider: &dyn ModelProvider,
        model: &str,
        config: &HarnessConfig,
    ) -> anyhow::Result<Option<CompactOutcome>> {
        // Reuses NAVI's summarisation path; only micro-compaction is specialised.
        state.auto_compact(messages, provider, model, config).await
    }
}

/// Wraps another prompt builder and stamps a visible marker into the base
/// instructions, so a swap is observable in the system prompt.
pub struct BrandedPrompt {
    inner: Arc<dyn PromptBuilder>,
    marker: String,
}

impl BrandedPrompt {
    /// Builds a branded prompt over `inner`.
    pub fn new(inner: Arc<dyn PromptBuilder>, marker: impl Into<String>) -> Self {
        Self {
            inner,
            marker: marker.into(),
        }
    }
}

impl PromptBuilder for BrandedPrompt {
    fn build(&self, input: SystemPromptInput, cache: Arc<PromptCache>) -> RenderedPrompt {
        let mut rendered = self.inner.build(input, cache);
        if !rendered.instructions.contains(&self.marker) {
            rendered.instructions.push('\n');
            rendered.instructions.push_str(&self.marker);
            rendered.instructions.push('\n');
        }
        rendered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use navi_core::DefaultCompactionStrategy;

    fn old_tool_result(tool: &str) -> ModelMessage {
        ModelMessage {
            role: ModelRole::Tool,
            content: "file contents".to_string(),
            content_parts: Vec::new(),
            tool_call_id: Some(format!("call-{tool}")),
            tool_name: Some(tool.to_string()),
            tool_calls: Vec::new(),
            created_at: Some(now_millis().saturating_sub(3_600_000)),
            thinking_content: None,
        }
    }

    fn old_assistant() -> ModelMessage {
        ModelMessage {
            role: ModelRole::Assistant,
            content: "done".to_string(),
            content_parts: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            tool_calls: Vec::new(),
            created_at: Some(now_millis().saturating_sub(3_600_000)),
            thinking_content: None,
        }
    }

    #[test]
    fn terse_compaction_clears_more_than_the_default() {
        let build = || {
            vec![
                old_tool_result("read_file"),
                old_tool_result("edit"),
                old_assistant(),
            ]
        };

        let mut default_messages = build();
        let default_cleared =
            DefaultCompactionStrategy.micro_compact(&mut default_messages, 60);
        let mut terse_messages = build();
        let terse_cleared = TerseCompaction.micro_compact(&mut terse_messages, 60);

        assert_eq!(default_cleared, 1, "the default only clears read-only results");
        assert_eq!(terse_cleared, 2, "the terse strategy clears every tool result");
        assert!(terse_messages[1].content.contains("[terse compaction]"));
        assert_eq!(
            default_messages[1].content, "file contents",
            "the default leaves the write-tool result alone"
        );
    }

    #[test]
    fn terse_compaction_respects_the_gap_threshold() {
        let mut messages = vec![
            ModelMessage {
                role: ModelRole::Tool,
                content: "fresh".to_string(),
                content_parts: Vec::new(),
                tool_call_id: None,
                tool_name: Some("read_file".to_string()),
                tool_calls: Vec::new(),
                created_at: Some(now_millis()),
                thinking_content: None,
            },
            ModelMessage {
                role: ModelRole::Assistant,
                content: "now".to_string(),
                content_parts: Vec::new(),
                tool_call_id: None,
                tool_name: None,
                tool_calls: Vec::new(),
                created_at: Some(now_millis()),
                thinking_content: None,
            },
        ];
        assert_eq!(TerseCompaction.micro_compact(&mut messages, 60), 0);
        assert_eq!(messages[0].content, "fresh");
    }
}
