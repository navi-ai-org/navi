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
        tools
            .into_iter()
            .filter(|tool| whitelist.contains(&tool.name))
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
    ) -> Result<Option<u64>>;
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
    ) -> Result<Option<u64>> {
        state.auto_compact(messages, provider, model, config).await
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
