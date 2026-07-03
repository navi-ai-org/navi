use crate::types::NaviError;
use anyhow::Context;
type Result<T> = std::result::Result<T, NaviError>;
use navi_core::{
    AgentRuntime, AgentRuntimeOptions, ApprovalDecision, CredentialStore, LoadedConfig,
    ModelOption, ProviderConfig, QuestionResponse, RuntimeComponents, RuntimeEvent, SessionGoal,
    SessionId, SessionSnapshot, SessionStore, SkillManifest, available_model_options,
    canonical_provider_id, config::effective_context_window, discover_configured_skills,
    model_can_run_publicly, provider_catalog, registry, registry::RegistryStore,
    resolve_provider_api_key, resolve_provider_config, resolve_provider_credential_status,
    save_global_config, save_project_config,
};
use navi_mcp::{LoadedMcpServers, McpServerInfo, load_configured_mcp_servers};
use navi_plugin_host::LoadedPlugin;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tokio::sync::{Mutex as AsyncMutex, broadcast};

use crate::tooling::{
    build_local_tooling, build_provider_for_project_config, list_models_for_provider,
};
use crate::types::{
    NaviConfigSaveTarget, NaviMissingCredentialError, NaviModelInfo, NaviModelSelectionRequest,
    NaviModelSelectionResult, NaviProviderAccountInfo, NaviProviderCredentialStatus,
    NaviProviderSyncFailure, NaviProviderSyncReport, NaviProviderSyncSkipped, NaviSavedSessionInfo,
    NaviSessionInfo, NaviSessionRequest, NaviSkillInfo, NaviSyncedProvider, NaviTurnRequest,
    NaviTurnResponse, NaviUsageLimitSnapshot, NaviUsageReport, NaviUsageWindow,
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
///     .build()
///     .expect("engine");
/// ```
#[derive(Clone)]
pub struct NaviEngineBuilder {
    project_dir: PathBuf,
    loaded_config: Option<LoadedConfig>,
    host_tools: Vec<Arc<dyn navi_core::Tool>>,
    runtime_components: RuntimeComponents,
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
            host_tools: Vec::new(),
            runtime_components: RuntimeComponents::default(),
        }
    }

    /// Overrides the loaded config instead of loading from the project directory.
    pub fn loaded_config(mut self, loaded_config: LoadedConfig) -> Self {
        self.loaded_config = Some(loaded_config);
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
        self
    }

    /// Configures the engine as an autonomous tutor runtime.
    ///
    /// This uses permissive tool security, the learning harness, tutor prompt
    /// builder, and study-aware compaction defaults.
    pub fn learning_tutor(mut self) -> Self {
        self.runtime_components = navi_core::learning_runtime_components();
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
        let loaded_config = match self.loaded_config {
            Some(config) => config,
            None => navi_core::NaviConfig::load(&self.project_dir)?,
        };

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
            if config.update_enabled {
                let store = store.clone();
                tokio::spawn(async move {
                    if registry::should_check_registry_update(&store, &config) {
                        let fetcher = registry::RegistryFetcher::new();
                        registry::run_registry_update_check(&store, &fetcher, &config).await;
                    }
                });
            }
        }

        Ok(NaviEngine {
            inner: Arc::new(NaviEngineInner {
                project_dir: self.project_dir,
                loaded_config: RwLock::new(loaded_config),
                host_tools: self.host_tools,
                runtime_components: self.runtime_components,
                sessions: RwLock::new(HashMap::new()),
                registry_store,
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
            "learning_tutor" | "navi_learning" | "tutor" => {
                components = navi_core::learning_runtime_components();
            }
            "default" | "code_agent" => {
                components = RuntimeComponents::default();
            }
            other => warnings.push(format!(
                "plugin registered unknown agent policy `{other}`; known policies are learning_tutor, navi_learning, tutor, default, and code_agent"
            )),
        }
    }
    components
}

fn normalize_plugin_policy_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace('-', "_")
}

