use crate::compact::{self, CompactState};
use crate::config::HarnessConfig;
use crate::harness::{
    AgentRunState, HarnessPolicy, ToolLoopDecision, compact_tool_observation, record_tool_call,
    record_tool_result,
};
use crate::model::{ModelMessage, ModelProvider};
use crate::prompt::{PromptCache, RenderedPrompt, SystemPromptInput, SystemPromptRenderer};
use crate::security::{SecurityDecision, SecurityPolicy};
use crate::tool::{ToolDefinition, ToolInvocation, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

#[derive(Clone)]
pub struct RuntimeComponents {
    pub security: Arc<dyn ToolSecurityPolicy>,
    pub harness: Arc<dyn HarnessDriver>,
    pub prompt: Arc<dyn PromptBuilder>,
    pub compaction: Arc<dyn CompactionStrategy>,
    pub hooks: Arc<dyn SessionHooks>,
}

impl Default for RuntimeComponents {
    fn default() -> Self {
        Self {
            security: Arc::new(DefaultToolSecurityPolicy),
            harness: Arc::new(DefaultHarnessDriver),
            prompt: Arc::new(DefaultPromptBuilder),
            compaction: Arc::new(DefaultCompactionStrategy),
            hooks: Arc::new(NoopSessionHooks),
        }
    }
}

pub trait ToolSecurityPolicy: Send + Sync {
    fn validate_tool(
        &self,
        base_policy: &SecurityPolicy,
        definition: &ToolDefinition,
        invocation: &ToolInvocation,
    ) -> SecurityDecision;
}

#[derive(Debug, Default)]
pub struct DefaultToolSecurityPolicy;

impl ToolSecurityPolicy for DefaultToolSecurityPolicy {
    fn validate_tool(
        &self,
        base_policy: &SecurityPolicy,
        definition: &ToolDefinition,
        invocation: &ToolInvocation,
    ) -> SecurityDecision {
        base_policy.validate_tool_invocation(definition, invocation)
    }
}

#[derive(Debug, Default)]
pub struct PermissiveSecurityPolicy;

impl ToolSecurityPolicy for PermissiveSecurityPolicy {
    fn validate_tool(
        &self,
        _base_policy: &SecurityPolicy,
        _definition: &ToolDefinition,
        _invocation: &ToolInvocation,
    ) -> SecurityDecision {
        SecurityDecision::Allow
    }
}

pub trait HarnessDriver: Send + Sync {
    fn filter_tools(
        &self,
        tools: Vec<ToolDefinition>,
        allowed_tool_names: Option<&[String]>,
    ) -> Vec<ToolDefinition>;

    fn record_tool_call(
        &self,
        state: &mut AgentRunState,
        policy: HarnessPolicy,
        invocation: &ToolInvocation,
    ) -> ToolLoopDecision;

    fn record_tool_result(
        &self,
        state: &mut AgentRunState,
        policy: HarnessPolicy,
        invocation: &ToolInvocation,
        result: &ToolResult,
    ) -> ToolLoopDecision;

    fn compact_tool_observation(
        &self,
        invocation: &ToolInvocation,
        result: &ToolResult,
        policy: HarnessPolicy,
    ) -> String;
}

#[derive(Debug, Default)]
pub struct DefaultHarnessDriver;

impl HarnessDriver for DefaultHarnessDriver {
    fn filter_tools(
        &self,
        tools: Vec<ToolDefinition>,
        allowed_tool_names: Option<&[String]>,
    ) -> Vec<ToolDefinition> {
        let Some(whitelist) = allowed_tool_names else {
            return tools;
        };
        // Session-core tools (title, tool_search, question, plan, memory, goal)
        // are callable regardless of the allowlist — keep them in the schema too,
        // otherwise the model cannot see tools it is still allowed to call.
        tools
            .into_iter()
            .filter(|tool| {
                whitelist.contains(&tool.name) || crate::turn::is_session_core_tool(&tool.name)
            })
            .collect()
    }

    fn record_tool_call(
        &self,
        state: &mut AgentRunState,
        policy: HarnessPolicy,
        invocation: &ToolInvocation,
    ) -> ToolLoopDecision {
        record_tool_call(state, policy, invocation)
    }

    fn record_tool_result(
        &self,
        state: &mut AgentRunState,
        policy: HarnessPolicy,
        invocation: &ToolInvocation,
        result: &ToolResult,
    ) -> ToolLoopDecision {
        record_tool_result(state, policy, invocation, result)
    }

    fn compact_tool_observation(
        &self,
        invocation: &ToolInvocation,
        result: &ToolResult,
        policy: HarnessPolicy,
    ) -> String {
        compact_tool_observation(invocation, result, policy)
    }
}

pub trait PromptBuilder: Send + Sync {
    fn build(&self, input: SystemPromptInput, cache: Arc<PromptCache>) -> RenderedPrompt;
}

#[derive(Debug, Default)]
pub struct DefaultPromptBuilder;

impl PromptBuilder for DefaultPromptBuilder {
    fn build(&self, input: SystemPromptInput, cache: Arc<PromptCache>) -> RenderedPrompt {
        SystemPromptRenderer::new(cache).render(input)
    }
}

#[async_trait]
pub trait CompactionStrategy: Send + Sync {
    fn micro_compact(&self, messages: &mut Vec<ModelMessage>, gap_threshold_minutes: u64) -> usize;

    async fn auto_compact(
        &self,
        state: &mut CompactState,
        messages: &mut Vec<ModelMessage>,
        provider: &dyn ModelProvider,
        model: &str,
        config: &HarnessConfig,
    ) -> Result<Option<compact::CompactOutcome>>;

    /// Force full compaction with the session model (manual Compact).
    async fn force_compact(
        &self,
        state: &mut CompactState,
        messages: &mut Vec<ModelMessage>,
        provider: &dyn ModelProvider,
        model: &str,
        config: &HarnessConfig,
    ) -> Result<Option<compact::CompactOutcome>> {
        // Default: same path as auto with force semantics via state.force_compact.
        state.force_compact(messages, provider, model, config).await
    }
}

#[derive(Debug, Default)]
pub struct DefaultCompactionStrategy;

#[async_trait]
impl CompactionStrategy for DefaultCompactionStrategy {
    fn micro_compact(&self, messages: &mut Vec<ModelMessage>, gap_threshold_minutes: u64) -> usize {
        compact::micro_compact(messages, gap_threshold_minutes)
    }

    async fn auto_compact(
        &self,
        state: &mut CompactState,
        messages: &mut Vec<ModelMessage>,
        provider: &dyn ModelProvider,
        model: &str,
        config: &HarnessConfig,
    ) -> Result<Option<compact::CompactOutcome>> {
        state.auto_compact(messages, provider, model, config).await
    }

    async fn force_compact(
        &self,
        state: &mut CompactState,
        messages: &mut Vec<ModelMessage>,
        provider: &dyn ModelProvider,
        model: &str,
        config: &HarnessConfig,
    ) -> Result<Option<compact::CompactOutcome>> {
        state.force_compact(messages, provider, model, config).await
    }
}

pub trait SessionHooks: Send + Sync {
    fn on_session_start(&self, _session_id: &str) {}
    fn on_turn_start(&self, _session_id: &str, _task: &str) {}
    fn on_tool_call(&self, _invocation: &ToolInvocation) {}
    fn on_tool_result(&self, _result: &ToolResult) {}
    fn on_turn_end(&self, _session_id: &str, _output: &str) {}
    fn on_session_end(&self, _session_id: &str) {}
}

#[derive(Debug, Default)]
pub struct NoopSessionHooks;

impl SessionHooks for NoopSessionHooks {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolKind;

    fn def(name: &str) -> ToolDefinition {
        ToolDefinition::new(
            name,
            "",
            ToolKind::Read,
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        )
    }

    fn names(tools: &[ToolDefinition]) -> Vec<String> {
        let mut names: Vec<String> = tools.iter().map(|tool| tool.name.clone()).collect();
        names.sort();
        names
    }

    #[test]
    fn no_allowlist_returns_every_tool() {
        let tools = vec![def("run"), def("plan"), def("update_goal")];
        let kept = DefaultHarnessDriver.filter_tools(tools, None);
        assert_eq!(names(&kept), vec!["plan", "run", "update_goal"]);
    }

    #[test]
    fn allowlist_keeps_session_core_tools_in_schema() {
        // A harness pack entry allowlist that omits goal tools must not hide
        // `update_goal`: auto-continuation runs regardless of the allowlist, so
        // hiding it leaves the model unable to ever close the goal.
        let tools = vec![
            def("run"),
            def("edit"),
            def("plan"),
            def("tool_search"),
            def("set_session_title"),
            def("get_goal"),
            def("create_goal"),
            def("update_goal"),
        ];
        let whitelist = vec!["run".to_string()];
        let kept = DefaultHarnessDriver.filter_tools(tools, Some(&whitelist));
        assert_eq!(
            names(&kept),
            vec![
                "create_goal",
                "get_goal",
                "plan",
                "run",
                "set_session_title",
                "tool_search",
                "update_goal",
            ]
        );
    }

    #[test]
    fn empty_allowlist_keeps_only_session_core_tools() {
        let tools = vec![def("run"), def("plan"), def("update_goal")];
        let whitelist: Vec<String> = Vec::new();
        let kept = DefaultHarnessDriver.filter_tools(tools, Some(&whitelist));
        assert_eq!(names(&kept), vec!["plan", "update_goal"]);
    }

    #[test]
    fn allowlist_never_invents_unregistered_tools() {
        // Session-core names are only *retained*, never added — the filter can
        // narrow the given definitions but never widen the registry.
        let tools = vec![def("run")];
        let whitelist = vec!["update_goal".to_string()];
        let kept = DefaultHarnessDriver.filter_tools(tools, Some(&whitelist));
        assert!(kept.is_empty(), "{:?}", names(&kept));
    }
}
