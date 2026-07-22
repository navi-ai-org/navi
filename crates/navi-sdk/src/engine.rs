use crate::types::NaviError;
use anyhow::Context;
type Result<T> = std::result::Result<T, NaviError>;
use navi_core::registry::types::{RegistryModel, RegistryProvider};
use navi_core::{
    AgentRuntime, AgentRuntimeOptions, ApprovalDecision, CredentialStore, LoadedConfig,
    MemoryExtractionModel, ModelOption, ProviderConfig, QuestionResponse, RuntimeComponents,
    RuntimeEvent, SessionGoal, SessionId, SessionSnapshot, SessionStore, SessionTitleHandle,
    SessionTitleTool, SkillManifest, available_model_options, canonical_provider_id,
    config::effective_context_window, discover_configured_skills, model_can_run_publicly,
    provider_catalog, registry, registry::RegistryStore, resolve_provider_api_key,
    resolve_provider_config, resolve_provider_credential_status, save_global_config,
    save_project_config,
};
use navi_mcp::{LoadedMcpServers, McpServerInfo, load_configured_mcp_servers};

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tokio::sync::{Mutex as AsyncMutex, broadcast};

use crate::profiles::{
    NaviPromptProfile, NaviSecurityProfile, NaviToolProfile, ProfilePromptBuilder,
    filter_tool_names,
};
use crate::tooling::{
    build_local_tooling, build_provider_for_project_config, list_models_for_provider,
};
use crate::types::{
    NaviConfigSaveTarget, NaviModelInfo, NaviModelSelectionRequest, NaviModelSelectionResult,
    NaviProviderAccountInfo, NaviProviderCredentialStatus, NaviProviderSyncFailure,
    NaviProviderSyncReport, NaviProviderSyncSkipped, NaviProviderUpsertResult,
    NaviSavedSessionInfo, NaviSessionInfo, NaviSessionRequest, NaviSkillInfo, NaviSyncedProvider,
    NaviTurnRequest, NaviTurnResponse, NaviUsageDetail, NaviUsageLimitSnapshot, NaviUsageReport,
    NaviUsageWindow,
};

/// Builder for constructing a [`NaviEngine`] with custom configuration.
///
/// Use [`NaviEngineBuilder::from_project`] to start, then chain optional setters
/// before calling [`NaviEngineBuilder::build`].
///
/// ```rust,no_run
/// use navi_sdk::{NaviEngineBuilder, NaviEngine};
///
/// let engine = NaviEngineBuilder::from_project(".")
/// .build()
/// .expect("engine");
/// ```
#[derive(Clone)]
pub struct NaviEngineBuilder {
    project_dir: PathBuf,
    loaded_config: Option<LoadedConfig>,
    /// Optional durable data directory override (sessions, credentials, plugins).
    data_dir: Option<PathBuf>,
    host_tools: Vec<Arc<dyn navi_core::Tool>>,
    runtime_components: RuntimeComponents,
    tool_profile: NaviToolProfile,
    allow_tools: Vec<String>,
    deny_tools: Vec<String>,
    prompt_profile: NaviPromptProfile,
    security_profile: NaviSecurityProfile,
    /// Explicit permission mode override (applied after security profile).
    permission_mode: Option<navi_core::PermissionMode>,
    /// When true, a custom `prompt` component was set and profile should not replace it.
    prompt_overridden: bool,
}

impl NaviEngineBuilder {
    /// Creates a new builder rooted at the given project directory.
    ///
    /// Config will be loaded from this directory's `.navi/config.toml` (if present)
    /// and the global config on [`build`](Self::build), unless you override it with
    /// [`loaded_config`](Self::loaded_config).
    pub fn from_project(project_dir: impl Into<PathBuf>) -> Self {
        Self {
            project_dir: project_dir.into(),
            loaded_config: None,
            data_dir: None,
            host_tools: Vec::new(),
            runtime_components: RuntimeComponents::default(),
            tool_profile: NaviToolProfile::default(),
            allow_tools: Vec::new(),
            deny_tools: Vec::new(),
            prompt_profile: NaviPromptProfile::default(),
            security_profile: NaviSecurityProfile::default(),
            permission_mode: None,
            prompt_overridden: false,
        }
    }

    /// Overrides the loaded config instead of loading from the project directory.
    pub fn loaded_config(mut self, loaded_config: LoadedConfig) -> Self {
        self.loaded_config = Some(loaded_config);
        self
    }

    /// Points durable NAVI state (sessions, credentials, plugins, registry) at
    /// an app-controlled directory. Applied after config load / `loaded_config`.
    pub fn data_dir(mut self, data_dir: impl Into<PathBuf>) -> Self {
        self.data_dir = Some(data_dir.into());
        self
    }

    /// Selects which built-in tools the model may see.
    ///
    /// See [`NaviToolProfile`]: `code_agent` (default), `host_tools_only`, `chat_only`.
    pub fn tool_profile(mut self, profile: NaviToolProfile) -> Self {
        self.tool_profile = profile;
        self
    }

    /// Restrict offered tools to this allowlist (when non-empty), applied after the profile.
    pub fn allow_tools(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.allow_tools = names.into_iter().map(Into::into).collect();
        self
    }

    /// Always omit these tool names from the model schema.
    pub fn deny_tools(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.deny_tools = names.into_iter().map(Into::into).collect();
        self
    }

    /// Selects the base system prompt identity (`code_agent` or `assistant`).
    pub fn prompt_profile(mut self, profile: NaviPromptProfile) -> Self {
        self.prompt_profile = profile;
        self
    }

    /// Applies a host security posture (`code_agent` or `host_app`).
    ///
    /// `host_app` forces restricted permission mode so write tools stay approval-gated.
    pub fn security_profile(mut self, profile: NaviSecurityProfile) -> Self {
        self.security_profile = profile;
        self
    }

    /// Sets permission mode at build time (overrides profile default when set).
    pub fn permission_mode(mut self, mode: navi_core::PermissionMode) -> Self {
        self.permission_mode = Some(mode);
        self
    }

    /// Registers a host-provided tool that will be available in all sessions.
    pub fn host_tool(mut self, tool: Arc<dyn navi_core::Tool>) -> Self {
        self.host_tools.push(tool);
        self
    }

    /// Replaces all runtime components used by sessions created from this engine.
    pub fn runtime_components(mut self, runtime_components: RuntimeComponents) -> Self {
        self.runtime_components = runtime_components;
        self.prompt_overridden = true;
        self
    }

    /// Replaces the tool security policy component.
    pub fn security(mut self, security: Arc<dyn navi_core::ToolSecurityPolicy>) -> Self {
        self.runtime_components.security = security;
        self
    }

    /// Replaces the harness driver component.
    pub fn harness(mut self, harness: Arc<dyn navi_core::HarnessDriver>) -> Self {
        self.runtime_components.harness = harness;
        self
    }

    /// Replaces the system prompt builder component.
    pub fn prompt(mut self, prompt: Arc<dyn navi_core::PromptBuilder>) -> Self {
        self.runtime_components.prompt = prompt;
        self.prompt_overridden = true;
        self
    }

    /// Replaces the compaction strategy component.
    pub fn compaction(mut self, compaction: Arc<dyn navi_core::CompactionStrategy>) -> Self {
        self.runtime_components.compaction = compaction;
        self
    }

    /// Replaces lifecycle hooks for sessions, turns, and tool execution.
    pub fn hooks(mut self, hooks: Arc<dyn navi_core::SessionHooks>) -> Self {
        self.runtime_components.hooks = hooks;
        self
    }

    /// Builds the [`NaviEngine`]. Loads config from the project directory if not
    /// overridden via [`loaded_config`](Self::loaded_config).
    pub fn build(self) -> Result<NaviEngine> {
        let mut loaded_config = match self.loaded_config {
            Some(config) => config,
            None => navi_core::NaviConfig::load(&self.project_dir)?,
        };

        if let Some(data_dir) = self.data_dir {
            loaded_config.data_dir = data_dir;
        }

        self.security_profile
            .apply(&mut loaded_config.config.security);
        if let Some(mode) = self.permission_mode {
            loaded_config.config.security.permission_mode = mode;
        }

        let mut runtime_components = self.runtime_components;
        if !self.prompt_overridden {
            runtime_components.prompt = Arc::new(ProfilePromptBuilder::new(self.prompt_profile));
        }

        let host_tool_names: HashSet<String> = self
            .host_tools
            .iter()
            .map(|t| t.definition().name.clone())
            .collect();

        // Initialize the registry store, load the active registry, and set it as the
        // catalog source so provider_catalog() uses the cache or embedded snapshot.
        let registry_store = match RegistryStore::open(&loaded_config.data_dir) {
            Ok(store) => {
                let store = Arc::new(store);
                registry::load_registry(&store);
                navi_core::set_registry_store(store.clone());
                Some(store)
            }
            Err(err) => {
                tracing::warn!(error = %err, "failed to open registry store, using built-in providers");
                None
            }
        };

        // Spawn a background update check if enabled.
        if let Some(ref store) = registry_store {
            let config = loaded_config.config.registry.clone();
            if config.update_enabled && std::env::var("NAVI_NO_REGISTRY_UPDATE").is_err() {
                let store = store.clone();
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        if registry::should_check_registry_update(&store, &config) {
                            let fetcher = registry::RegistryFetcher::new();
                            registry::run_registry_update_check(&store, &fetcher, &config).await;
                        }
                    });
                }
            }
        }

        Ok(NaviEngine {
            inner: Arc::new(NaviEngineInner {
                project_dir: self.project_dir,
                loaded_config: RwLock::new(loaded_config),
                host_tools: self.host_tools,
                host_tool_names,
                tool_profile: self.tool_profile,
                allow_tools: self.allow_tools,
                deny_tools: self.deny_tools,
                prompt_profile: self.prompt_profile,
                security_profile: self.security_profile,
                runtime_components,
                sessions: RwLock::new(HashMap::new()),
                registry_store,
                voice: std::sync::Mutex::new(crate::voice::VoiceRuntime::new()),
            }),
        })
    }
}

fn runtime_components_for_plugin_policies(
    mut components: RuntimeComponents,
    agent_policies: &[String],
    warnings: &mut Vec<String>,
) -> RuntimeComponents {
    for policy in agent_policies {
        match normalize_plugin_policy_name(policy).as_str() {
            "default" | "code_agent" => {
                components = RuntimeComponents::default();
            }
            other => warnings.push(format!(
                "plugin registered unknown agent policy `{other}`; known policies are default and code_agent"
            )),
        }
    }
    components
}

fn normalize_plugin_policy_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace('-', "_")
}

