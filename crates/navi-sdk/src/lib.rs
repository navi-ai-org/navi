use anyhow::{Context, Result};
use async_trait::async_trait;
use navi_core::{
    AgentMode, AgentRuntime, AgentRuntimeOptions, ApprovalDecision, ContextPacket, ContextSource,
    CredentialSource, CredentialStore, LoadedConfig, ModelOption, ModelProvider, RuntimeEvent,
    SecurityPolicy, SessionId, SessionSnapshot, SkillManifest, Tool, ToolDefinition, ToolExecutor,
    ToolInvocation, ToolKind, ToolResult, active_skills, canonical_provider_id,
    discover_configured_skills, resolve_provider_api_key, resolve_provider_config,
    resolve_provider_credential_status,
};
use navi_mcp::{LoadedMcpServers, McpServerInfo, load_configured_mcp_servers};
use navi_openai::OpenAiProvider;
use navi_plugin_host::{LoadedPlugin, load_configured_plugins};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{Mutex as AsyncMutex, broadcast};

#[derive(Clone)]
pub struct NaviEngineBuilder {
    project_dir: PathBuf,
    loaded_config: Option<LoadedConfig>,
    agent_mode: Option<AgentMode>,
    host_tools: Vec<Arc<dyn Tool>>,
}

impl NaviEngineBuilder {
    pub fn from_project(project_dir: impl Into<PathBuf>) -> Self {
        Self {
            project_dir: project_dir.into(),
            loaded_config: None,
            agent_mode: None,
            host_tools: Vec::new(),
        }
    }

    pub fn loaded_config(mut self, loaded_config: LoadedConfig) -> Self {
        self.loaded_config = Some(loaded_config);
        self
    }

    pub fn agent_mode(mut self, agent_mode: AgentMode) -> Self {
        self.agent_mode = Some(agent_mode);
        self
    }

    pub fn host_tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.host_tools.push(tool);
        self
    }

    pub fn build(self) -> Result<NaviEngine> {
        let loaded_config = match self.loaded_config {
            Some(config) => config,
            None => navi_core::NaviConfig::load(&self.project_dir)?,
        };
        Ok(NaviEngine {
            inner: Arc::new(NaviEngineInner {
                project_dir: self.project_dir,
                loaded_config,
                agent_mode: self.agent_mode,
                host_tools: self.host_tools,
                sessions: Mutex::new(HashMap::new()),
            }),
        })
    }
}

#[derive(Clone)]
pub struct NaviEngine {
    inner: Arc<NaviEngineInner>,
}

struct NaviEngineInner {
    project_dir: PathBuf,
    loaded_config: LoadedConfig,
    agent_mode: Option<AgentMode>,
    host_tools: Vec<Arc<dyn Tool>>,
    sessions: Mutex<HashMap<String, Arc<NaviSession>>>,
}

pub struct NaviSession {
    runtime: AsyncMutex<AgentRuntime>,
    events: broadcast::Receiver<RuntimeEvent>,
    approval_resolver: navi_core::ApprovalResolver,
    turn_canceller: navi_core::TurnCanceller,
    mcp: LoadedMcpServers,
    _plugins: Vec<LoadedPlugin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviSessionRequest {
    #[serde(default)]
    pub project_dir: Option<PathBuf>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub agent_mode: Option<AgentMode>,
    #[serde(default)]
    pub context_packets: Vec<ContextPacket>,
    #[serde(default)]
    pub active_skills: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviSessionInfo {
    pub id: String,
    pub project_dir: PathBuf,
    pub model: String,
    pub provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviTurnRequest {
    pub session_id: String,
    pub message: String,
    #[serde(default)]
    pub context_packets: Vec<ContextPacket>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviTurnResponse {
    pub session_id: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviModelInfo {
    pub id: String,
    pub name: String,
    pub provider_id: String,
    pub provider_label: String,
    pub task_size: String,
    pub context_window_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviSkillInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviProviderCredentialStatus {
    pub provider_id: String,
    pub configured: bool,
    pub source: Option<String>,
    pub label: String,
    pub detail: Option<String>,
    pub env_var: String,
    pub credential_store_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviProviderAccountInfo {
    pub provider_id: String,
    pub provider_label: String,
    pub env_var: String,
    pub has_stored_key: bool,
    pub status: NaviProviderCredentialStatus,
}

pub struct NaviRuntimeTooling {
    pub security_policy: SecurityPolicy,
    pub tool_executor: Arc<ToolExecutor>,
    pub warnings: Vec<String>,
    _plugins: Vec<LoadedPlugin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviMissingCredentialError {
    pub provider_id: String,
    pub env_var: String,
    pub credential_store_path: PathBuf,
}

impl NaviMissingCredentialError {
    pub fn message(&self) -> String {
        format!(
            "missing API key for provider '{}'. Set {} or add a key to {}",
            self.provider_id,
            self.env_var,
            self.credential_store_path.display()
        )
    }
}

impl fmt::Display for NaviMissingCredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message())
    }
}

impl Error for NaviMissingCredentialError {}

#[derive(Clone)]
pub struct HostToolDefinition {
    pub name: String,
    pub description: String,
    pub kind: ToolKind,
    pub input_schema: Value,
}

#[async_trait]
pub trait HostToolHandler: Send + Sync {
    async fn invoke(&self, input: Value) -> Result<Value>;
}

pub struct SdkHostTool {
    definition: HostToolDefinition,
    handler: Arc<dyn HostToolHandler>,
}

impl SdkHostTool {
    pub fn new(definition: HostToolDefinition, handler: Arc<dyn HostToolHandler>) -> Self {
        Self {
            definition,
            handler,
        }
    }
}

#[async_trait]
impl Tool for SdkHostTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.definition.name.clone(),
            description: self.definition.description.clone(),
            kind: self.definition.kind,
            input_schema: self.definition.input_schema.clone(),
        }
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let output = self.handler.invoke(invocation.input).await?;
        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: true,
            output,
        })
    }
}

