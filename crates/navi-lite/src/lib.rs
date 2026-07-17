use anyhow::Result;
use async_trait::async_trait;
use navi_core::config::{HarnessConfig, MemoryConfig, ModelConfig, RegistryConfig};
use navi_core::{
    AgentEvent, AgentRuntime, AgentRuntimeOptions, CompactState, CompactionStrategy, ContextPacket,
    LoadedConfig, ModelMessage, ModelProvider, NaviConfig, PermissionMode, ProviderConfig,
    ProviderKind, RenderedPrompt, RuntimeComponents, SecurityConfig, SecurityDecision,
    SecurityPolicy, SessionHooks, SessionId, SystemPromptInput, Tool, ToolDefinition, ToolExecutor,
    ToolInvocation, ToolKind, ToolResult, ToolSecurityPolicy,
};
use navi_openai::OpenAiProvider;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

pub const HEALTH_CHECK_TOOL: &str = "lite_health_check";
pub const EMIT_REPORT_TOOL: &str = "lite_emit_report";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiteConfig {
    pub project_dir: PathBuf,
    pub data_dir: PathBuf,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub provider_kind: ProviderKind,
}

impl LiteConfig {
    pub fn from_env(project_dir: impl Into<PathBuf>) -> Result<Self> {
        let base_url = std::env::var("NAVI_LITE_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        let api_key = std::env::var("NAVI_LITE_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .map_err(|_| anyhow::anyhow!("set NAVI_LITE_API_KEY or OPENAI_API_KEY"))?;
        let model = std::env::var("NAVI_LITE_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
        Ok(Self {
            project_dir: project_dir.into(),
            data_dir: std::env::temp_dir().join("navi-lite"),
            base_url,
            api_key,
            model,
            provider_kind: ProviderKind::OpenAiResponses,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiteMission {
    pub id: String,
    pub task: String,
    pub allowed_tools: Vec<String>,
}

impl LiteMission {
    pub fn health_check() -> Self {
        Self {
            id: "health-check".to_string(),
            task:
                "Execute the health check, then emit the final report JSON using lite_emit_report."
                    .to_string(),
            allowed_tools: vec![HEALTH_CHECK_TOOL.to_string(), EMIT_REPORT_TOOL.to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiteMissionResult {
    pub ok: bool,
    pub session_id: String,
    pub raw_agent_text: String,
    pub report: Option<Value>,
    pub tool_names: Vec<String>,
}

pub struct LiteRuntime {
    config: LiteConfig,
    provider: Arc<dyn ModelProvider>,
}

impl LiteRuntime {
    pub fn new(config: LiteConfig) -> Result<Self> {
        let provider_config = provider_config(&config);
        let provider = OpenAiProvider::from_provider_config_with_key(
            &provider_config,
            config.api_key.clone(),
        )?;
        Ok(Self::with_provider(config, Arc::new(provider)))
    }

    pub fn with_provider(config: LiteConfig, provider: Arc<dyn ModelProvider>) -> Self {
        Self { config, provider }
    }

    pub async fn run_mission(&self, mission: LiteMission) -> Result<LiteMissionResult> {
        let loaded_config = loaded_config(&self.config);
        let security_policy = SecurityPolicy::new(
            self.config.project_dir.clone(),
            self.config.data_dir.clone(),
            loaded_config.config.effective_security_config(),
        )?;
        let security = Arc::new(LiteSecurityPolicy::new(mission.allowed_tools.clone()));
        let mut executor =
            ToolExecutor::empty_with_security_policy(security_policy, security.clone());
        executor.register_tool(Arc::new(LiteHealthCheckTool));
        executor.register_tool(Arc::new(LiteEmitReportTool));
        let executor = Arc::new(executor);

        let components = RuntimeComponents {
            security,
            prompt: Arc::new(LitePromptBuilder::new(mission.clone())),
            compaction: Arc::new(NoopCompactionStrategy),
            hooks: Arc::new(NoopLiteHooks),
            ..RuntimeComponents::default()
        };

        let mut runtime = AgentRuntime::new(AgentRuntimeOptions {
            loaded_config,
            model_provider: self.provider.clone(),
            project_dir: self.config.project_dir.clone(),
            tool_executor: Some(executor.clone()),
            context_packets: Vec::<ContextPacket>::new(),
            active_skills: Vec::new(),
            initial_messages: Vec::new(),
            initial_events: Vec::new(),
            initial_created_at: None,
            initial_updated_at: None,
            initial_goal: None,
            session_id: Some(SessionId::new(mission.id.clone())),
            event_tx: None,
            runtime_components: Some(components),
            session_title_handle: None,
            memory_extraction_model: None,
        });

        let session_id = runtime.start_session()?.as_str().to_string();
        let response = runtime.send_turn(mission.task).await?;
        let (report, tool_names) = report_from_events(runtime.events());
        Ok(LiteMissionResult {
            ok: report.is_some() || !response.text.trim().is_empty(),
            session_id,
            raw_agent_text: response.text,
            report,
            tool_names,
        })
    }
}

#[derive(Debug)]
pub struct LiteSecurityPolicy {
    allowed: HashSet<String>,
}

impl LiteSecurityPolicy {
    pub fn new(allowed: Vec<String>) -> Self {
        Self {
            allowed: allowed.into_iter().collect(),
        }
    }
}

impl ToolSecurityPolicy for LiteSecurityPolicy {
    fn validate_tool(
        &self,
        base_policy: &SecurityPolicy,
        definition: &ToolDefinition,
        invocation: &ToolInvocation,
    ) -> SecurityDecision {
        if !self.allowed.contains(&definition.name) {
            return SecurityDecision::Deny(format!(
                "tool `{}` is not allowed by this lite mission",
                definition.name
            ));
        }
        if !self.allowed.contains(&invocation.tool_name) {
            return SecurityDecision::Deny(format!(
                "tool `{}` is not allowed by this lite mission",
                invocation.tool_name
            ));
        }
        base_policy.validate_tool_invocation(definition, invocation)
    }
}

#[derive(Debug, Clone)]
struct LitePromptBuilder {
    mission: LiteMission,
}

impl LitePromptBuilder {
    fn new(mission: LiteMission) -> Self {
        Self { mission }
    }
}

impl navi_core::PromptBuilder for LitePromptBuilder {
    fn build(
        &self,
        input: SystemPromptInput,
        _cache: Arc<navi_core::PromptCache>,
    ) -> RenderedPrompt {
        let tools = input
            .tools
            .iter()
            .map(|tool| format!("- `{}`: {}", tool.name, tool.description))
            .collect::<Vec<_>>()
            .join("\n");
        RenderedPrompt {
            instructions: format!(
                "You are NAVI Lite, a sealed edge agent.\n\
                 Mission id: {}.\n\
                 You may only call the listed tools. Do not request shell, filesystem, package, patch, plugin, MCP, or network tools.\n\
                 Call `{health}` first, then call `{report}` with a JSON report object.\n\
                 The report object must include: ok, summary, checks, risks, recommended_actions.\n\
                 Available tools:\n{}",
                self.mission.id,
                tools,
                health = HEALTH_CHECK_TOOL,
                report = EMIT_REPORT_TOOL
            ),
            developer_messages: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct NoopCompactionStrategy;

#[async_trait]
impl CompactionStrategy for NoopCompactionStrategy {
    fn micro_compact(
        &self,
        _messages: &mut Vec<ModelMessage>,
        _gap_threshold_minutes: u64,
    ) -> usize {
        0
    }

    async fn auto_compact(
        &self,
        _state: &mut CompactState,
        _messages: &mut Vec<ModelMessage>,
        _provider: &dyn ModelProvider,
        _model: &str,
        _config: &HarnessConfig,
    ) -> Result<Option<navi_core::CompactOutcome>> {
        Ok(None)
    }
}

#[derive(Debug)]
struct NoopLiteHooks;

impl SessionHooks for NoopLiteHooks {}

#[derive(Debug)]
struct LiteHealthCheckTool;

#[async_trait]
impl Tool for LiteHealthCheckTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            HEALTH_CHECK_TOOL,
            "Collects a minimal local health snapshot without shell commands.",
            ToolKind::Read,
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {}
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let output = json!({
            "timestamp_unix": unix_timestamp(),
            "hostname": hostname(),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "current_dir": std::env::current_dir().ok().map(|p| p.display().to_string()),
            "uptime_seconds": linux_uptime_seconds(),
        });
        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: true,
            output,
        })
    }
}

#[derive(Debug)]
struct LiteEmitReportTool;

#[async_trait]
impl Tool for LiteEmitReportTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            EMIT_REPORT_TOOL,
            "Emits the final structured mission report JSON.",
            ToolKind::Read,
            json!({
                "type": "object",
                "required": ["report"],
                "additionalProperties": false,
                "properties": {
                    "report": {
                        "type": "object",
                        "required": ["ok", "summary", "checks", "risks", "recommended_actions"],
                        "properties": {
                            "ok": { "type": "boolean" },
                            "summary": { "type": "string" },
                            "checks": { "type": "array" },
                            "risks": { "type": "array" },
                            "recommended_actions": { "type": "array" }
                        }
                    }
                }
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let report = invocation
            .input
            .get("report")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing report"))?;
        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: true,
            output: json!({ "report": report }),
        })
    }
}

fn loaded_config(config: &LiteConfig) -> LoadedConfig {
    let mut navi = NaviConfig::default();
    navi.model = ModelConfig {
        provider: "navi-lite".to_string(),
        name: config.model.clone(),
    };
    navi.providers = vec![provider_config(config)];
    navi.memory = MemoryConfig {
        enabled: false,
        session_memory_enabled: false,
        ..MemoryConfig::default()
    };
    navi.registry = RegistryConfig {
        update_enabled: false,
        ..RegistryConfig::default()
    };
    navi.security = SecurityConfig {
        permission_mode: PermissionMode::Auto,
        restrict_paths_to_project: true,
        allow_external_plugins: false,
        ..SecurityConfig::default()
    };
    navi.goals.enabled = false;
    LoadedConfig {
        config: navi,
        global_config_path: None,
        project_config_path: None,
        data_dir: config.data_dir.clone(),
    }
}

fn provider_config(config: &LiteConfig) -> ProviderConfig {
    ProviderConfig {
        id: "navi-lite".to_string(),
        label: "NAVI Lite Gateway".to_string(),
        kind: config.provider_kind,
        api_key_env: "NAVI_LITE_API_KEY".to_string(),
        base_url: Some(config.base_url.clone()),
        tool_calling_mode: Some(navi_core::ToolCallingMode::Native),
        ..ProviderConfig::default()
    }
}

fn report_from_events(events: &[AgentEvent]) -> (Option<Value>, Vec<String>) {
    let mut requested = HashMap::<String, String>::new();
    let mut tool_names = Vec::new();
    let mut report = None;

    for event in events {
        match event {
            AgentEvent::ToolRequested(invocation) => {
                requested.insert(invocation.id.clone(), invocation.tool_name.clone());
                tool_names.push(invocation.tool_name.clone());
            }
            AgentEvent::ToolCompleted(result) => {
                if requested.get(&result.invocation_id).map(String::as_str)
                    == Some(EMIT_REPORT_TOOL)
                    && result.ok
                {
                    report = result.output.get("report").cloned();
                }
            }
            _ => {}
        }
    }

    tool_names.sort();
    tool_names.dedup();
    (report, tool_names)
}

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn hostname() -> Option<String> {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn linux_uptime_seconds() -> Option<u64> {
    let raw = std::fs::read_to_string("/proc/uptime").ok()?;
    let first = raw.split_whitespace().next()?;
    first.split('.').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use navi_core::{ModelRequest, ModelStream, ModelStreamEvent};
    use std::sync::Mutex;

    struct MockProvider {
        calls: Mutex<usize>,
    }

    impl ModelProvider for MockProvider {
        fn stream(&self, _request: ModelRequest) -> ModelStream {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            if *calls == 1 {
                Box::pin(stream::iter(vec![
                    Ok(ModelStreamEvent::ToolCall(ToolInvocation {
                        id: "health-1".to_string(),
                        tool_name: HEALTH_CHECK_TOOL.to_string(),
                        input: json!({}),
                    })),
                    Ok(ModelStreamEvent::Done),
                ]))
            } else {
                Box::pin(stream::iter(vec![
                    Ok(ModelStreamEvent::ToolCall(ToolInvocation {
                        id: "report-1".to_string(),
                        tool_name: EMIT_REPORT_TOOL.to_string(),
                        input: json!({
                            "report": {
                                "ok": true,
                                "summary": "healthy",
                                "checks": [],
                                "risks": [],
                                "recommended_actions": []
                            }
                        }),
                    })),
                    Ok(ModelStreamEvent::TextDelta {
                        text: "{\"ok\":true}".to_string(),
                    }),
                    Ok(ModelStreamEvent::Done),
                ]))
            }
        }
    }

    fn test_config() -> LiteConfig {
        let temp = tempfile::tempdir().unwrap();
        LiteConfig {
            project_dir: temp.path().to_path_buf(),
            data_dir: temp.path().join("data"),
            base_url: "https://example.test/v1".to_string(),
            api_key: "test".to_string(),
            model: "test-model".to_string(),
            provider_kind: ProviderKind::OpenAiResponses,
        }
    }

    #[test]
    fn lite_security_denies_unknown_tools() {
        let temp = tempfile::tempdir().unwrap();
        let policy = SecurityPolicy::new(
            temp.path().to_path_buf(),
            temp.path().join("data"),
            SecurityConfig {
                permission_mode: PermissionMode::Auto,
                restrict_paths_to_project: true,
                ..SecurityConfig::default()
            },
        )
        .unwrap();
        let security = LiteSecurityPolicy::new(vec![HEALTH_CHECK_TOOL.to_string()]);
        let def = ToolDefinition::new(
            "bash",
            "shell",
            ToolKind::Command,
            json!({ "type": "object" }),
        );
        let inv = ToolInvocation {
            id: "1".to_string(),
            tool_name: "bash".to_string(),
            input: json!({ "command": "echo hi" }),
        };
        assert!(matches!(
            security.validate_tool(&policy, &def, &inv),
            SecurityDecision::Deny(_)
        ));
    }

    #[test]
    fn empty_executor_starts_without_tools() {
        let temp = tempfile::tempdir().unwrap();
        let policy = SecurityPolicy::new(
            temp.path().to_path_buf(),
            temp.path().join("data"),
            SecurityConfig::default(),
        )
        .unwrap();
        let executor = ToolExecutor::empty(policy);
        assert!(executor.tool_names().is_empty());
    }

    // Session snapshot uses block_in_place — needs multi_thread Tokio.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn health_mission_returns_report_from_emit_tool() {
        let config = test_config();
        let runtime = LiteRuntime::with_provider(
            config,
            Arc::new(MockProvider {
                calls: Mutex::new(0),
            }),
        );
        let result = runtime
            .run_mission(LiteMission::health_check())
            .await
            .unwrap();
        assert!(result.ok);
        assert_eq!(
            result
                .report
                .unwrap()
                .get("summary")
                .and_then(Value::as_str),
            Some("healthy")
        );
        assert_eq!(
            result.tool_names,
            vec![EMIT_REPORT_TOOL.to_string(), HEALTH_CHECK_TOOL.to_string()]
        );
    }
}