/// Apply host tool profile + allow/deny lists to a session's tool executor.
fn apply_tool_profile_filter(
    executor: &mut navi_core::ToolExecutor,
    profile: NaviToolProfile,
    host_tool_names: &HashSet<String>,
    allow_tools: &[String],
    deny_tools: &[String],
) {
    let registered = executor.tool_names();
    let keep = filter_tool_names(
        &registered,
        profile,
        host_tool_names,
        allow_tools,
        deny_tools,
    );
    if keep.len() == registered.len() {
        let keep_set: HashSet<&str> = keep.iter().map(|s| s.as_str()).collect();
        if registered.iter().all(|n| keep_set.contains(n.as_str())) {
            return;
        }
    }
    let keep_set: HashSet<String> = keep.into_iter().collect();
    executor.retain_tools(|name| keep_set.contains(name));
}

/// The main NAVI engine handle. Clone-safe (wraps `Arc` internally).
///
/// Provides session lifecycle, model management, credential management,
/// skill discovery, and MCP server access. Create via [`NaviEngineBuilder`].
#[derive(Clone)]
pub struct NaviEngine {
    pub(crate) inner: Arc<NaviEngineInner>,
}

pub(crate) struct NaviEngineInner {
    pub(crate) project_dir: PathBuf,
    loaded_config: RwLock<LoadedConfig>,
    host_tools: Vec<Arc<dyn navi_core::Tool>>,
    host_tool_names: HashSet<String>,
    tool_profile: NaviToolProfile,
    allow_tools: Vec<String>,
    deny_tools: Vec<String>,
    prompt_profile: NaviPromptProfile,
    security_profile: NaviSecurityProfile,
    runtime_components: RuntimeComponents,
    sessions: RwLock<HashMap<String, Arc<NaviSession>>>,
    registry_store: Option<Arc<RegistryStore>>,
    /// Local dictation (ONNX). Engine-scoped, not per-session.
    pub(crate) voice: std::sync::Mutex<crate::voice::VoiceRuntime>,
}

/// An active NAVI session with its own runtime, event stream, and approval handles.
pub struct NaviSession {
    runtime: AsyncMutex<AgentRuntime>,
    events: broadcast::Receiver<RuntimeEvent>,
    approval_resolver: navi_core::ApprovalResolver,
    question_resolver: navi_core::QuestionResolver,
    plan_review_resolver: navi_core::PlanReviewResolver,
    sudo_password_resolver: navi_core::SudoPasswordResolver,
    turn_canceller: navi_core::TurnCanceller,
    tui_components: Vec<String>,
    tui_panels: std::sync::Mutex<Vec<Box<dyn navi_plugin_api::TuiComponent>>>,
    mcp: LoadedMcpServers,
}