impl NaviEngine {
    pub async fn start_session(&self, request: NaviSessionRequest) -> Result<NaviSessionInfo> {
        let project_dir = request
            .project_dir
            .clone()
            .unwrap_or_else(|| self.inner.project_dir.clone());
        let loaded_config = self.inner.loaded_config.clone();
        let provider = build_model_provider(&loaded_config)?;
        let mut tool_executor = build_local_tooling(&loaded_config, project_dir.clone())?;
        for tool in &self.inner.host_tools {
            Arc::get_mut(&mut tool_executor.tool_executor)
                .expect("tool executor is not shared yet")
                .register_tool(tool.clone());
        }
        let mcp = load_configured_mcp_servers(&loaded_config.config.mcp).await;
        for tool in &mcp.tools {
            Arc::get_mut(&mut tool_executor.tool_executor)
                .expect("tool executor is not shared yet")
                .register_tool(tool.clone());
        }
        for warning in &tool_executor.warnings {
            tracing::warn!(warning = %warning, "plugin load warning");
        }

        let mut runtime = AgentRuntime::new(AgentRuntimeOptions {
            loaded_config: loaded_config.clone(),
            model_provider: provider,
            project_dir: project_dir.clone(),
            tool_executor: Some(tool_executor.tool_executor.clone()),
            agent_mode: request.agent_mode.or(self.inner.agent_mode),
            context_packets: request.context_packets,
            active_skills: request.active_skills,
            session_id: request.session_id.map(SessionId),
            event_tx: None,
        });
        let events = runtime.stream_events();
        let session_id = runtime.start_session()?;
        let approval_resolver = runtime.approval_resolver();
        let turn_canceller = runtime.turn_canceller();
        let info = NaviSessionInfo {
            id: session_id.0.clone(),
            project_dir,
            model: loaded_config.config.model.name.clone(),
            provider: loaded_config.config.model.provider.clone(),
        };
        self.inner.sessions.lock().unwrap().insert(
            session_id.0,
            Arc::new(NaviSession {
                runtime: AsyncMutex::new(runtime),
                events,
                approval_resolver,
                turn_canceller,
                mcp,
                _plugins: tool_executor._plugins,
            }),
        );
        Ok(info)
    }

    pub async fn send_turn(&self, request: NaviTurnRequest) -> Result<NaviTurnResponse> {
        let session = self.session(&request.session_id)?;
        let mut runtime = session.runtime.lock().await;
        for packet in request.context_packets {
            runtime.add_context_packet(packet);
        }
        let response = runtime.send_turn(request.message).await?;
        Ok(NaviTurnResponse {
            session_id: request.session_id,
            text: response.text,
        })
    }

    pub async fn cancel_turn(&self, session_id: &str) -> Result<()> {
        let session = self.session(session_id)?;
        session.turn_canceller.cancel();
        Ok(())
    }

    pub async fn resolve_approval(
        &self,
        session_id: &str,
        decision: ApprovalDecision,
    ) -> Result<bool> {
        let session = self.session(session_id)?;
        Ok(session.approval_resolver.resolve(decision))
    }

    pub async fn add_context_packet(&self, session_id: &str, packet: ContextPacket) -> Result<()> {
        let session = self.session(session_id)?;
        let mut runtime = session.runtime.lock().await;
        runtime.add_context_packet(packet);
        Ok(())
    }

    pub async fn snapshot_session(&self, session_id: &str) -> Result<SessionSnapshot> {
        let session = self.session(session_id)?;
        let mut runtime = session.runtime.lock().await;
        runtime.snapshot_session()
    }