/// The main NAVI engine handle. Clone-safe (wraps `Arc` internally).
///
/// Provides session lifecycle, model management, credential management,
/// skill discovery, and MCP server access. Create via [`NaviEngineBuilder`].
#[derive(Clone)]
pub struct NaviEngine {
    inner: Arc<NaviEngineInner>,
}

struct NaviEngineInner {
    project_dir: PathBuf,
    loaded_config: RwLock<LoadedConfig>,
    host_tools: Vec<Arc<dyn navi_core::Tool>>,
    runtime_components: RuntimeComponents,
    sessions: RwLock<HashMap<String, Arc<NaviSession>>>,
    registry_store: Option<Arc<RegistryStore>>,
}

/// An active NAVI session with its own runtime, event stream, and approval handles.
pub struct NaviSession {
    runtime: AsyncMutex<AgentRuntime>,
    events: broadcast::Receiver<RuntimeEvent>,
    approval_resolver: navi_core::ApprovalResolver,
    question_resolver: navi_core::QuestionResolver,
    turn_canceller: navi_core::TurnCanceller,
    tui_components: Vec<String>,
    mcp: LoadedMcpServers,
    _plugins: Vec<LoadedPlugin>,
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
        for warning in &tool_executor.warnings {
            tracing::warn!(warning = %warning, "plugin load warning");
        }

        // Register tools that need a weak reference back to the executor.
        let mut executor = Arc::try_unwrap(tool_executor.tool_executor).map_err(|_| {
            NaviError::Config("cannot finalize cyclic tools after executor is shared".into())
        })?;
        let shared_provider = Arc::new(std::sync::RwLock::new(provider.clone()));
        let shared_model = Arc::new(std::sync::RwLock::new(
            loaded_config.config.model.name.clone(),
        ));
        let shared_config = Arc::new(std::sync::RwLock::new(loaded_config.config.clone()));
        let prompt_cache = Arc::new(navi_core::PromptCache::new());
        executor.register_skill_loader(
            project_dir.clone(),
            loaded_config.data_dir.clone(),
            shared_config.clone(),
        );
        tool_executor.tool_executor = Arc::new_cyclic(|weak_exec| {
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
            executor.register_tool(Arc::new(navi_core::RepoExploreTool::new(
                weak_exec.clone(),
                shared_provider.clone(),
                project_dir.clone(),
                loaded_config.data_dir.clone(),
                shared_model.clone(),
                loaded_config.config.harness.clone(),
                shared_config.clone(),
                prompt_cache.clone(),
                runtime_components.clone(),
            )));
            executor
        });