impl NaviEngine {
    /// Starts a new agent session with the given model, context packets, and skills.
    ///
    /// Returns a [`NaviSessionInfo`] with the session ID and model details.
    /// The session can then receive turns via [`send_turn`](Self::send_turn).
    ///
    /// If a session with the same ID already exists, returns the existing session
    /// info without recreating. This preserves background command state across turns.
    pub async fn start_session(&self, request: NaviSessionRequest) -> Result<NaviSessionInfo> {
        let project_dir = request
            .project_dir
            .clone()
            .unwrap_or_else(|| self.inner.project_dir.clone());

        // If a session with this ID already exists, return it without recreating.
        if let Some(session_id) = &request.session_id {
            let existing_session = {
                let sessions = self
                    .inner
                    .sessions
                    .read()
                    .unwrap_or_else(|e| e.into_inner());
                sessions.get(session_id).cloned()
            };
            if let Some(session) = existing_session {
                let mut runtime = session.runtime.lock().await;
                runtime.set_active_skills(request.active_skills.clone());
                let loaded_config = self.loaded_config();
                // Keep the live runtime on the engine's current model/provider.
                // Without this, `select_model` (or a failed TUI rebuild) can leave
                // the session on the previous model while the footer shows the new
                // one — next turn may 400/empty ("No response.") on the old wire.
                let (runtime_provider, runtime_model) = runtime.model_selection();
                let desired_provider = loaded_config.config.model.provider.as_str();
                let desired_model = loaded_config.config.model.name.as_str();
                if runtime_provider != desired_provider || runtime_model != desired_model {
                    match build_provider_for_project_config(&loaded_config, &project_dir) {
                        Ok(model_provider) => {
                            runtime.set_model_provider(loaded_config.clone(), model_provider);
                        }
                        Err(err) => {
                            tracing::warn!(
                                error = %err,
                                from_provider = %runtime_provider,
                                from_model = %runtime_model,
                                to_provider = %desired_provider,
                                to_model = %desired_model,
                                "failed to sync model onto existing session; turn may use stale provider"
                            );
                        }
                    }
                }
                return Ok(NaviSessionInfo {
                    id: session_id.clone(),
                    project_dir,
                    model: loaded_config.config.model.name.clone(),
                    provider: loaded_config.config.model.provider.clone(),
                });
            }
        }

        let loaded_config = self.loaded_config();
        let provider = build_provider_for_project_config(&loaded_config, &project_dir)?;
        let memory_extraction_model =
            self.configured_memory_extraction_model(&loaded_config, &project_dir)?;
        let mut tool_executor = build_local_tooling(
            &loaded_config,
            project_dir.clone(),
            &self.inner.runtime_components,
        )?;
        let runtime_components = runtime_components_for_plugin_policies(
            self.inner.runtime_components.clone(),
            &tool_executor.agent_policies,
            &mut tool_executor.warnings,
        );
        for tool in &self.inner.host_tools {
            let executor = Arc::get_mut(&mut tool_executor.tool_executor).ok_or_else(|| {
                NaviError::Config("cannot register host tool after tool executor is shared".into())
            })?;
            executor.register_tool(tool.clone());
        }
        let mcp = load_configured_mcp_servers(
            &loaded_config.config.mcp,
            &loaded_config.config.security.allowed_mcp_servers,
        )
        .await;
        for tool in &mcp.tools {
            let executor = Arc::get_mut(&mut tool_executor.tool_executor).ok_or_else(|| {
                NaviError::Config("cannot register MCP tool after tool executor is shared".into())
            })?;
            executor.register_tool(tool.clone());
        }
        {
            let executor = Arc::get_mut(&mut tool_executor.tool_executor).ok_or_else(|| {
                NaviError::Config(
                    "cannot register attachment analysis tool after tool executor is shared".into(),
                )
            })?;
            executor.register_tool(Arc::new(
                crate::attachment_tool::AttachmentAnalysisTool::new(
                    loaded_config.clone(),
                    project_dir.clone(),
                ),
            ));
        }
        for warning in &tool_executor.warnings {
            tracing::warn!(warning = %warning, "plugin load warning");
        }

        // Register tools that need a weak reference back to the executor.
        let mut executor = Arc::try_unwrap(tool_executor.tool_executor).map_err(|_| {
            NaviError::Config("cannot finalize cyclic tools after executor is shared".into())
        })?;
        // The active chat model names the session through this cheap local tool;
        // never create a second background completion merely to generate a title.
        let session_title_handle = SessionTitleHandle::new();
        let install_code_agent_extras =
            matches!(self.inner.tool_profile, NaviToolProfile::CodeAgent)
                && self.inner.allow_tools.is_empty();

        // Skip title/skill tooling when the host does not want a code-agent surface.
        // (AgentRuntime may re-register some of these; we re-filter after start.)
        if install_code_agent_extras {
            executor.register_tool(Arc::new(SessionTitleTool::new(
                session_title_handle.clone(),
            )));
        }
        let shared_provider = Arc::new(std::sync::RwLock::new(provider.clone()));
        let shared_model = Arc::new(std::sync::RwLock::new(
            loaded_config.config.model.name.clone(),
        ));
        let shared_config = Arc::new(std::sync::RwLock::new(loaded_config.config.clone()));
        let prompt_cache = Arc::new(navi_core::PromptCache::new());
        if install_code_agent_extras {
            executor.register_skill_loader(
                project_dir.clone(),
                loaded_config.data_dir.clone(),
                shared_config.clone(),
            );
        }

        // Apply host tool profile while we uniquely own the executor (before Arc).
        apply_tool_profile_filter(
            &mut executor,
            self.inner.tool_profile,
            &self.inner.host_tool_names,
            &self.inner.allow_tools,
            &self.inner.deny_tools,
        );

        tool_executor.tool_executor = if install_code_agent_extras {
            Arc::new_cyclic(|weak_exec| {
                let subagent = navi_core::SubagentTool::new(
                    weak_exec.clone(),
                    shared_provider.clone(),
                    project_dir.clone(),
                    loaded_config.data_dir.clone(),
                    shared_model.clone(),
                    loaded_config.config.harness.clone(),
                    shared_config.clone(),
                    prompt_cache.clone(),
                    runtime_components.clone(),
                );
                executor.register_tool(Arc::new(subagent));
                // Deterministic BM25+symbol search — no nested model turn.
                executor.register_tool(Arc::new(navi_core::RepoExploreTool::new(
                    project_dir.clone(),
                )));
                // Workflow multi-agent orchestration via real nested subagent turns.
                let workflow_policy = navi_core::SecurityPolicy::new(
                    project_dir.clone(),
                    loaded_config.data_dir.clone(),
                    loaded_config.config.security.clone(),
                )
                .unwrap_or_else(|_| {
                    navi_core::SecurityPolicy::new(
                        project_dir.clone(),
                        loaded_config.data_dir.clone(),
                        navi_core::SecurityConfig::default(),
                    )
                    .expect("security policy")
                });
                executor.register_tool(Arc::new(navi_core::WorkflowTool::with_subagent_bridge(
                    workflow_policy,
                    loaded_config.config.workflow.clone(),
                    weak_exec.clone(),
                )));
                executor
            })
        } else {
            Arc::new(executor)
        };

        let runtime_tool_executor = tool_executor.tool_executor;
        let tui_components = tool_executor.tui_components;
        let tui_panels = tool_executor.tui_panels;
        let mut runtime = AgentRuntime::new(AgentRuntimeOptions {
            loaded_config: loaded_config.clone(),
            model_provider: provider,
            project_dir: project_dir.clone(),
            tool_executor: Some(runtime_tool_executor),
            context_packets: request.context_packets,
            active_skills: request.active_skills,
            initial_messages: request.initial_messages,
            initial_events: request.initial_events,
            initial_created_at: request.initial_created_at,
            initial_updated_at: request.initial_updated_at,
            initial_goal: request.initial_goal,
            session_id: request.session_id.map(SessionId::new),
            event_tx: None,
            runtime_components: Some(runtime_components),
            session_title_handle: Some(session_title_handle),
            memory_extraction_model,
            skip_auto_tool_bootstrap: !install_code_agent_extras,
        });
        let events = runtime.stream_events();
        let session_id = runtime.start_session()?;
        // AgentRuntime may re-register goal/skill tools on start; re-apply host
        // tool profile so chat_only / host_tools_only stay effective.
        if let Some(exec) = runtime.tool_executor() {
            let registered = exec.tool_names();
            let keep = filter_tool_names(
                &registered,
                self.inner.tool_profile,
                &self.inner.host_tool_names,
                &self.inner.allow_tools,
                &self.inner.deny_tools,
            );
            let keep_set: HashSet<String> = keep.into_iter().collect();
            drop(exec);
            runtime.retain_tools(|name| keep_set.contains(name));
        }
        let approval_resolver = runtime.approval_resolver();
        let question_resolver = runtime.question_resolver();
        let plan_review_resolver = runtime.plan_review_resolver();
        let sudo_password_resolver = runtime.sudo_password_resolver();
        let turn_canceller = runtime.turn_canceller();
        let info = NaviSessionInfo {
            id: session_id.as_str().to_string(),
            project_dir,
            model: loaded_config.config.model.name.clone(),
            provider: loaded_config.config.model.provider.clone(),
        };
        self.inner
            .sessions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                session_id.into_inner(),
                Arc::new(NaviSession {
                    runtime: AsyncMutex::new(runtime),
                    events,
                    approval_resolver,
                    question_resolver,
                    plan_review_resolver,
                    sudo_password_resolver,
                    turn_canceller,
                    tui_components,
                    tui_panels: std::sync::Mutex::new(tui_panels),
                    mcp,
                }),
            );
        Ok(info)
    }

    /// Builds the opt-in model used for automatic memory extraction. This is
    /// intentionally separate from the chat provider: leaving it unset
    /// disables automatic extraction instead of charging the interactive model
    /// in a hidden background request.
    fn configured_memory_extraction_model(
        &self,
        loaded_config: &LoadedConfig,
        project_dir: &std::path::Path,
    ) -> Result<Option<MemoryExtractionModel>> {
        let Some(entry) = loaded_config
            .config
            .background_models
            .memory_extraction
            .as_ref()
        else {
            return Ok(None);
        };
        let (Some(provider), Some(model)) = (entry.provider.as_deref(), entry.model.as_deref())
        else {
            return Err(NaviError::Config(
                "background_models.memory_extraction requires an explicit provider and model"
                    .into(),
            ));
        };

        let mut extraction_config = loaded_config.clone();
        extraction_config.config.model.provider = provider.to_string();
        extraction_config.config.model.name = model.to_string();
        let provider = build_provider_for_project_config(&extraction_config, project_dir)?;
        Ok(Some(MemoryExtractionModel {
            provider,
            model_name: model.to_string(),
        }))
    }

    /// Sends a user message to an active session and waits for the assistant response.
    ///
    /// Returns the final text response. For streaming events, use
    /// [`subscribe_events`](Self::subscribe_events) before calling this.
    pub async fn send_turn(&self, request: NaviTurnRequest) -> Result<NaviTurnResponse> {
        let session = self.session(&request.session_id)?;
        let mut runtime = session.runtime.lock().await;
        for packet in request.context_packets {
            runtime.add_context_packet(packet);
        }
        // Goal auto-continuation runs inside AgentRuntime::send_turn_with_parts
        // (thread-idle lifecycle) so every host gets the same behavior.
        let response = runtime
            .send_turn_with_parts(request.message, request.content_parts, request.thinking)
            .await?;
        Ok(NaviTurnResponse {
            session_id: request.session_id,
            text: response.text,
        })
    }

    /// Returns the current goal for a session.
    pub async fn get_goal(&self, session_id: &str) -> Result<Option<SessionGoal>> {
        let session = self.session(session_id)?;
        let runtime = session.runtime.lock().await;
        Ok(runtime.get_goal())
    }

    /// Sets a goal for a session. The goal will guide the agent across turns
    /// (auto-continuation while status is Active).
    pub async fn set_goal(
        &self,
        session_id: &str,
        objective: impl Into<String>,
        token_budget: Option<i64>,
    ) -> Result<SessionGoal> {
        let session = self.session(session_id)?;
        let runtime = session.runtime.lock().await;
        // set_goal publishes GoalUpdated for live clients.
        Ok(runtime.set_goal(objective.into(), token_budget))
    }

    /// Clears the goal for a session (notifies live clients).
    pub async fn clear_goal(&self, session_id: &str) -> Result<()> {
        let session = self.session(session_id)?;
        let runtime = session.runtime.lock().await;
        runtime.clear_goal();
        Ok(())
    }

    /// Updates the goal status (e.g. pause, resume, complete, blocked).
    pub async fn update_goal_status(
        &self,
        session_id: &str,
        status: navi_core::GoalStatus,
    ) -> Result<Option<SessionGoal>> {
        let session = self.session(session_id)?;
        let runtime = session.runtime.lock().await;
        if let Some(mut goal) = runtime.get_goal() {
            goal.transition_to(status);
            if status == navi_core::GoalStatus::Paused {
                runtime.goal_runtime().set_auto_continue(false);
            } else if status == navi_core::GoalStatus::Active {
                runtime.goal_runtime().set_auto_continue(true);
            }
            // update_goal publishes GoalUpdated for live clients.
            runtime.update_goal(goal.clone());
            Ok(Some(goal))
        } else {
            Ok(None)
        }
    }

    /// Replaces the goal's checklist with a new set of tasks.
    pub async fn update_goal_checklist(
        &self,
        session_id: &str,
        tasks: Vec<navi_core::GoalTask>,
    ) -> Result<Option<SessionGoal>> {
        let session = self.session(session_id)?;
        let runtime = session.runtime.lock().await;
        Ok(runtime.update_goal_checklist(tasks))
    }

    /// Updates a single task's status in the goal checklist.
    pub async fn update_goal_task_status(
        &self,
        session_id: &str,
        task_id: usize,
        status: navi_core::TaskStatus,
    ) -> Result<Option<SessionGoal>> {
        let session = self.session(session_id)?;
        let runtime = session.runtime.lock().await;
        Ok(runtime.update_goal_task_status(task_id, status))
    }

    /// Cancels the currently active turn for the given session.
    pub async fn cancel_turn(&self, session_id: &str) -> Result<()> {
        let session = self.session(session_id)?;
        session.turn_canceller.cancel();
        Ok(())
    }

    /// Rewind a live session so later turns are forgotten (edit-message support).
    ///
    /// Keeps the first `keep_user_turns` user turns and drops everything after.
    /// The caller should then `send_turn` with the replacement user text.
    /// Returns how many model messages remain in the live history.
    pub async fn rewind_session(&self, session_id: &str, keep_user_turns: usize) -> Result<usize> {
        let session = self.session(session_id)?;
        // Cancel any in-flight turn so the session loop is free to process rewind.
        session.turn_canceller.cancel();
        let mut runtime = session.runtime.lock().await;
        runtime
            .rewind_to_user_turns(keep_user_turns)
            .await
            .map_err(NaviError::from)
    }

    /// Force-compact a session's conversation history using the session's own model.
    ///
    /// Replaces older turns with a model-produced summary and emits compact events.
    /// Does not use a subagent or background model route.
    pub async fn compact_session(&self, session_id: &str) -> Result<navi_core::CompactOutcome> {
        let session = self.session(session_id)?;
        // Cancel any in-flight turn so the session loop can run compact.
        session.turn_canceller.cancel();
        let mut runtime = session.runtime.lock().await;
        runtime.compact_now().await.map_err(NaviError::from)
    }

    /// Tool names registered for an active session after profile filtering.
    pub async fn list_session_tools(&self, session_id: &str) -> Result<Vec<String>> {
        let session = self.session(session_id)?;
        let runtime = session.runtime.lock().await;
        let Some(executor) = runtime.tool_executor() else {
            return Ok(Vec::new());
        };
        Ok(executor.tool_names())
    }

    /// Active host tool profile for this engine.
    pub fn tool_profile(&self) -> NaviToolProfile {
        self.inner.tool_profile
    }

    /// Active prompt profile for this engine.
    pub fn prompt_profile(&self) -> NaviPromptProfile {
        self.inner.prompt_profile
    }

    /// Active security profile for this engine.
    pub fn security_profile(&self) -> NaviSecurityProfile {
        self.inner.security_profile
    }

    /// Reopen a saved session snapshot with full provider history.
    ///
    /// Prefer this over hand-rolling `initial_messages` / `initial_events`.
    /// Attachment bytes are rehydrated from the project path or `{data_dir}/attachments/`.
    pub async fn start_session_from_snapshot(
        &self,
        snapshot: &SessionSnapshot,
    ) -> Result<NaviSessionInfo> {
        let data_dir = self.loaded_config().data_dir;
        let request = crate::session_request_from_snapshot(snapshot, Some(data_dir.as_path()));
        self.start_session(request).await
    }

    /// Insert or update a custom OpenAI-compatible (or other) provider in config.
    ///
    /// Unreachable base URLs do not crash build or list; credential resolution
    /// and model selection still work offline.
    pub fn upsert_provider(
        &self,
        provider: ProviderConfig,
        save_target: NaviConfigSaveTarget,
    ) -> Result<NaviProviderUpsertResult> {
        if provider.id.trim().is_empty() {
            return Err(NaviError::Config("provider id must not be empty".into()));
        }
        let mut loaded_config = self.loaded_config();
        let id = provider.id.clone();
        if let Some(existing) = loaded_config
            .config
            .providers
            .iter_mut()
            .find(|p| p.id == id)
        {
            *existing = provider;
        } else {
            loaded_config.config.providers.push(provider);
        }
        let saved_to = self.save_loaded_config(&loaded_config, save_target)?;
        self.replace_loaded_config(loaded_config.clone());
        Ok(NaviProviderUpsertResult {
            provider_id: id,
            loaded_config,
            saved_to,
        })
    }

    /// Returns the current agent mode (Default or Plan).
    pub fn agent_mode(&self, session_id: &str) -> Result<navi_core::plan_mode::AgentMode> {
        let session = self.session(session_id)?;
        let runtime = session.runtime.try_lock();
        Ok(runtime
            .map(|r| r.agent_mode())
            .unwrap_or(navi_core::plan_mode::AgentMode::Default))
    }

    /// Enters Plan mode for the given session.
    pub async fn enter_plan_mode(&self, session_id: &str) -> Result<()> {
        let session = self.session(session_id)?;
        let runtime = session.runtime.lock().await;
        runtime.enter_plan_mode();
        Ok(())
    }

    /// Exits Plan mode and returns to normal execution.
    pub async fn exit_plan_mode(&self, session_id: &str) -> Result<()> {
        let session = self.session(session_id)?;
        let runtime = session.runtime.lock().await;
        runtime.exit_plan_mode();
        Ok(())
    }

    /// Resolves a pending tool approval request. Returns `true` if the approval was
    /// consumed, `false` if there was no pending request.
    pub async fn resolve_approval(
        &self,
        session_id: &str,
        decision: ApprovalDecision,
    ) -> Result<bool> {
        let session = self.session(session_id)?;
        Ok(session.approval_resolver.resolve(decision))
    }

    /// Resolves a pending interactive question. Returns `true` if the response was
    /// consumed, `false` if there was no pending request.
    pub async fn resolve_question(
        &self,
        session_id: &str,
        response: QuestionResponse,
    ) -> Result<bool> {
        let session = self.session(session_id)?;
        Ok(session.question_resolver.resolve(response))
    }

    /// Resolves a pending plan review (unblocks the plan tool / turn).
    pub async fn resolve_plan_review(
        &self,
        session_id: &str,
        response: navi_core::event::PlanReviewResponse,
    ) -> Result<bool> {
        let session = self.session(session_id)?;
        Ok(session.plan_review_resolver.resolve(response))
    }

    /// Resolves a sudo password prompt. The password never enters chat history.
    pub async fn resolve_sudo_password(
        &self,
        session_id: &str,
        response: navi_core::event::SudoPasswordResponse,
    ) -> Result<bool> {
        let session = self.session(session_id)?;
        Ok(session.sudo_password_resolver.resolve(response))
    }

    /// Adds a context packet (file, selection, memory, etc.) to an active session.
    pub async fn add_context_packet(
        &self,
        session_id: &str,
        packet: navi_core::ContextPacket,
    ) -> Result<()> {
        let session = self.session(session_id)?;
        let mut runtime = session.runtime.lock().await;
        runtime.add_context_packet(packet);
        Ok(())
    }

    /// Takes a point-in-time snapshot of the session state for persistence.
    pub async fn snapshot_session(&self, session_id: &str) -> Result<SessionSnapshot> {
        let session = self.session(session_id)?;
        let mut runtime = session.runtime.lock().await;
        Ok(runtime.snapshot_session_async().await?)
    }

    /// Changes the model used by an active session.
    pub async fn set_model(&self, session_id: &str, provider: &str, model: &str) -> Result<()> {
        let session = self.session(session_id)?;
        let mut loaded_config = self.loaded_config();
        let provider_config = resolve_provider_config(&loaded_config.config, provider)
            .with_context(|| format!("unknown provider {provider}"))?;
        loaded_config.config.model.provider = provider_config.id.clone();
        loaded_config.config.model.name = model.to_string();
        let model_provider =
            build_provider_for_project_config(&loaded_config, &self.inner.project_dir)?;

        {
            let mut runtime = session.runtime.lock().await;
            runtime.set_model_provider(loaded_config.clone(), model_provider);
        }
        Ok(())
    }

    /// Returns the current permission mode for tool execution.
    pub fn get_permission_mode(&self) -> navi_core::PermissionMode {
        self.loaded_config()
            .config
            .effective_security_config()
            .permission_mode
    }

    /// Sets the permission mode for tool execution.
    ///
    /// Updates the engine config and every active session's tool security
    /// policy so the change takes effect without restarting sessions.
    pub async fn set_permission_mode(&self, mode: navi_core::PermissionMode) -> Result<()> {
        let security = {
            let mut config = self
                .inner
                .loaded_config
                .write()
                .unwrap_or_else(|e| e.into_inner());
            config.config.security.permission_mode = mode;
            config.config.tui.yolo_mode = matches!(mode, navi_core::PermissionMode::Yolo);
            config.config.effective_security_config()
        };

        let sessions = self
            .inner
            .sessions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();

        for session in sessions {
            let mut runtime = session.runtime.lock().await;
            runtime
                .set_security_config(security.clone())
                .map_err(|err| NaviError::Config(err.to_string()))?;
        }
        Ok(())
    }

    /// Closes an active in-memory session. Returns `true` when a session was removed.
    ///
    /// This does not delete persisted session history. Any active turn is asked to
    /// cancel before the session handle is dropped.
    pub async fn close_session(&self, session_id: &str) -> Result<bool> {
        let removed = self
            .inner
            .sessions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session_id);
        if let Some(session) = &removed {
            session.turn_canceller.cancel();
        }
        Ok(removed.is_some())
    }

    /// Lists all active background bash commands for a session.
    pub async fn list_background_commands(
        &self,
        session_id: &str,
    ) -> Result<Vec<navi_core::BackgroundCommandSnapshot>> {
        let session = self.session(session_id)?;
        let runtime = session.runtime.lock().await;
        let Some(executor) = runtime.tool_executor() else {
            return Ok(Vec::new());
        };
        Ok(executor.list_background_commands().await)
    }

    /// Polls a specific background bash command for a session.
    pub async fn poll_background_command(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<navi_core::BackgroundCommandSnapshot> {
        let session = self.session(session_id)?;
        let runtime = session.runtime.lock().await;
        let Some(executor) = runtime.tool_executor() else {
            return Err(NaviError::Config("tool executor unavailable".into()));
        };
        executor
            .poll_background_command(task_id)
            .await
            .ok_or_else(|| NaviError::Config(format!("background task `{task_id}` not found")))
    }

    /// Cancels a specific background bash command for a session.
    pub async fn cancel_background_command(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<navi_core::BackgroundCommandSnapshot> {
        let session = self.session(session_id)?;
        let runtime = session.runtime.lock().await;
        let Some(executor) = runtime.tool_executor() else {
            return Err(NaviError::Config("tool executor unavailable".into()));
        };
        executor
            .cancel_background_command(task_id)
            .await
            .ok_or_else(|| NaviError::Config(format!("background task `{task_id}` not found")))
    }

    /// Lists all available models across configured providers.
    pub fn list_models(&self) -> Vec<NaviModelInfo> {
        let loaded_config = self.loaded_config();
        available_model_options(&loaded_config.config)
            .into_iter()
            .map(model_info_from_option)
            .collect()
    }

    /// Lists all configured providers with their credential status.
    pub fn list_provider_accounts(&self) -> Result<Vec<NaviProviderAccountInfo>> {
        let loaded_config = self.loaded_config();
        let credential_store = self.credential_store();
        provider_catalog(&loaded_config.config)
            .into_iter()
            .map(|provider| {
                let status = self.provider_credential_status_for(
                    &loaded_config,
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
            .collect::<Result<Vec<_>>>()
    }

    /// Fetches account usage / rate-limit windows for the selected provider.
    ///
    /// Always returns a report when credentials exist (or when the provider has
    /// no remote usage API). Session/context usage is layered on by the TUI.
    pub async fn usage_report(&self) -> Result<NaviUsageReport> {
        let loaded_config = self.loaded_config();
        let provider_id = loaded_config.config.model.provider.as_str();
        let canonical = canonical_provider_id(provider_id);
        let provider = resolve_provider_config(&loaded_config.config, provider_id)
            .with_context(|| format!("unknown provider {provider_id}"))?;
        let credential_store = self.credential_store();

        // Prefer any stored credential (including OAuth tokens that are not
        // usable as model keys, e.g. ChatGPT OAuth).
        let access_token = credential_store
            .get_api_key(provider_id)
            .or_else(|| resolve_provider_api_key(&credential_store, &provider, provider_id));

        match canonical {
            "openai" => {
                let Some(token) = access_token else {
                    return Ok(empty_usage_report(
                        &provider,
                        "session",
                        Some("No OpenAI credential configured. Add an API key or sign in with OAuth.".into()),
                    ));
                };
                let oauth_kind = credential_store.get_oauth_api_kind(provider_id);
                // ChatGPT usage windows require a browser-OAuth access token,
                // not a Platform `sk-…` API key.
                if oauth_kind.is_some() || !token.starts_with("sk-") {
                    match navi_providers::openai_usage_report(&token).await {
                        Ok(report) => Ok(NaviUsageReport {
                            provider_id: provider.id.clone(),
                            provider_label: provider.label.clone(),
                            plan_type: report.plan_type,
                            limit_reached_kind: report.limit_reached_kind,
                            limits: report
                                .limits
                                .into_iter()
                                .map(openai_usage_limit_snapshot_to_sdk)
                                .collect(),
                            source: "openai-oauth".into(),
                            notes: Some("ChatGPT usage windows (OAuth).".into()),
                            details: Vec::new(),
                        }),
                        Err(err) => Ok(empty_usage_report(
                            &provider,
                            "openai-oauth-error",
                            Some(format!("Account usage unavailable: {err}")),
                        )),
                    }
                } else {
                    Ok(empty_usage_report(
                        &provider,
                        "openai-api-key",
                        Some(
                            "Platform API keys do not expose ChatGPT usage windows. Sign in with OpenAI OAuth for rate-limit bars, or check platform.openai.com usage.".into(),
                        ),
                    ))
                }
            }
            "xai" => {
                let Some(token) = access_token else {
                    return Ok(empty_usage_report(
                        &provider,
                        "session",
                        Some("No xAI credential configured.".into()),
                    ));
                };
                if navi_providers::is_xai_oauth_access_token(&token)
                    || credential_store.get_oauth_api_kind(provider_id).as_deref()
                        == Some(navi_core::XAI_GROK_CLI_OAUTH_KIND)
                {
                    match navi_providers::xai_usage_report(&token).await {
                        Ok(report) => Ok(xai_report_to_sdk(&provider, report)),
                        Err(err) => Ok(empty_usage_report(
                            &provider,
                            "xai-oauth-error",
                            Some(format!("xAI billing unavailable: {err}")),
                        )),
                    }
                } else {
                    Ok(empty_usage_report(
                        &provider,
                        "xai-api-key",
                        Some(
                            "Platform XAI_API_KEY has no public usage endpoint. Sign in with xAI OAuth for weekly credit usage, or check console.x.ai.".into(),
                        ),
                    ))
                }
            }
            "openrouter" => {
                let Some(token) = access_token else {
                    return Ok(empty_usage_report(
                        &provider,
                        "session",
                        Some("No OpenRouter credential configured.".into()),
                    ));
                };
                match navi_providers::openrouter_usage_report(&token).await {
                    Ok(report) => Ok(openrouter_report_to_sdk(&provider, report)),
                    Err(err) => Ok(empty_usage_report(
                        &provider,
                        "openrouter-error",
                        Some(format!("OpenRouter usage unavailable: {err}")),
                    )),
                }
            }
            "commandcode" => {
                let Some(token) = access_token else {
                    return Ok(empty_usage_report(
                        &provider,
                        "session",
                        Some("No Command Code credential configured.".into()),
                    ));
                };
                match navi_providers::commandcode_fetch_usage_data(&token).await {
                    Ok(data) => Ok(commandcode_report_to_sdk(&provider, data)),
                    Err(err) => Ok(empty_usage_report(
                        &provider,
                        "commandcode-error",
                        Some(format!("Command Code usage unavailable: {err}")),
                    )),
                }
            }
            "charm-hyper" => {
                let Some(token) = access_token else {
                    return Ok(empty_usage_report(
                        &provider,
                        "session",
                        Some("No Charm Hyper credential configured.".into()),
                    ));
                };
                match navi_providers::charm_hyper_credits_report(&token).await {
                    Ok(report) => Ok(charm_hyper_report_to_sdk(&provider, report)),
                    Err(err) => Ok(empty_usage_report(
                        &provider,
                        "charm-hyper-error",
                        Some(format!("Charm Hyper credits unavailable: {err}")),
                    )),
                }
            }
            _ => Ok(empty_usage_report(
                &provider,
                "session",
                Some(format!(
                    "{} has no remote usage API in NAVI yet — showing session/context usage only.",
                    provider.label
                )),
            )),
        }
    }

    /// Returns the credential status for a specific provider.
    pub fn credential_status(&self, provider_id: &str) -> Result<NaviProviderCredentialStatus> {
        let loaded_config = self.loaded_config();
        self.provider_credential_status_for(
            &loaded_config,
            &self.credential_store(),
            provider_id,
            None,
        )
    }

    /// Stores an API key for the given provider in the credential store.
    ///
    /// Replaces the provider's accounts with a single default account.
    /// Prefer [`Self::add_provider_account`] for multi-account setups.
    pub fn set_provider_api_key(&self, provider_id: &str, api_key: &str) -> Result<()> {
        Ok(self.credential_store().set_api_key(provider_id, api_key)?)
    }

    /// Deletes a stored API key. Returns `true` if a key was removed.
    pub fn delete_provider_api_key(&self, provider_id: &str) -> Result<bool> {
        Ok(self.credential_store().delete_api_key(provider_id)?)
    }

    /// List multi-account credentials for a provider.
    pub fn list_credential_accounts(
        &self,
        provider_id: &str,
    ) -> Result<Vec<navi_core::CredentialAccountInfo>> {
        Ok(self
            .credential_store()
            .list_credential_accounts(provider_id, Some(self.inner.project_dir.as_path()))?)
    }

    /// Add an API-key account without wiping sibling accounts. Returns account id.
    pub fn add_provider_account(
        &self,
        provider_id: &str,
        api_key: &str,
        label: Option<&str>,
    ) -> Result<String> {
        Ok(self
            .credential_store()
            .add_api_key_account(provider_id, api_key, label, None)?)
    }

    /// Select which account is active (default + project binding).
    pub fn select_provider_account(&self, provider_id: &str, account_id: &str) -> Result<()> {
        let store = self.credential_store();
        store.set_default_account(provider_id, account_id)?;
        store.set_project_account(&self.inner.project_dir, provider_id, account_id)?;
        Ok(())
    }

    /// Delete one credential account. Returns true if removed.
    pub fn delete_provider_account(&self, provider_id: &str, account_id: &str) -> Result<bool> {
        Ok(self
            .credential_store()
            .delete_credential_account(provider_id, account_id)?)
    }

    /// Lists skills from the SQLite store plus built-ins.
    pub fn list_skills(&self) -> Result<Vec<NaviSkillInfo>> {
        let loaded_config = self.loaded_config();
        Ok(discover_configured_skills(
            &loaded_config.config.skills,
            &self.inner.project_dir,
            &loaded_config.data_dir,
        )?
        .into_iter()
        .map(|m| {
            skill_info_from_manifest(m, &self.inner.project_dir, &loaded_config.data_dir, false)
        })
        .collect())
    }

    /// Load one skill including its full instruction body.
    pub fn get_skill(&self, skill_id: &str) -> Result<NaviSkillInfo> {
        let loaded_config = self.loaded_config();
        let manifest = navi_core::load_skill_by_id(
            &loaded_config.config.skills,
            &self.inner.project_dir,
            &loaded_config.data_dir,
            skill_id,
        )?;
        Ok(skill_info_from_manifest(
            manifest,
            &self.inner.project_dir,
            &loaded_config.data_dir,
            true,
        ))
    }

    /// Create or update a skill in the SQLite store (`data_dir/skills.sqlite`).
    ///
    /// Shared by TUI and Desktop. User scope is global; project scope is keyed
    /// by the engine project directory.
    pub fn save_skill(
        &self,
        request: navi_core::SkillWriteRequest,
    ) -> Result<navi_core::SkillWriteResult> {
        let loaded_config = self.loaded_config();
        navi_core::write_skill(&request, &self.inner.project_dir, &loaded_config.data_dir)
            .map_err(NaviError::from)
    }

    /// Delete a user- or project-authored skill. Returns whether something was removed.
    pub fn delete_skill(&self, skill_id: &str) -> Result<bool> {
        let loaded_config = self.loaded_config();
        navi_core::delete_skill(skill_id, &self.inner.project_dir, &loaded_config.data_dir)
            .map_err(NaviError::from)
    }

    /// Sets the active skills for an existing session.
    pub async fn set_session_skills(&self, session_id: &str, skills: Vec<String>) -> Result<()> {
        let session = self.session(session_id)?;
        let mut runtime = session.runtime.lock().await;
        runtime.set_active_skills(skills);
        Ok(())
    }

    /// Lists MCP servers connected to the given session.
    pub fn list_mcp_servers(&self, session_id: &str) -> Result<Vec<McpServerInfo>> {
        let session = self.session(session_id)?;
        Ok(session.mcp.servers.clone())
    }

    /// Lists tool names provided by MCP servers in the given session.
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

    /// Subscribes to the event stream for a session. Events include assistant deltas,
    /// tool calls, approval requests, and completion signals.
    pub fn subscribe_events(&self, session_id: &str) -> Result<broadcast::Receiver<RuntimeEvent>> {
        let session = self.session(session_id)?;
        Ok(session.events.resubscribe())
    }

    /// Lists TUI component declarations for this session.
    ///
    /// Native in-process panels were removed (WASM-only plugins). This remains
    /// for a future host-mediated UI protocol and currently returns empty unless
    /// populated by a later extension path.
    pub fn list_tui_components(&self, session_id: &str) -> Result<Vec<String>> {
        let session = self.session(session_id)?;
        Ok(session.tui_components.clone())
    }

    /// Takes ownership of TUI component panels for this session.
    ///
    /// Native `libloading` panels are no longer loaded. Returns empty until a
    /// host-mediated WASM UI protocol is implemented.
    pub fn take_tui_panels(
        &self,
        session_id: &str,
    ) -> Result<Vec<Box<dyn navi_plugin_api::TuiComponent>>> {
        let session = self.session(session_id)?;
        let mut panels = session.tui_panels.lock().unwrap_or_else(|e| e.into_inner());
        Ok(panels.drain(..).collect())
    }

    // ── Auto-memory API ─────────────────────────────────────────────────

    /// Opens the auto-memory SQLite store for this project.
    fn memory_store(&self) -> Result<navi_core::memory::AutoMemoryStore> {
        let loaded_config = self.loaded_config();
        let manager = navi_core::memory::MemoryManager::new(
            self.inner.project_dir.clone(),
            loaded_config.data_dir.clone(),
            &loaded_config.config.memory,
        )?;
        let db_path = manager.auto_memory.db_path.clone();
        Ok(navi_core::memory::AutoMemoryStore::open(&db_path)?)
    }

    /// Saves a persistent memory entry.
    pub fn memory_write(
        &self,
        id: &str,
        memory_type: navi_core::memory::MemoryType,
        name: &str,
        description: &str,
        body: &str,
    ) -> Result<()> {
        let store = self.memory_store()?;
        let entry = navi_core::memory::new_entry(id, memory_type, name, description, body);
        store.upsert(&entry)?;
        Ok(())
    }

    /// Reads a memory by id.
    pub fn memory_read(&self, id: &str) -> Result<Option<navi_core::memory::MemoryEntry>> {
        let store = self.memory_store()?;
        Ok(store.get(id)?)
    }

    /// Lists all memories, optionally filtered by status.
    pub fn memory_list(
        &self,
        status: Option<navi_core::memory::MemoryStatus>,
    ) -> Result<Vec<navi_core::memory::MemorySummary>> {
        let store = self.memory_store()?;
        Ok(store.list(status)?)
    }

    /// Searches memories by text query.
    pub fn memory_search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<navi_core::memory::MemorySummary>> {
        let store = self.memory_store()?;
        Ok(store.search_text(query, limit)?)
    }

    /// Updates a memory's fields and/or status.
    pub fn memory_update(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
        body: Option<&str>,
        status: Option<navi_core::memory::MemoryStatus>,
    ) -> Result<()> {
        let store = self.memory_store()?;
        if let Some(s) = status {
            store.set_status(id, s)?;
        }
        store.update(id, name, description, body)?;
        Ok(())
    }

    /// Deletes a memory permanently.
    pub fn memory_delete(&self, id: &str) -> Result<()> {
        let store = self.memory_store()?;
        store.delete(id)?;
        Ok(())
    }

    /// Returns the count of active memories.
    pub fn memory_count(&self) -> Result<usize> {
        let store = self.memory_store()?;
        Ok(store.count_active()?)
    }

    /// Returns a compact markdown index of all active memories for prompt injection.
    pub fn memory_index(&self) -> String {
        let store = match self.memory_store() {
            Ok(s) => s,
            Err(_) => return String::new(),
        };
        store.build_prompt_context(2000)
    }

    // ── End auto-memory API ─────────────────────────────────────────────

    /// Syncs the provider registry into the local SQLite cache.
    ///
    /// Prefers a project-local `registry/` directory when present, otherwise
    /// fetches the manifest and provider definitions from GitHub. Returns
    /// `true` if the cache was updated.
    pub async fn sync_registry(&self, force: bool) -> Result<bool> {
        let store = self
            .inner
            .registry_store
            .as_ref()
            .ok_or_else(|| NaviError::Config("registry store is not available".into()))?;

        let local_registry = self.inner.project_dir.join("registry");
        if local_registry.join("manifest.json").is_file() {
            return navi_core::registry::sync_local_registry(store, &local_registry)
                .map_err(|e| NaviError::Config(e.to_string()));
        }

        let fetcher = navi_core::registry::RegistryFetcher::new();
        navi_core::registry::sync_registry(store, &fetcher, force)
            .await
            .map_err(|e| NaviError::Config(e.to_string()))
    }

    /// Fetches the latest model list from a specific provider and updates config.
    pub async fn sync_provider_models(
        &self,
        provider_id: &str,
        save_target: NaviConfigSaveTarget,
    ) -> Result<NaviProviderSyncReport> {
        self.sync_models_inner(Some(provider_id.to_string()), save_target)
            .await
    }

    /// Fetches the latest model lists from all configured providers.
    pub async fn sync_models(
        &self,
        save_target: NaviConfigSaveTarget,
    ) -> Result<NaviProviderSyncReport> {
        self.sync_models_inner(None, save_target).await
    }

    /// Selects a model for the engine and optionally persists the config change.
    pub fn select_model(
        &self,
        request: NaviModelSelectionRequest,
    ) -> Result<NaviModelSelectionResult> {
        let mut loaded_config = self.loaded_config();
        let provider = resolve_provider_config(&loaded_config.config, &request.provider_id)
            .with_context(|| format!("unknown provider {}", request.provider_id))?;
        let credential_store = CredentialStore::new(loaded_config.data_dir.clone());
        let has_credential =
            resolve_provider_api_key(&credential_store, &provider, &provider.id).is_some();
        let can_run_publicly = model_can_run_publicly(&provider.id, &request.model);

        loaded_config.config.model.provider = provider.id.clone();
        loaded_config.config.model.name = request.model.clone();
        let saved_to = self.save_loaded_config(&loaded_config, request.save_target)?;
        self.replace_loaded_config(loaded_config.clone());

        Ok(NaviModelSelectionResult {
            context_window_tokens: Some(effective_context_window(&loaded_config.config)),
            loaded_config,
            saved_to,
            provider_id: provider.id,
            model: request.model,
            provider_configured: has_credential || can_run_publicly,
        })
    }

    /// Lists all persisted sessions with their titles and timestamps.
    pub fn list_saved_sessions(&self) -> Result<Vec<NaviSavedSessionInfo>> {
        let loaded_config = self.loaded_config();
        let store = SessionStore::with_redaction(
            loaded_config.data_dir.clone(),
            loaded_config.config.security.redact_secrets_in_sessions,
        );
        Ok(store
            .list_info()
            .into_iter()
            .map(|info| NaviSavedSessionInfo {
                id: info.id.into_inner(),
                title: info.title,
                project: info.project,
                created_at: info.created_at,
                updated_at: info.updated_at,
            })
            .collect())
    }

    /// Lists all persisted sessions without blocking the async runtime.
    pub async fn list_saved_sessions_async(&self) -> Result<Vec<NaviSavedSessionInfo>> {
        let loaded_config = self.loaded_config();
        let store = SessionStore::with_redaction(
            loaded_config.data_dir.clone(),
            loaded_config.config.security.redact_secrets_in_sessions,
        );
        Ok(store
            .list_info_async()
            .await
            .into_iter()
            .map(|info| NaviSavedSessionInfo {
                id: info.id.into_inner(),
                title: info.title,
                project: info.project,
                created_at: info.created_at,
                updated_at: info.updated_at,
            })
            .collect())
    }

    /// Loads a persisted session snapshot by ID.
    pub fn load_saved_session(&self, session_id: &str) -> Result<SessionSnapshot> {
        let loaded_config = self.loaded_config();
        Ok(SessionStore::with_redaction(
            loaded_config.data_dir,
            loaded_config.config.security.redact_secrets_in_sessions,
        )
        .load(session_id)?)
    }

    /// Loads a persisted session snapshot by ID without blocking the async runtime.
    pub async fn load_saved_session_async(&self, session_id: &str) -> Result<SessionSnapshot> {
        let loaded_config = self.loaded_config();
        Ok(SessionStore::with_redaction(
            loaded_config.data_dir,
            loaded_config.config.security.redact_secrets_in_sessions,
        )
        .load_async(session_id.to_string())
        .await?)
    }

    /// Deletes a persisted session. Returns `true` if a session was removed.
    pub fn delete_saved_session(&self, session_id: &str) -> Result<bool> {
        let loaded_config = self.loaded_config();
        Ok(SessionStore::with_redaction(
            loaded_config.data_dir,
            loaded_config.config.security.redact_secrets_in_sessions,
        )
        .delete(session_id)?)
    }

    /// Deletes a persisted session without blocking the async runtime.
    pub async fn delete_saved_session_async(&self, session_id: &str) -> Result<bool> {
        let loaded_config = self.loaded_config();
        Ok(SessionStore::with_redaction(
            loaded_config.data_dir,
            loaded_config.config.security.redact_secrets_in_sessions,
        )
        .delete_async(session_id.to_string())
        .await?)
    }

    /// Renames a persisted session (updates the snapshot title).
    pub fn rename_saved_session(&self, session_id: &str, title: &str) -> Result<bool> {
        let loaded_config = self.loaded_config();
        Ok(SessionStore::with_redaction(
            loaded_config.data_dir,
            loaded_config.config.security.redact_secrets_in_sessions,
        )
        .rename(session_id, title)?)
    }

    /// Renames a persisted session without blocking the async runtime.
    pub async fn rename_saved_session_async(&self, session_id: &str, title: &str) -> Result<bool> {
        let loaded_config = self.loaded_config();
        Ok(SessionStore::with_redaction(
            loaded_config.data_dir,
            loaded_config.config.security.redact_secrets_in_sessions,
        )
        .rename_async(session_id.to_string(), title.to_string())
        .await?)
    }

    /// Reloads WASM plugin tools on every active in-memory session without restarting NAVI.
    ///
    /// Installed plugins are read from `{data_dir}/plugins/` plus configured `wasm_plugins` scan roots.
    #[cfg(feature = "wasm-plugins")]
    pub async fn reload_wasm_plugins(&self) -> Result<Vec<String>> {
        let loaded_config = self.loaded_config();
        let project_dir = self.inner.project_dir.clone();
        let mut warnings = Vec::new();
        for session_id in self.session_ids() {
            let session = self.session(&session_id)?;
            let mut runtime = session.runtime.lock().await;
            let mut fresh = build_local_tooling(
                &loaded_config,
                project_dir.clone(),
                &self.inner.runtime_components,
            )?;
            for tool in &self.inner.host_tools {
                let executor = Arc::get_mut(&mut fresh.tool_executor).ok_or_else(|| {
                    NaviError::Config("cannot register host tool during plugin reload".into())
                })?;
                executor.register_tool(tool.clone());
            }
            for tool in &session.mcp.tools {
                let executor = Arc::get_mut(&mut fresh.tool_executor).ok_or_else(|| {
                    NaviError::Config("cannot register MCP tool during plugin reload".into())
                })?;
                executor.register_tool(tool.clone());
            }
            {
                let executor = Arc::get_mut(&mut fresh.tool_executor).ok_or_else(|| {
                    NaviError::Config(
                        "cannot register session title tool during plugin reload".into(),
                    )
                })?;
                executor.register_tool(Arc::new(SessionTitleTool::new(
                    runtime.session_title_handle(),
                )));
            }
            runtime.set_tool_executor(fresh.tool_executor);
            warnings.extend(fresh.warnings);
        }
        Ok(warnings)
    }

    /// Reports that this build does not include the WASM plugin runtime.
    #[cfg(not(feature = "wasm-plugins"))]
    pub async fn reload_wasm_plugins(&self) -> Result<Vec<String>> {
        Ok(vec![
            "WASM plugin runtime is disabled in this build; rebuild `navi-sdk` with feature `wasm-plugins`"
                .to_string(),
        ])
    }

    /// Returns the IDs of all active (in-memory) sessions.
    pub fn session_ids(&self) -> Vec<String> {
        let mut ids = self
            .inner
            .sessions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    fn session(&self, session_id: &str) -> Result<Arc<NaviSession>> {
        self.inner
            .sessions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
            .ok_or_else(|| NaviError::SessionNotFound(session_id.to_string()))
    }

    /// Returns a snapshot of the current loaded configuration.
    pub fn loaded_config(&self) -> LoadedConfig {
        self.inner
            .loaded_config
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn replace_loaded_config(&self, loaded_config: LoadedConfig) {
        *self
            .inner
            .loaded_config
            .write()
            .unwrap_or_else(|e| e.into_inner()) = loaded_config;
    }

    async fn sync_models_inner(
        &self,
        provider_id: Option<String>,
        save_target: NaviConfigSaveTarget,
    ) -> Result<NaviProviderSyncReport> {
        if let Err(err) = self.sync_registry(false).await {
            tracing::warn!(error = %err, "registry sync failed before model sync");
        }

        let mut loaded_config = self.loaded_config();
        let credential_store = CredentialStore::new(loaded_config.data_dir.clone());
        let providers = provider_catalog(&loaded_config.config);
        let selected_provider = provider_id.as_ref().map(|id| canonical_provider_id(id));
        let mut updated = Vec::new();
        let mut failed = Vec::new();
        let mut skipped = Vec::new();

        for provider in providers {
            if let Some(selected_provider) = selected_provider
                && canonical_provider_id(&provider.id) != selected_provider
            {
                continue;
            }

            let Some(api_key) =
                resolve_provider_api_key(&credential_store, &provider, &provider.id)
            else {
                skipped.push(NaviProviderSyncSkipped {
                    provider_id: provider.id,
                    reason: "missing credential".to_string(),
                });
                continue;
            };

            // Aggregator providers (e.g. OpenRouter) get a rich model sync
            // that fetches metadata from /models and stores it in the SQLite
            // registry cache with capability tags (free, nitro, online).
            if provider.aggregator {
                if let Some(ref store) = self.inner.registry_store {
                    match navi_core::registry::sync_aggregator_models(store, &provider, &api_key)
                        .await
                    {
                        Ok(count) => {
                            updated.push(NaviSyncedProvider {
                                provider_id: provider.id,
                                model_count: count,
                            });
                        }
                        Err(error) => failed.push(NaviProviderSyncFailure {
                            provider_id: provider.id,
                            message: error.to_string(),
                        }),
                    }
                    continue;
                }
            }

            match list_models_for_provider(&provider, api_key).await {
                Ok(models) => {
                    let model_count = models.len();

                    // Persist synced model names into the SQLite registry cache
                    // so they survive config.toml stripping and are available via
                    // --print-providers and the model picker.
                    //
                    // Critical: `/models` only returns ids. New SKUs (e.g. grok-4.5)
                    // must inherit vision/context from provider defaults + family
                    // siblings already in the cache — never write bare NULL rows.
                    // When a canonical catalog entry exists, it overrides sibling
                    // guesses for context_window / reasoning_levels / effort.
                    if let Some(ref store) = self.inner.registry_store {
                        let existing = store.load_provider_models(&provider.id).unwrap_or_default();
                        let catalog = store.load_canonical_model_catalog().unwrap_or_default();
                        let catalog_ref = if catalog.is_empty() {
                            None
                        } else {
                            Some(&catalog)
                        };
                        let mut merged: Vec<RegistryModel> = models
                            .iter()
                            .map(|name| {
                                navi_core::registry::enrich_synced_registry_model_with_catalog(
                                    name,
                                    &existing,
                                    &provider.id,
                                    catalog_ref,
                                )
                            })
                            .collect();

                        // Keep cached models not returned by the API (still enrich
                        // attachment gaps so stale NULL rows get provider defaults).
                        let api_names: std::collections::HashSet<String> =
                            merged.iter().map(|m| m.name.to_lowercase()).collect();
                        for (name, _cached) in &existing {
                            if !api_names.contains(&name.to_lowercase()) {
                                merged.push(
                                    navi_core::registry::enrich_synced_registry_model_with_catalog(
                                        name,
                                        &existing,
                                        &provider.id,
                                        catalog_ref,
                                    ),
                                );
                            }
                        }

                        // Deduplicate by name (case-insensitive).
                        let mut seen: std::collections::HashSet<String> =
                            std::collections::HashSet::new();
                        merged.retain(|m| seen.insert(m.name.to_lowercase()));

                        let registry_provider = RegistryProvider {
                            id: provider.id.clone(),
                            label: provider.label.clone(),
                            description: provider.description.clone(),
                            kind: format!("{:?}", provider.kind).to_lowercase(),
                            api_key_env: provider.api_key_env.clone(),
                            base_url: provider.base_url.clone(),
                            tool_calling_mode: provider
                                .tool_calling_mode
                                .map(|m| format!("{:?}", m).to_lowercase()),
                            aggregator: provider.aggregator,
                            extends: None,
                            defaults: navi_core::registry::provider_registry_defaults(&provider.id),
                            request_options: provider.request_options.clone().unwrap_or_default(),
                            models: merged,
                        };
                        if let Err(err) = store.upsert_provider_with_sha256(
                            &registry_provider,
                            Some(navi_core::registry::LOCAL_API_SYNC_SHA),
                        ) {
                            tracing::warn!(provider = %provider.id, error = %err, "failed to persist synced models to registry cache");
                        }
                    }

                    loaded_config
                        .config
                        .update_provider_models(&provider.id, &models);
                    updated.push(NaviSyncedProvider {
                        provider_id: provider.id,
                        model_count,
                    });
                }
                Err(error) => failed.push(NaviProviderSyncFailure {
                    provider_id: provider.id,
                    message: error.to_string(),
                }),
            }
        }

        if let Some(provider_id) = provider_id {
            let canonical = canonical_provider_id(&provider_id);
            let touched = updated
                .iter()
                .any(|provider| canonical_provider_id(&provider.provider_id) == canonical)
                || failed
                    .iter()
                    .any(|provider| canonical_provider_id(&provider.provider_id) == canonical)
                || skipped
                    .iter()
                    .any(|provider| canonical_provider_id(&provider.provider_id) == canonical);
            if !touched {
                failed.push(NaviProviderSyncFailure {
                    provider_id,
                    message: "unknown provider".to_string(),
                });
            }
        }

        let saved_to = if updated.is_empty() {
            None
        } else {
            self.save_loaded_config(&loaded_config, save_target)?
        };
        if !updated.is_empty() {
            self.replace_loaded_config(loaded_config.clone());
        }

        Ok(NaviProviderSyncReport {
            loaded_config,
            saved_to,
            updated,
            failed,
            skipped,
        })
    }

    pub(crate) fn save_loaded_config(
        &self,
        loaded_config: &LoadedConfig,
        target: NaviConfigSaveTarget,
    ) -> Result<Option<PathBuf>> {
        match target {
            NaviConfigSaveTarget::None => Ok(None),
            NaviConfigSaveTarget::Project => {
                let path = save_project_config(&self.inner.project_dir, &loaded_config.config)
                    .map_err(NaviError::from)?;
                Ok(Some(path))
            }
            NaviConfigSaveTarget::Global => {
                let global_path = loaded_config
                    .global_config_path
                    .as_ref()
                    .context("global config path is unavailable")
                    .map_err(NaviError::from)?;
                let path = save_global_config(global_path, &loaded_config.config)
                    .map_err(NaviError::from)?;
                Ok(Some(path))
            }
            NaviConfigSaveTarget::Auto => {
                if loaded_config.project_config_path.is_some() {
                    let path = save_project_config(&self.inner.project_dir, &loaded_config.config)
                        .map_err(NaviError::from)?;
                    Ok(Some(path))
                } else {
                    let global_path = loaded_config
                        .global_config_path
                        .as_ref()
                        .context("global config path is unavailable")
                        .map_err(NaviError::from)?;
                    let path = save_global_config(global_path, &loaded_config.config)
                        .map_err(NaviError::from)?;
                    Ok(Some(path))
                }
            }
        }
    }

    fn credential_store(&self) -> CredentialStore {
        CredentialStore::new(self.loaded_config().data_dir)
    }

    fn provider_credential_status_for(
        &self,
        loaded_config: &LoadedConfig,
        credential_store: &CredentialStore,
        provider_id: &str,
        provider: Option<&ProviderConfig>,
    ) -> Result<NaviProviderCredentialStatus> {
        let provider_config = match provider {
            Some(provider) => provider.clone(),
            None => resolve_provider_config(&loaded_config.config, provider_id)
                .with_context(|| format!("unknown provider {provider_id}"))?,
        };
        let selected_model = (canonical_provider_id(provider_id)
            == canonical_provider_id(&loaded_config.config.model.provider))
        .then_some(loaded_config.config.model.name.as_str());
        let status = resolve_provider_credential_status(
            credential_store,
            &provider_config,
            provider_id,
            selected_model,
        );

        Ok(NaviProviderCredentialStatus {
            provider_id: provider_id.to_string(),
            configured: status.configured,
            source: status.source.map(|s| s.as_str().to_string()),
            label: status.label,
            detail: status.detail,
            env_var: provider_config.api_key_env,
            credential_store_path: credential_store.path().to_path_buf(),
        })
    }
}

fn openai_usage_limit_snapshot_to_sdk(
    snapshot: navi_providers::OpenAiUsageLimitSnapshot,
) -> NaviUsageLimitSnapshot {
    NaviUsageLimitSnapshot {
        limit_id: snapshot.limit_id,
        limit_name: snapshot.limit_name,
        metered_feature: snapshot.metered_feature,
        limit_reached: snapshot.limit_reached,
        primary: snapshot.primary.map(openai_usage_window_to_sdk),
        secondary: snapshot.secondary.map(openai_usage_window_to_sdk),
    }
}

fn openai_usage_window_to_sdk(window: navi_providers::OpenAiUsageWindow) -> NaviUsageWindow {
    NaviUsageWindow {
        used_percent: window.used_percent,
        limit_window_seconds: window.limit_window_seconds,
        reset_after_seconds: window.reset_after_seconds,
        reset_at: window.reset_at,
    }
}

fn empty_usage_report(
    provider: &navi_core::ProviderConfig,
    source: &str,
    notes: Option<String>,
) -> NaviUsageReport {
    NaviUsageReport {
        provider_id: provider.id.clone(),
        provider_label: provider.label.clone(),
        plan_type: None,
        limit_reached_kind: None,
        limits: Vec::new(),
        source: source.to_string(),
        notes,
        details: Vec::new(),
    }
}

fn percent_window(used_percent: f64) -> NaviUsageWindow {
    let used = used_percent.clamp(0.0, 100.0).round() as i32;
    NaviUsageWindow {
        used_percent: used,
        limit_window_seconds: 0,
        reset_after_seconds: 0,
        reset_at: 0,
    }
}

fn percent_window_with_period(
    used_percent: f64,
    period_start: Option<&str>,
    period_end: Option<&str>,
) -> NaviUsageWindow {
    let used = used_percent.clamp(0.0, 100.0).round() as i32;
    let (limit_window_seconds, reset_after_seconds, reset_at) =
        period_window_timing(period_start, period_end);
    NaviUsageWindow {
        used_percent: used,
        limit_window_seconds,
        reset_after_seconds,
        reset_at,
    }
}

fn period_window_timing(period_start: Option<&str>, period_end: Option<&str>) -> (i32, i32, i32) {
    let Some(end) = period_end.and_then(parse_rfc3339_unix) else {
        return (0, 0, 0);
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let reset_after = (end - now).max(0) as i32;
    let window = match period_start.and_then(parse_rfc3339_unix) {
        Some(start) if end > start => (end - start) as i32,
        _ => 0,
    };
    (window, reset_after, end.min(i32::MAX as i64) as i32)
}

fn parse_rfc3339_unix(value: &str) -> Option<i64> {
    // Prefer chrono-less parsing for common RFC3339 forms returned by xAI.
    // Examples:
    // - 2026-07-18T15:39:11.601812+00:00
    // - 2026-07-18T15:39:11Z
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Use `time` crate if available via transitive deps? Keep a small parser
    // based on `DateTime` from the standard library is not available; use
    // a lightweight approach via `httpdate` is not ideal for RFC3339 with
    // offsets. Fall back to splitting.
    // Format: YYYY-MM-DDTHH:MM:SS[.frac](Z|+HH:MM|-HH:MM)
    let (datetime, offset_secs) = if let Some(rest) = trimmed.strip_suffix('Z') {
        (rest, 0i64)
    } else if let Some(idx) = trimmed.rfind('+') {
        let (dt, off) = trimmed.split_at(idx);
        (dt, parse_offset(off)?)
    } else if let Some(idx) = trimmed.rfind('-') {
        // Distinguish date dashes from timezone minus: timezone appears after time 'T'.
        let t_pos = trimmed.find('T')?;
        if idx > t_pos {
            let (dt, off) = trimmed.split_at(idx);
            (dt, -parse_offset(&format!("+{}", &off[1..]))?)
        } else {
            return None;
        }
    } else {
        return None;
    };
    let datetime = datetime.split('.').next().unwrap_or(datetime);
    let mut parts = datetime.split('T');
    let date = parts.next()?;
    let time = parts.next()?;
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    let mut t = time.split(':');
    let hour: i64 = t.next()?.parse().ok()?;
    let minute: i64 = t.next()?.parse().ok()?;
    let second: i64 = t.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from civil date (Howard Hinnant algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468; // days since 1970-01-01
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset_secs)
}

fn parse_offset(offset: &str) -> Option<i64> {
    // +HH:MM or +HHMM
    let s = offset.trim().trim_start_matches('+');
    if let Some((h, m)) = s.split_once(':') {
        let hours: i64 = h.parse().ok()?;
        let mins: i64 = m.parse().ok()?;
        return Some(hours * 3600 + mins * 60);
    }
    if s.len() == 4 {
        let hours: i64 = s[..2].parse().ok()?;
        let mins: i64 = s[2..].parse().ok()?;
        return Some(hours * 3600 + mins * 60);
    }
    None
}

fn friendly_period_type(period_type: Option<&str>) -> Option<String> {
    match period_type {
        Some("USAGE_PERIOD_TYPE_WEEKLY") => Some("weekly".into()),
        Some("USAGE_PERIOD_TYPE_MONTHLY") => Some("monthly".into()),
        Some(other) => Some(other.to_ascii_lowercase()),
        None => None,
    }
}

fn friendly_product_name(product: &str) -> String {
    match product {
        "GrokBuild" => "Grok Build".into(),
        "Api" => "API".into(),
        other => other.to_string(),
    }
}

fn xai_report_to_sdk(
    provider: &navi_core::ProviderConfig,
    report: navi_providers::XaiUsageReport,
) -> NaviUsageReport {
    let mut limits = Vec::new();
    if let Some(credit) = report.credit_usage_percent {
        limits.push(NaviUsageLimitSnapshot {
            limit_id: Some("credits".into()),
            limit_name: Some("Weekly limit".into()),
            metered_feature: Some("credits".into()),
            limit_reached: credit >= 100.0,
            primary: Some(percent_window_with_period(
                credit,
                report.period_start.as_deref(),
                report.period_end.as_deref(),
            )),
            secondary: None,
        });
    }
    for product in &report.product_usage {
        limits.push(NaviUsageLimitSnapshot {
            limit_id: Some(product.product.to_ascii_lowercase()),
            limit_name: Some(friendly_product_name(&product.product)),
            metered_feature: Some(product.product.clone()),
            limit_reached: product.usage_percent >= 100.0,
            primary: Some(percent_window_with_period(
                product.usage_percent,
                report.period_start.as_deref(),
                report.period_end.as_deref(),
            )),
            secondary: None,
        });
    }

    let mut details = Vec::new();
    if let Some(period) = friendly_period_type(report.period_type.as_deref()) {
        details.push(NaviUsageDetail {
            label: "Period".into(),
            value: period,
        });
    }
    if let Some(end) = report.period_end.as_ref() {
        details.push(NaviUsageDetail {
            label: "Next reset".into(),
            value: end.clone(),
        });
    }
    if let Some(start) = report.period_start.as_ref() {
        details.push(NaviUsageDetail {
            label: "Period start".into(),
            value: start.clone(),
        });
    }
    if let Some(bal) = report.prepaid_balance.filter(|v| *v > 0.0) {
        details.push(NaviUsageDetail {
            label: "Prepaid balance".into(),
            value: format!("{bal}"),
        });
    }
    if let (Some(used), Some(cap)) = (report.on_demand_used, report.on_demand_cap) {
        if used > 0.0 || cap > 0.0 {
            details.push(NaviUsageDetail {
                label: "On-demand".into(),
                value: format!("{used} / {cap}"),
            });
        }
    }

    let plan = if report.is_unified_billing == Some(true) {
        Some("unified".into())
    } else {
        friendly_period_type(report.period_type.as_deref())
    };

    NaviUsageReport {
        provider_id: provider.id.clone(),
        provider_label: provider.label.clone(),
        plan_type: plan,
        limit_reached_kind: None,
        limits,
        source: "xai-oauth".into(),
        notes: Some("xAI weekly credits (OAuth).".into()),
        details,
    }
}

fn openrouter_report_to_sdk(
    provider: &navi_core::ProviderConfig,
    report: navi_providers::OpenRouterUsageReport,
) -> NaviUsageReport {
    let mut details = Vec::new();
    let fmt_usd = |v: f64| format!("${v:.4}");
    if let Some(v) = report.usage {
        details.push(NaviUsageDetail {
            label: "Lifetime spend".into(),
            value: fmt_usd(v),
        });
    }
    if let Some(v) = report.usage_daily {
        details.push(NaviUsageDetail {
            label: "Today".into(),
            value: fmt_usd(v),
        });
    }
    if let Some(v) = report.usage_weekly {
        details.push(NaviUsageDetail {
            label: "This week".into(),
            value: fmt_usd(v),
        });
    }
    if let Some(v) = report.usage_monthly {
        details.push(NaviUsageDetail {
            label: "This month".into(),
            value: fmt_usd(v),
        });
    }
    if let Some(v) = report.limit {
        details.push(NaviUsageDetail {
            label: "Credit limit".into(),
            value: fmt_usd(v),
        });
    }
    if let Some(v) = report.limit_remaining {
        details.push(NaviUsageDetail {
            label: "Remaining".into(),
            value: fmt_usd(v),
        });
    }
    if let Some(v) = report.limit_reset.as_ref() {
        details.push(NaviUsageDetail {
            label: "Resets".into(),
            value: v.clone(),
        });
    }

    let mut limits = Vec::new();
    if let (Some(limit), Some(remaining)) = (report.limit, report.limit_remaining) {
        if limit > 0.0 {
            let used_pct = ((limit - remaining) / limit * 100.0).clamp(0.0, 100.0);
            limits.push(NaviUsageLimitSnapshot {
                limit_id: Some("credit-limit".into()),
                limit_name: Some("Credit limit".into()),
                metered_feature: Some("credits".into()),
                limit_reached: remaining <= 0.0,
                primary: Some(percent_window(used_pct)),
                secondary: None,
            });
        }
    }

    let plan = if report.is_free_tier == Some(true) {
        Some("free".into())
    } else {
        report.label
    };

    NaviUsageReport {
        provider_id: provider.id.clone(),
        provider_label: provider.label.clone(),
        plan_type: plan,
        limit_reached_kind: None,
        limits,
        source: "openrouter".into(),
        notes: Some("OpenRouter key usage.".into()),
        details,
    }
}

fn charm_hyper_report_to_sdk(
    provider: &navi_core::ProviderConfig,
    report: navi_providers::CharmHyperCreditsReport,
) -> NaviUsageReport {
    let balance = report.balance;
    let usd = navi_providers::hypercredits_to_usd(balance);
    let formatted = navi_providers::format_hypercredits(balance);
    let source = report.source.as_deref().unwrap_or("credits-api");
    let details = vec![
        NaviUsageDetail {
            label: "Balance".into(),
            value: format!("◆ {formatted} Hypercredits"),
        },
        NaviUsageDetail {
            label: "Balance (USD)".into(),
            value: format!("≈ ${usd:.2}  (1 Hypercredit = $0.05)"),
        },
        NaviUsageDetail {
            label: "Billing".into(),
            value: "Prepaid Hypercredits — session spend is estimated from list rates and converted to credits.".into(),
        },
    ];
    NaviUsageReport {
        provider_id: provider.id.clone(),
        provider_label: provider.label.clone(),
        plan_type: Some("hypercredits".into()),
        limit_reached_kind: if balance <= 0.0 {
            Some("credits_depleted".into())
        } else {
            None
        },
        limits: Vec::new(),
        source: format!("charm-hyper-{source}"),
        notes: Some(match source {
            "stream-usage" => {
                "Charm Hyper remaining Hypercredits from the last stream usage payload (usage.remaining.hypercredits)."
                    .into()
            }
            _ => {
                "Charm Hyper prepaid balance (GET /v1/credits). Token list rates are USD; credits = USD ÷ $0.05."
                    .into()
            }
        }),
        details,
    }
}

fn commandcode_report_to_sdk(
    provider: &navi_core::ProviderConfig,
    data: navi_providers::CommandCodeUsageData,
) -> NaviUsageReport {
    let mut details = Vec::new();
    if let Some(whoami) = data.whoami.as_object() {
        if let Some(email) = whoami
            .get("email")
            .or_else(|| whoami.get("user").and_then(|u| u.get("email")))
            .and_then(|v| v.as_str())
        {
            details.push(NaviUsageDetail {
                label: "Account".into(),
                value: email.to_string(),
            });
        }
    }
    if let Some(credits) = data.credits.as_ref() {
        details.push(NaviUsageDetail {
            label: "Credits".into(),
            value: credits.to_string(),
        });
    }
    if let Some(sub) = data.subscription.as_ref() {
        details.push(NaviUsageDetail {
            label: "Subscription".into(),
            value: sub.to_string(),
        });
    }
    if let Some(summary) = data.usage_summary.as_ref() {
        details.push(NaviUsageDetail {
            label: "Usage summary".into(),
            value: summary.to_string(),
        });
    }
    if !data.models.is_empty() {
        details.push(NaviUsageDetail {
            label: "Models".into(),
            value: format!("{} available", data.models.len()),
        });
    }

    NaviUsageReport {
        provider_id: provider.id.clone(),
        provider_label: provider.label.clone(),
        plan_type: None,
        limit_reached_kind: None,
        limits: Vec::new(),
        source: "commandcode".into(),
        notes: Some("Command Code account usage.".into()),
        details,
    }
}

fn model_info_from_option(option: ModelOption) -> NaviModelInfo {
    let id = format!(
        "{}:{}",
        canonical_provider_id(&option.provider_id),
        option.name
    );
    let (effort_options, effort_binary) =
        crate::types::effort_options_for_model(option.supports_thinking, &option.reasoning_levels);
    NaviModelInfo {
        id,
        name: option.name,
        provider_id: option.provider_id,
        provider_label: option.provider_label,
        task_size: format!("{:?}", option.task_size),
        context_window_tokens: option.context_window_tokens,
        supports_thinking: option.supports_thinking,
        reasoning_levels: option.reasoning_levels,
        default_reasoning_effort: option.default_reasoning_effort,
        effort_options,
        effort_binary,
    }
}

fn skill_info_from_manifest(
    skill: SkillManifest,
    _project_dir: &std::path::Path,
    _data_dir: &std::path::Path,
    include_instructions: bool,
) -> NaviSkillInfo {
    let path_str = skill.path.display().to_string();
    let editable = navi_core::skill_is_editable(&skill);
    let scope = match skill.source {
        navi_core::SkillSource::Builtin => Some("builtin".into()),
        navi_core::SkillSource::Store => Some(match skill.scope {
            navi_core::SkillWriteScope::Project => "project".into(),
            navi_core::SkillWriteScope::User => "user".into(),
        }),
    };
    NaviSkillInfo {
        id: skill.id,
        name: skill.name,
        description: skill.description,
        version: skill.version,
        author: skill.author,
        tags: skill.tags,
        requires: skill.requires,
        path: Some(path_str),
        instructions: include_instructions.then_some(skill.instructions),
        editable,
        scope,
        allow_tools: skill.allow_tools,
        deny_tools: skill.deny_tools,
        source: Some(format!("{:?}", skill.source).to_lowercase()),
    }
}

#[cfg(test)]
#[path = "engine/tests.rs"]
mod tests;