    pub async fn set_model(&self, session_id: &str, provider: &str, model: &str) -> Result<()> {
        let session = self.session(session_id)?;
        let mut runtime = session.runtime.lock().await;
        runtime.set_model(provider, model);
        Ok(())
    }

    pub fn list_models(&self) -> Vec<NaviModelInfo> {
        navi_core::available_model_options(&self.inner.loaded_config.config)
            .into_iter()
            .map(model_info_from_option)
            .collect()
    }

    pub fn list_provider_accounts(&self) -> Result<Vec<NaviProviderAccountInfo>> {
        let credential_store = self.credential_store();
        Ok(
            navi_core::provider_catalog(&self.inner.loaded_config.config)
                .into_iter()
                .map(|provider| {
                    let status = self.provider_credential_status_for(
                        &credential_store,
                        &provider.id,
                        Some(&provider),
                    )?;
                    Ok(NaviProviderAccountInfo {
                        has_stored_key: credential_store.get_api_key(&provider.id).is_some(),
                        provider_id: provider.id,
                        provider_label: provider.label,
                        env_var: provider.api_key_env,
                        status,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        )
    }

    pub fn credential_status(&self, provider_id: &str) -> Result<NaviProviderCredentialStatus> {
        self.provider_credential_status_for(&self.credential_store(), provider_id, None)
    }

    pub fn set_provider_api_key(&self, provider_id: &str, api_key: &str) -> Result<()> {
        self.credential_store().set_api_key(provider_id, api_key)
    }

    pub fn delete_provider_api_key(&self, provider_id: &str) -> Result<bool> {
        self.credential_store().delete_api_key(provider_id)
    }

    pub fn list_skills(&self) -> Result<Vec<NaviSkillInfo>> {
        Ok(discover_configured_skills(
            &self.inner.loaded_config.config.skills,
            &self.inner.project_dir,
            &self.inner.loaded_config.data_dir,
        )?
        .into_iter()
        .map(skill_info_from_manifest)
        .collect())
    }

    pub async fn set_session_skills(&self, session_id: &str, skills: Vec<String>) -> Result<()> {
        let session = self.session(session_id)?;
        let mut runtime = session.runtime.lock().await;
        runtime.set_active_skills(skills);
        Ok(())
    }

    pub fn list_mcp_servers(&self, session_id: &str) -> Result<Vec<McpServerInfo>> {
        let session = self.session(session_id)?;
        Ok(session.mcp.servers.clone())
    }

    pub fn list_mcp_tools(&self, session_id: &str) -> Result<Vec<String>> {
        let session = self.session(session_id)?;
        let mut tools = session
            .mcp
            .servers
            .iter()
            .flat_map(|server| server.tools.clone())
            .collect::<Vec<_>>();
        tools.sort();
        Ok(tools)
    }

    pub fn subscribe_events(&self, session_id: &str) -> Result<broadcast::Receiver<RuntimeEvent>> {
        let session = self.session(session_id)?;
        Ok(session.events.resubscribe())
    }

    pub fn session_ids(&self) -> Vec<String> {
        let mut ids = self
            .inner
            .sessions
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    fn session(&self, session_id: &str) -> Result<Arc<NaviSession>> {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .with_context(|| format!("unknown NAVI session `{session_id}`"))
    }

    fn credential_store(&self) -> CredentialStore {
        CredentialStore::new(self.inner.loaded_config.data_dir.clone())
    }

    fn provider_credential_status_for(
        &self,
        credential_store: &CredentialStore,
        provider_id: &str,
        provider: Option<&navi_core::ProviderConfig>,
    ) -> Result<NaviProviderCredentialStatus> {
        let provider_config = match provider {
            Some(provider) => provider.clone(),
            None => resolve_provider_config(&self.inner.loaded_config.config, provider_id)
                .with_context(|| format!("unknown provider {provider_id}"))?,
        };
        let selected_model = (canonical_provider_id(provider_id)
            == canonical_provider_id(&self.inner.loaded_config.config.model.provider))
        .then_some(self.inner.loaded_config.config.model.name.as_str());
        let status = resolve_provider_credential_status(
            credential_store,
            &provider_config,
            provider_id,
            selected_model,
        );

        Ok(NaviProviderCredentialStatus {
            provider_id: provider_id.to_string(),
            configured: status.configured,
            source: status.source.map(credential_source_to_string),
            label: status.label,
            detail: status.detail,
            env_var: provider_config.api_key_env,
            credential_store_path: credential_store.path().to_path_buf(),
        })
    }
}

pub fn build_local_tooling(
    loaded_config: &LoadedConfig,
    project_dir: PathBuf,
) -> Result<NaviRuntimeTooling> {
    let security_policy = SecurityPolicy::new(
        project_dir,
        loaded_config.data_dir.clone(),
        loaded_config.config.security.clone(),
    )?;
    let mut tool_executor = ToolExecutor::new(security_policy.clone());
    let plugin_report = load_configured_plugins(
        &loaded_config.config.plugins,
        &security_policy,
        &mut tool_executor,
    );

    Ok(NaviRuntimeTooling {
        security_policy,
        tool_executor: Arc::new(tool_executor),
        warnings: plugin_report.warnings,
        _plugins: plugin_report.loaded_plugins,
    })
}

pub fn configured_active_skills(
    loaded_config: &LoadedConfig,
    project_dir: &std::path::Path,
    session_active: &[String],
) -> Vec<SkillManifest> {
    match discover_configured_skills(
        &loaded_config.config.skills,
        project_dir,
        &loaded_config.data_dir,
    ) {
        Ok(skills) => active_skills(&skills, &loaded_config.config.skills.active, session_active),
        Err(err) => {
            tracing::warn!(error = %err, "failed to load configured skills");
            Vec::new()
        }
    }
}

pub fn context_packet_from_text(
    source: ContextSource,
    title: &str,
    content: &str,
) -> ContextPacket {
    ContextPacket {
        id: None,
        source,
        title: Some(title.to_string()),
        content: content.to_string(),
        priority: 0,
        metadata: json!({}),
    }
}

pub fn session_id_string(session_id: &SessionId) -> String {
    session_id.0.clone()
}

pub fn build_model_provider(loaded_config: &LoadedConfig) -> Result<Arc<dyn ModelProvider>> {
    let provider_config =
        resolve_provider_config(&loaded_config.config, &loaded_config.config.model.provider)
            .ok_or_else(|| {
                anyhow::anyhow!("unknown provider {}", loaded_config.config.model.provider)
            })?;
    let credential_store = CredentialStore::new(loaded_config.data_dir.clone());
    let api_key = resolve_provider_api_key(
        &credential_store,
        &provider_config,
        &loaded_config.config.model.provider,
    )
    .ok_or_else(|| NaviMissingCredentialError {
        provider_id: provider_config.id.clone(),
        env_var: provider_config.api_key_env.clone(),
        credential_store_path: credential_store.path().to_path_buf(),
    })?;

    Ok(Arc::new(OpenAiProvider::from_provider_config_with_key(
        &provider_config,
        api_key,
    )?))
}

fn model_info_from_option(option: ModelOption) -> NaviModelInfo {
    let id = format!(
        "{}:{}",
        canonical_provider_id(&option.provider_id),
        option.name
    );
    NaviModelInfo {
        id,
        name: option.name,
        provider_id: option.provider_id,
        provider_label: option.provider_label,
        task_size: format!("{:?}", option.task_size),
        context_window_tokens: option.context_window_tokens,
    }
}

fn skill_info_from_manifest(skill: SkillManifest) -> NaviSkillInfo {
    NaviSkillInfo {
        id: skill.id,
        name: skill.name,
        description: skill.description,
    }
}

fn credential_source_to_string(source: CredentialSource) -> String {
    match source {
        CredentialSource::Env => "env",
        CredentialSource::Stored => "stored",
        CredentialSource::External => "external",
        CredentialSource::PublicModel => "public-model",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_credential_error_is_structured_and_downcastable() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let loaded_config = LoadedConfig {
            config: navi_core::NaviConfig {
                model: navi_core::ModelConfig {
                    provider: "test-provider".to_string(),
                    name: "test-model".to_string(),
                },
                providers: vec![navi_core::ProviderConfig {
                    id: "test-provider".to_string(),
                    label: "Test Provider".to_string(),
                    description: String::new(),
                    kind: navi_core::ProviderKind::OpenAiResponses,
                    api_key_env: "NAVI_TEST_MISSING_CREDENTIAL_KEY_98770".to_string(),
                    base_url: Some("https://example.test/v1".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            global_config_path: None,
            project_config_path: None,
            data_dir: tempdir.path().to_path_buf(),
        };

        let error = match build_model_provider(&loaded_config) {
            Ok(_) => panic!("expected missing credential"),
            Err(error) => error,
        };
        let missing = error
            .downcast_ref::<NaviMissingCredentialError>()
            .expect("typed missing credential error");

        assert_eq!(missing.provider_id, "test-provider");
        assert_eq!(missing.env_var, "NAVI_TEST_MISSING_CREDENTIAL_KEY_98770");
        assert_eq!(
            missing.credential_store_path,
            tempdir.path().join("credentials.toml")
        );
        assert!(missing.message().contains("test-provider"));
        assert!(!missing.message().contains("sk-"));
    }
}
