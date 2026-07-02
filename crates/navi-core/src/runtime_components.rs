use crate::compact::{self, CompactState};
use crate::config::HarnessConfig;
use crate::context::render_context_packets;
use crate::harness::{
    AgentRunState, HarnessPolicy, ToolLoopDecision, compact_tool_observation, record_tool_call,
    record_tool_result,
};
use crate::model::{ModelMessage, ModelProvider, ModelRole};
use crate::prompt::{PromptCache, SystemPromptInput, SystemPromptRenderer};
use crate::security::{SecurityDecision, SecurityPolicy};
use crate::skills::{render_active_skills, render_available_skills};
use crate::tool::{ToolDefinition, ToolInvocation, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const STUDY_COMPACTABLE_TOOLS: &[&str] = &[
    "read_file",
    "fs_browser",
    "grep",
    "bash",
    "consultar_materiais",
    "material_lookup",
    "search_materials",
];

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
    fn build(&self, input: SystemPromptInput, cache: Arc<PromptCache>) -> String;
}

#[derive(Debug, Default)]
pub struct DefaultPromptBuilder;

impl PromptBuilder for DefaultPromptBuilder {
    fn build(&self, input: SystemPromptInput, cache: Arc<PromptCache>) -> String {
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

#[derive(Debug, Clone)]
pub struct LearningHarnessConfig {
    pub max_consecutive_errors: usize,
    pub stop_on_repeated_tool: bool,
    pub compact_observation_max_bytes: Option<usize>,
}

impl Default for LearningHarnessConfig {
    fn default() -> Self {
        Self {
            max_consecutive_errors: 5,
            stop_on_repeated_tool: false,
            compact_observation_max_bytes: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LearningHarness {
    config: LearningHarnessConfig,
}

impl LearningHarness {
    pub fn new(config: LearningHarnessConfig) -> Self {
        Self { config }
    }
}

impl HarnessDriver for LearningHarness {
    fn filter_tools(
        &self,
        tools: Vec<ToolDefinition>,
        allowed_tool_names: Option<&[String]>,
    ) -> Vec<ToolDefinition> {
        DefaultHarnessDriver.filter_tools(tools, allowed_tool_names)
    }

    fn record_tool_call(
        &self,
        state: &mut AgentRunState,
        policy: HarnessPolicy,
        invocation: &ToolInvocation,
    ) -> ToolLoopDecision {
        if self.config.stop_on_repeated_tool {
            return record_tool_call(state, policy, invocation);
        }

        let signature = tool_signature_hash(invocation);
        if state.last_tool_signature.as_deref() == Some(signature.as_str()) {
            state.repeated_tool_calls += 1;
        } else {
            state.repeated_tool_calls = 0;
        }
        state.last_tool_signature = Some(signature);
        state.tool_iterations += 1;
        state.total_tool_calls += 1;
        ToolLoopDecision::Continue
    }

    fn record_tool_result(
        &self,
        state: &mut AgentRunState,
        mut policy: HarnessPolicy,
        invocation: &ToolInvocation,
        result: &ToolResult,
    ) -> ToolLoopDecision {
        policy.max_consecutive_tool_errors = self.config.max_consecutive_errors;
        policy.max_consecutive_invalid_arguments = self.config.max_consecutive_errors;
        policy.max_consecutive_malformed_arguments = self.config.max_consecutive_errors;
        policy.max_consecutive_unknown_tools = self.config.max_consecutive_errors;
        record_tool_result(state, policy, invocation, result)
    }

    fn compact_tool_observation(
        &self,
        invocation: &ToolInvocation,
        result: &ToolResult,
        mut policy: HarnessPolicy,
    ) -> String {
        if let Some(max_bytes) = self.config.compact_observation_max_bytes {
            policy.observation_max_bytes = max_bytes;
        }
        compact_tool_observation(invocation, result, policy)
    }
}

#[derive(Debug, Clone)]
pub struct TutorPromptOptions {
    pub role: String,
    pub style: String,
    pub language: String,
}

impl Default for TutorPromptOptions {
    fn default() -> Self {
        Self {
            role: "tutor".to_string(),
            style: "socratic".to_string(),
            language: "pt-BR".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TutorPromptBuilder {
    options: TutorPromptOptions,
}

impl TutorPromptBuilder {
    pub fn new(options: TutorPromptOptions) -> Self {
        Self { options }
    }
}

impl PromptBuilder for TutorPromptBuilder {
    fn build(&self, input: SystemPromptInput, cache: Arc<PromptCache>) -> String {
        let agents = cache
            .read_file(&input.project_dir.join("AGENTS.md"))
            .unwrap_or_else(|_| "No project instructions found.".to_string());
        let manifest = if input.include_tool_prompt_manifest && !input.tools.is_empty() {
            Some(cache.render_tool_manifest(&input.tools))
        } else {
            None
        };

        let mut prompt = format!(
            concat!(
                "You are NAVI Tutor, an autonomous learning guide.\n",
                "Role: {role}. Teaching style: {style}. Response language: {language}.\n",
                "Project/workspace: {workspace}.\n\n",
                "Learning contract:\n",
                "1. Guide the student through understanding, practice, feedback, and review.\n",
                "2. Use tools to inspect materials, generate exercises, grade answers, update progress, and schedule follow-up work.\n",
                "3. Prefer questions, hints, and worked examples over direct answers when the student is practicing.\n",
                "4. Preserve assessment state, grading rationale, schedules, and progress facts.\n",
                "5. You are not a terminal code agent in this runtime unless the host exposes code tools for a lesson.\n\n",
                "Host autonomy:\n",
                "- The embedding host controls tool safety and available capabilities.\n",
                "- If a tool is available, use it when it improves learning outcomes or state accuracy.\n",
                "- Keep responses concise, actionable, and adapted to the student's current level.\n"
            ),
            role = self.options.role,
            style = self.options.style,
            language = self.options.language,
            workspace = input.project_dir.display(),
        );

        if let Some(memory) = input.memory_injection {
            prompt.push_str("\n=== Learning Memory ===\n");
            prompt.push_str(&memory);
            prompt.push('\n');
        }
        prompt.push_str("\n=== Workspace Instructions ===\n");
        prompt.push_str(&agents);
        if let Some(context) = render_context_packets(&input.context_packets) {
            prompt.push_str("\n\n");
            prompt.push_str(&context);
        }
        if let Some(skills) = render_available_skills(&input.available_skills) {
            prompt.push_str("\n\n");
            prompt.push_str(&skills);
        }
        if let Some(skills) = render_active_skills(&input.active_skills) {
            prompt.push_str("\n\n");
            prompt.push_str(&skills);
        }
        if let Some(manifest) = manifest {
            prompt.push_str("\n\n=== Available Tutor Tools ===\n");
            prompt.push_str(&manifest);
        }
        prompt
    }
}

#[derive(Debug, Clone)]
pub struct StudyCompactionConfig {
    pub keep_all_assessments: bool,
    pub exempt_tool_names: Vec<String>,
}

impl Default for StudyCompactionConfig {
    fn default() -> Self {
        Self {
            keep_all_assessments: true,
            exempt_tool_names: vec![
                "grill_avaliacao".to_string(),
                "grading_rubric".to_string(),
                "questionario".to_string(),
                "quiz".to_string(),
                "cron_agendador".to_string(),
                "scheduler".to_string(),
                "student_progress".to_string(),
                "assessment_history".to_string(),
            ],
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StudyCompactionStrategy {
    config: StudyCompactionConfig,
}

impl StudyCompactionStrategy {
    pub fn new(config: StudyCompactionConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl CompactionStrategy for StudyCompactionStrategy {
    fn micro_compact(&self, messages: &mut Vec<ModelMessage>, gap_threshold_minutes: u64) -> usize {
        let now = current_unix_millis();
        let gap_threshold_ms = gap_threshold_minutes * 60 * 1000;
        let last_assistant_ts = messages
            .iter()
            .rev()
            .find(|msg| msg.role == ModelRole::Assistant)
            .and_then(|msg| msg.created_at);

        let Some(last_ts) = last_assistant_ts else {
            return 0;
        };
        if now.saturating_sub(last_ts) < gap_threshold_ms {
            return 0;
        }

        let mut cleared = 0;
        for msg in messages.iter_mut() {
            if msg.role != ModelRole::Tool
                || msg.content.contains("[Old tool result content cleared]")
            {
                continue;
            }
            let Some(tool_name) = msg.tool_name.as_deref() else {
                continue;
            };
            if self.should_preserve_tool(tool_name) {
                continue;
            }
            if STUDY_COMPACTABLE_TOOLS.contains(&tool_name) {
                msg.content = "[Old tool result content cleared]".to_string();
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
    ) -> Result<Option<u64>> {
        state.auto_compact(messages, provider, model, config).await
    }
}

impl StudyCompactionStrategy {
    fn should_preserve_tool(&self, tool_name: &str) -> bool {
        if self
            .config
            .exempt_tool_names
            .iter()
            .any(|name| name == tool_name)
        {
            return true;
        }
        self.config.keep_all_assessments
            && (tool_name.contains("assessment")
                || tool_name.contains("avaliacao")
                || tool_name.contains("quiz")
                || tool_name.contains("questionario")
                || tool_name.contains("rubric"))
    }
}

pub fn learning_runtime_components() -> RuntimeComponents {
    RuntimeComponents {
        security: Arc::new(PermissiveSecurityPolicy),
        harness: Arc::new(LearningHarness::default()),
        prompt: Arc::new(TutorPromptBuilder::default()),
        compaction: Arc::new(StudyCompactionStrategy::default()),
        hooks: Arc::new(NoopSessionHooks),
    }
}

fn tool_signature_hash(invocation: &ToolInvocation) -> String {
    use std::collections::hash_map::DefaultHasher;

    let mut hasher = DefaultHasher::new();
    invocation.tool_name.hash(&mut hasher);
    0xff_u8.hash(&mut hasher);
    let input = serde_json::to_vec(&invocation.input)
        .unwrap_or_else(|_| invocation.input.to_string().into_bytes());
    input.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HarnessProfile;
    use serde_json::json;

    fn test_policy() -> HarnessPolicy {
        HarnessPolicy {
            profile: HarnessProfile::Small,
            observation_max_bytes: 2048,
            max_tool_calls: 0,
            max_parallel_tool_calls: 1,
            max_consecutive_tool_errors: 2,
            max_consecutive_invalid_arguments: 2,
            max_consecutive_malformed_arguments: 2,
            max_consecutive_unknown_tools: 2,
        }
    }

    #[test]
    fn learning_harness_allows_repeated_tool_calls_by_default() {
        let harness = LearningHarness::default();
        let mut state = AgentRunState::default();
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "questionario".to_string(),
            input: json!({"topic": "fractions"}),
        };

        for _ in 0..25 {
            assert!(matches!(
                harness.record_tool_call(&mut state, test_policy(), &invocation),
                ToolLoopDecision::Continue
            ));
        }
        assert_eq!(state.total_tool_calls, 25);
    }

    #[test]
    fn study_compaction_preserves_assessment_tools() {
        let now = current_unix_millis();
        let old = now.saturating_sub(61 * 60 * 1000);
        let mut messages = vec![
            ModelMessage::assistant("ready"),
            ModelMessage::tool_result("c1", "consultar_materiais", "large material"),
            ModelMessage::tool_result("c2", "questionario", "quiz state"),
            ModelMessage::tool_result("c3", "grill_avaliacao", "rubric"),
        ];
        messages[0].created_at = Some(old);

        let strategy = StudyCompactionStrategy::default();
        let cleared = strategy.micro_compact(&mut messages, 60);

        assert_eq!(cleared, 1);
        assert!(messages[1].content.contains("cleared"));
        assert_eq!(messages[2].content, "quiz state");
        assert_eq!(messages[3].content, "rubric");
    }

    #[test]
    fn tutor_prompt_builder_uses_learning_identity() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let input = SystemPromptInput {
            config: crate::NaviConfig::default(),
            project_dir: tempdir.path().to_path_buf(),
            memory_injection: Some("student remembers variables".to_string()),
            tools: Vec::new(),
            include_tool_prompt_manifest: false,
            context_packets: Vec::new(),
            available_skills: Vec::new(),
            active_skills: Vec::new(),
        };

        let prompt = TutorPromptBuilder::default().build(input, Arc::new(PromptCache::new()));
        assert!(prompt.contains("NAVI Tutor"));
        assert!(prompt.contains("student remembers variables"));
        assert!(prompt.contains("Response language: pt-BR"));
    }
}
