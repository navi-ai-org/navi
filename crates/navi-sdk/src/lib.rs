use anyhow::{Context, Result};
use async_trait::async_trait;
use navi_core::{
    AgentMode, AgentRuntime, AgentRuntimeOptions, ApprovalDecision, ContextPacket, ContextSource,
    LoadedConfig, ModelOption, ModelProvider, RuntimeEvent, SecurityPolicy, SessionId,
    SessionSnapshot, Tool, ToolDefinition, ToolExecutor, ToolInvocation, ToolKind, ToolResult,
    canonical_provider_id, resolve_provider_config,
};
use navi_openai::OpenAiProvider;
use navi_plugin_host::{LoadedPlugin, load_configured_plugins};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
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
    _plugins: Vec<LoadedPlugin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviSessionRequest {
    #[serde(default)]
    pub project_dir: Option<PathBuf>,
    #[serde(default)]
    pub agent_mode: Option<AgentMode>,
    #[serde(default)]
    pub context_packets: Vec<ContextPacket>,
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
        let provider = model_provider_for_config(&loaded_config)?;
        let security_policy = SecurityPolicy::new(
            project_dir.clone(),
            loaded_config.data_dir.clone(),
            loaded_config.config.security.clone(),
        )?;
        let mut tool_executor = ToolExecutor::new(security_policy.clone());
        for tool in &self.inner.host_tools {
            tool_executor.register_tool(tool.clone());
        }
        let plugin_report = load_configured_plugins(
            &loaded_config.config.plugins,
            &security_policy,
            &mut tool_executor,
        );
        for warning in &plugin_report.warnings {
            tracing::warn!(warning = %warning, "plugin load warning");
        }

        let mut runtime = AgentRuntime::new(AgentRuntimeOptions {
            loaded_config: loaded_config.clone(),
            model_provider: provider,
            project_dir: project_dir.clone(),
            tool_executor: Some(Arc::new(tool_executor)),
            agent_mode: request.agent_mode.or(self.inner.agent_mode),
            context_packets: request.context_packets,
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
                _plugins: plugin_report.loaded_plugins,
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

fn model_provider_for_config(loaded_config: &LoadedConfig) -> Result<Arc<dyn ModelProvider>> {
    let provider_config =
        resolve_provider_config(&loaded_config.config, &loaded_config.config.model.provider)
            .ok_or_else(|| {
                anyhow::anyhow!("unknown provider {}", loaded_config.config.model.provider)
            })?;

    Ok(Arc::new(OpenAiProvider::from_provider_config(
        &provider_config,
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