        let runtime_tool_executor = tool_executor.tool_executor;
        let tui_components = tool_executor.tui_components;
        let plugins = tool_executor._plugins;
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
        });
        let events = runtime.stream_events();
        let session_id = runtime.start_session()?;
        let approval_resolver = runtime.approval_resolver();
        let question_resolver = runtime.question_resolver();
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
                    turn_canceller,
                    tui_components,
                    mcp,
                    _plugins: plugins,
                }),
            );
        Ok(info)
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
        let response = runtime
            .send_turn_with_parts(request.message, request.content_parts, request.thinking)
            .await?;
        // Check for goal auto-continue after turn completes.
        let mut response_text = response.text;
        loop {
            let continuation = runtime.goal_idle_prompt();
            if let Some(prompt) = continuation {
                drop(runtime);
                // Auto-continue: start a new turn with the steering prompt as input.
                let mut new_runtime = session.runtime.lock().await;
                let auto_response = new_runtime
                    .send_turn_with_parts(prompt, Vec::new(), None)
                    .await?;
                response_text = auto_response.text;
                runtime = new_runtime;
            } else {
                break;
            }
        }
        Ok(NaviTurnResponse {
            session_id: request.session_id,
            text: response_text,
        })
    }

    /// Cancels the currently active turn for the given session.

    /// Returns the current goal for a session.
    pub async fn get_goal(&self, session_id: &str) -> Result<Option<SessionGoal>> {
        let session = self.session(session_id)?;
        let runtime = session.runtime.lock().await;
        Ok(runtime.get_goal())
    }

    /// Sets a goal for a session. The goal will guide the agent across turns.
    pub async fn set_goal(
        &self,
        session_id: &str,
        objective: impl Into<String>,
        token_budget: Option<i64>,
    ) -> Result<SessionGoal> {
        let session = self.session(session_id)?;
        let runtime = session.runtime.lock().await;
        Ok(runtime.set_goal(objective.into(), token_budget))
    }

    /// Clears the goal for a session.
    pub async fn clear_goal(&self, session_id: &str) -> Result<()> {
        let session = self.session(session_id)?;
        let runtime = session.runtime.lock().await;
        runtime.clear_goal();
        Ok(())
    }

    pub async fn cancel_turn(&self, session_id: &str) -> Result<()> {
        let session = self.session(session_id)?;
        session.turn_canceller.cancel();
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

    /// Fetches OpenAI/ChatGPT usage windows for the selected OpenAI account.
    pub async fn usage_report(&self) -> Result<NaviUsageReport> {
        let loaded_config = self.loaded_config();
        let provider_id = loaded_config.config.model.provider.as_str();
        if canonical_provider_id(provider_id) != "openai" {
            return Err(NaviError::Config(
                "usage windows are currently available only for the OpenAI provider".into(),
            ));
        }

        let provider = resolve_provider_config(&loaded_config.config, provider_id)
            .with_context(|| format!("unknown provider {provider_id}"))?;
        let credential_store = self.credential_store();
        let access_token = resolve_provider_api_key(&credential_store, &provider, provider_id)
            .ok_or_else(|| {
                NaviError::MissingCredential(NaviMissingCredentialError {
                    provider_id: provider_id.to_string(),
                    env_var: provider.api_key_env.clone(),
                    credential_store_path: credential_store.path().to_path_buf(),
                })
            })?;

        let report = navi_providers::openai_usage_report(&access_token)
            .await
            .map_err(NaviError::Provider)?;
        Ok(NaviUsageReport {
            provider_id: provider_id.to_string(),
            provider_label: provider.label,
            plan_type: report.plan_type,
            limit_reached_kind: report.limit_reached_kind,
            limits: report
                .limits
                .into_iter()
                .map(openai_usage_limit_snapshot_to_sdk)
                .collect(),
        })
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
    pub fn set_provider_api_key(&self, provider_id: &str, api_key: &str) -> Result<()> {
        Ok(self.credential_store().set_api_key(provider_id, api_key)?)
    }

    /// Deletes a stored API key. Returns `true` if a key was removed.
    pub fn delete_provider_api_key(&self, provider_id: &str) -> Result<bool> {
        Ok(self.credential_store().delete_api_key(provider_id)?)
    }

    /// Discovers and lists configured skills from the project and global skill directories.
    pub fn list_skills(&self) -> Result<Vec<NaviSkillInfo>> {
        let loaded_config = self.loaded_config();
        Ok(discover_configured_skills(
            &loaded_config.config.skills,
            &self.inner.project_dir,
            &loaded_config.data_dir,
        )?
        .into_iter()
        .map(skill_info_from_manifest)
        .collect())
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

    /// Lists TUI component declarations registered by native plugins for this session.
    ///
    /// The SDK preserves these names as frontend-scoped declarations. `navi-tui`
    /// decides whether a known component name maps to an actual ratatui widget.
    pub fn list_tui_components(&self, session_id: &str) -> Result<Vec<String>> {
        let session = self.session(session_id)?;
        Ok(session.tui_components.clone())
    }

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

    fn replace_loaded_config(&self, loaded_config: LoadedConfig) {
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

            match list_models_for_provider(&provider, api_key).await {
                Ok(models) => {
                    let model_count = models.len();
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

    fn save_loaded_config(
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
        version: skill.version,
        author: skill.author,
        tags: skill.tags,
        requires: skill.requires,
    }
}

#[cfg(test)]
#[path = "engine/tests.rs"]
mod tests;
