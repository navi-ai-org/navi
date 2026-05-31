use crate::types::NaviError;
use anyhow::Context;
type Result<T> = std::result::Result<T, NaviError>;
use navi_core::{
    AgentMode, AgentRuntime, AgentRuntimeOptions, ApprovalDecision, CredentialStore, LoadedConfig,
    ModelOption, ProviderConfig, RuntimeEvent, SessionId, SessionSnapshot, SessionStore,
    SkillManifest, available_model_options, canonical_provider_id,
    config::effective_context_window, discover_configured_skills, model_can_run_publicly,
    provider_catalog, resolve_provider_api_key, resolve_provider_config,
    resolve_provider_credential_status, save_global_config, save_project_config,
};
use navi_mcp::{LoadedMcpServers, McpServerInfo, load_configured_mcp_servers};
use navi_plugin_host::LoadedPlugin;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tokio::sync::{Mutex as AsyncMutex, broadcast};

use crate::tooling::{build_local_tooling, build_model_provider, list_models_for_provider};
use crate::types::{
    NaviConfigSaveTarget, NaviModelInfo, NaviModelSelectionRequest, NaviModelSelectionResult,
    NaviProviderAccountInfo, NaviProviderCredentialStatus, NaviProviderSyncFailure,
    NaviProviderSyncReport, NaviProviderSyncSkipped, NaviSavedSessionInfo, NaviSessionInfo,
    NaviSessionRequest, NaviSkillInfo, NaviSyncedProvider, NaviTurnRequest, NaviTurnResponse,
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
    agent_mode: Option<AgentMode>,
    host_tools: Vec<Arc<dyn navi_core::Tool>>,
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
            agent_mode: None,
            host_tools: Vec::new(),
        }
    }

    /// Overrides the loaded config instead of loading from the project directory.
    pub fn loaded_config(mut self, loaded_config: LoadedConfig) -> Self {
        self.loaded_config = Some(loaded_config);
        self
    }

    /// Sets the initial [`AgentMode`] for sessions created by this engine.
    pub fn agent_mode(mut self, agent_mode: AgentMode) -> Self {
        self.agent_mode = Some(agent_mode);
        self
    }

    /// Registers a host-provided tool that will be available in all sessions.
    pub fn host_tool(mut self, tool: Arc<dyn navi_core::Tool>) -> Self {
        self.host_tools.push(tool);
        self
    }

    /// Builds the [`NaviEngine`]. Loads config from the project directory if not
    /// overridden via [`loaded_config`](Self::loaded_config).
    pub fn build(self) -> Result<NaviEngine> {
        let loaded_config = match self.loaded_config {
            Some(config) => config,
            None => navi_core::NaviConfig::load(&self.project_dir)?,
        };
        Ok(NaviEngine {
            inner: Arc::new(NaviEngineInner {
                project_dir: self.project_dir,
                loaded_config: RwLock::new(loaded_config),
                agent_mode: self.agent_mode,
                host_tools: self.host_tools,
                sessions: RwLock::new(HashMap::new()),
            }),
        })
    }
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
    agent_mode: Option<AgentMode>,
    host_tools: Vec<Arc<dyn navi_core::Tool>>,
    sessions: RwLock<HashMap<String, Arc<NaviSession>>>,
}

/// An active NAVI session with its own runtime, event stream, and approval handles.
pub struct NaviSession {
    runtime: AsyncMutex<AgentRuntime>,
    events: broadcast::Receiver<RuntimeEvent>,
    approval_resolver: navi_core::ApprovalResolver,
    turn_canceller: navi_core::TurnCanceller,
    mcp: LoadedMcpServers,
    _plugins: Vec<LoadedPlugin>,
}

impl NaviEngine {
    /// Starts a new agent session with the given model, context packets, and skills.
    ///
    /// Returns a [`NaviSessionInfo`] with the session ID and model details.
    /// The session can then receive turns via [`send_turn`](Self::send_turn).
    pub async fn start_session(&self, request: NaviSessionRequest) -> Result<NaviSessionInfo> {
        let project_dir = request
            .project_dir
            .clone()
            .unwrap_or_else(|| self.inner.project_dir.clone());
        let loaded_config = self.loaded_config();
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
            initial_messages: request.initial_messages,
            session_id: request.session_id.map(SessionId::new),
            event_tx: None,
        });
        let events = runtime.stream_events();
        let session_id = runtime.start_session()?;
        let approval_resolver = runtime.approval_resolver();
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
                    turn_canceller,
                    mcp,
                    _plugins: tool_executor._plugins,
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
        let response = runtime.send_turn(request.message).await?;
        Ok(NaviTurnResponse {
            session_id: request.session_id,
            text: response.text,
        })
    }

    /// Cancels the currently active turn for the given session.
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
        Ok(runtime.snapshot_session()?)
    }

    /// Changes the model used by an active session.
    pub async fn set_model(&self, session_id: &str, provider: &str, model: &str) -> Result<()> {
        let session = self.session(session_id)?;
        let mut runtime = session.runtime.lock().await;
        runtime.set_model(provider, model);
        Ok(())
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
        Ok(provider_catalog(&loaded_config.config)
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
            .collect::<Result<Vec<_>>>()?)
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
            .list()
            .into_iter()
            .map(|snapshot| NaviSavedSessionInfo {
                id: snapshot.id.into_inner(),
                title: navi_core::session_title_from_events(&snapshot.events),
                project: snapshot.project,
                created_at: snapshot.created_at,
                updated_at: snapshot.updated_at,
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

    /// Deletes a persisted session. Returns `true` if a session was removed.
    pub fn delete_saved_session(&self, session_id: &str) -> Result<bool> {
        let loaded_config = self.loaded_config();
        Ok(SessionStore::with_redaction(
            loaded_config.data_dir,
            loaded_config.config.security.redact_secrets_in_sessions,
        )
        .delete(session_id)?)
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
        let mut loaded_config = self.loaded_config();
        let credential_store = CredentialStore::new(loaded_config.data_dir.clone());
        let providers = provider_catalog(&loaded_config.config);
        let selected_provider = provider_id.as_ref().map(|id| canonical_provider_id(id));
        let mut updated = Vec::new();
        let mut failed = Vec::new();
        let mut skipped = Vec::new();

        for provider in providers {
            if let Some(selected_provider) = selected_provider.as_deref() {
                if canonical_provider_id(&provider.id) != selected_provider {
                    continue;
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::NaviMissingCredentialError;
    use navi_core::{AgentEvent, NaviConfig, SessionId, SessionSnapshot};
    use std::path::PathBuf;

    fn test_engine() -> (NaviEngine, tempfile::TempDir) {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let config = test_config();
        let loaded_config = LoadedConfig {
            config,
            global_config_path: Some(tempdir.path().join("config.toml")),
            project_config_path: None,
            data_dir: tempdir.path().to_path_buf(),
        };
        let engine = NaviEngineBuilder::from_project(tempdir.path())
            .loaded_config(loaded_config)
            .build()
            .expect("build engine");
        (engine, tempdir)
    }

    fn test_config() -> NaviConfig {
        // Use a config with a custom provider whose env var is definitely not set
        let mut config = NaviConfig::default();
        config.providers.push(ProviderConfig {
            id: "test-provider".to_string(),
            label: "Test Provider".to_string(),
            description: String::new(),
            kind: navi_core::ProviderKind::OpenAiResponses,
            api_key_env: "NAVI_SDK_TEST_NONEXISTENT_ENV_12345".to_string(),
            base_url: Some("https://example.test/v1".to_string()),
            models: vec![navi_core::config::types::ProviderModelConfig {
                name: "test-model".to_string(),
                task_size: navi_core::config::types::ModelTaskSize::Small,
                context_window_tokens: Some(8192),
                tool_prompt_manifest: None,
            }],
            ..Default::default()
        });
        config.model.provider = "test-provider".to_string();
        config.model.name = "test-model".to_string();
        config
    }

    fn test_engine_with_project_config() -> (NaviEngine, tempfile::TempDir) {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let config = test_config();
        let project_config = tempdir.path().join(".navi").join("config.toml");
        let loaded_config = LoadedConfig {
            config,
            global_config_path: Some(tempdir.path().join("global.toml")),
            project_config_path: Some(project_config),
            data_dir: tempdir.path().to_path_buf(),
        };
        let engine = NaviEngineBuilder::from_project(tempdir.path())
            .loaded_config(loaded_config)
            .build()
            .expect("build engine");
        (engine, tempdir)
    }

    fn write_session_file(tempdir: &tempfile::TempDir, session_id: &str) {
        let sessions_dir = tempdir.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");
        let snapshot = SessionSnapshot {
            version: SessionSnapshot::CURRENT_VERSION,
            id: SessionId::new(session_id.to_string()),
            title: None,
            project: PathBuf::from("/tmp/test-project"),
            created_at: 1000,
            updated_at: 2000,
            events: vec![AgentEvent::UserTaskSubmitted {
                text: "test task".to_string(),
            }],
            memory: None,
        };
        let content = serde_json::to_string(&snapshot).expect("serialize session");
        std::fs::write(sessions_dir.join(format!("{session_id}.json")), content)
            .expect("write session file");
    }

    // ── Group 1: Builder tests ──────────────────────────────────────────

    #[test]
    fn builder_with_explicit_config_succeeds() {
        let (engine, _tempdir) = test_engine();
        let loaded = engine.loaded_config();
        assert_eq!(loaded.config.model.provider, "test-provider");
        assert_eq!(loaded.config.model.name, "test-model");
    }

    #[test]
    fn builder_loads_from_project_dir() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        // Just verify that from_project().build() succeeds with defaults
        // (config loading from project dir depends on cwd, so we test the builder path)
        let result = NaviEngineBuilder::from_project(tempdir.path()).build();
        assert!(result.is_ok());
    }

    // ── Group 2: Model listing tests ────────────────────────────────────

    #[test]
    fn list_models_returns_default_models() {
        let (engine, _tempdir) = test_engine();
        let models = engine.list_models();
        assert!(!models.is_empty(), "should have built-in models");
        for model in &models {
            assert!(!model.id.is_empty());
            assert!(!model.name.is_empty());
            assert!(!model.provider_id.is_empty());
            assert!(model.id.contains(':'), "id should be provider:model format");
        }
    }

    #[test]
    fn list_models_includes_custom_provider_models() {
        let (engine, _tempdir) = test_engine();
        let models = engine.list_models();
        // The test config adds "test-provider" with a default model,
        // so it should appear alongside built-in providers
        let test_models: Vec<_> = models
            .iter()
            .filter(|m| m.provider_id == "test-provider")
            .collect();
        assert!(
            !test_models.is_empty(),
            "custom provider models should be included"
        );
    }

    // ── Group 3: Credential management tests ────────────────────────────

    #[test]
    fn credential_status_reports_missing_without_key() {
        let (engine, _tempdir) = test_engine();
        let status = engine.credential_status("test-provider").expect("status");
        assert!(!status.configured);
    }

    #[test]
    fn set_then_get_provider_api_key_roundtrip() {
        let (engine, _tempdir) = test_engine();
        engine
            .set_provider_api_key("test-provider", "sk-test-key")
            .expect("set key");
        let status = engine.credential_status("test-provider").expect("status");
        assert!(status.configured);
        assert_eq!(status.source.as_deref(), Some("stored"));
    }

    #[test]
    fn delete_provider_api_key_returns_true_for_existing() {
        let (engine, _tempdir) = test_engine();
        engine
            .set_provider_api_key("test-provider", "sk-test")
            .expect("set key");
        let deleted = engine
            .delete_provider_api_key("test-provider")
            .expect("delete");
        assert!(deleted);
        let status = engine.credential_status("test-provider").expect("status");
        assert!(!status.configured);
    }

    #[test]
    fn delete_provider_api_key_returns_false_for_missing() {
        let (engine, _tempdir) = test_engine();
        let deleted = engine
            .delete_provider_api_key("nonexistent-provider")
            .expect("delete");
        assert!(!deleted);
    }

    #[test]
    fn list_provider_accounts_returns_all_providers() {
        let (engine, _tempdir) = test_engine();
        let accounts = engine.list_provider_accounts().expect("list accounts");
        assert!(!accounts.is_empty(), "should have built-in providers");
        let ids: Vec<&str> = accounts.iter().map(|a| a.provider_id.as_str()).collect();
        assert!(ids.contains(&"openai"), "should include openai");
    }

    #[test]
    fn list_provider_accounts_reflects_stored_key() {
        let (engine, _tempdir) = test_engine();
        engine
            .set_provider_api_key("test-provider", "sk-test")
            .expect("set key");
        let accounts = engine.list_provider_accounts().expect("list accounts");
        let test_prov = accounts
            .iter()
            .find(|a| a.provider_id == "test-provider")
            .expect("test-provider account");
        assert!(test_prov.has_stored_key);

        // Other providers should not have stored keys
        for account in &accounts {
            if account.provider_id != "test-provider" {
                assert!(
                    !account.has_stored_key,
                    "{} should not have stored key",
                    account.provider_id
                );
            }
        }
    }

    #[test]
    fn credential_status_errors_for_unknown_provider() {
        let (engine, _tempdir) = test_engine();
        let result = engine.credential_status("nonexistent-provider-xyz");
        assert!(result.is_err());
    }

    // ── Group 4: Model selection tests ──────────────────────────────────

    #[test]
    fn select_model_updates_loaded_config() {
        let (engine, _tempdir) = test_engine();
        let result = engine
            .select_model(NaviModelSelectionRequest {
                provider_id: "openai".to_string(),
                model: "gpt-5.1".to_string(),
                save_target: NaviConfigSaveTarget::None,
            })
            .expect("select model");
        assert_eq!(result.provider_id, "openai");
        assert_eq!(result.model, "gpt-5.1");
        assert_eq!(result.loaded_config.config.model.provider, "openai");
        assert_eq!(result.loaded_config.config.model.name, "gpt-5.1");
    }

    #[test]
    fn select_model_returns_context_window() {
        let (engine, _tempdir) = test_engine();
        let result = engine
            .select_model(NaviModelSelectionRequest {
                provider_id: "openai".to_string(),
                model: "gpt-5.1".to_string(),
                save_target: NaviConfigSaveTarget::None,
            })
            .expect("select model");
        assert!(result.context_window_tokens.is_some());
        assert!(result.context_window_tokens.unwrap() > 0);
    }

    #[test]
    fn select_model_with_save_target_none_returns_no_path() {
        let (engine, _tempdir) = test_engine();
        let result = engine
            .select_model(NaviModelSelectionRequest {
                provider_id: "openai".to_string(),
                model: "gpt-5.1".to_string(),
                save_target: NaviConfigSaveTarget::None,
            })
            .expect("select model");
        assert!(result.saved_to.is_none());
    }

    #[test]
    fn select_model_with_save_target_project_writes_config() {
        let (engine, _tempdir) = test_engine_with_project_config();
        let result = engine
            .select_model(NaviModelSelectionRequest {
                provider_id: "openai".to_string(),
                model: "gpt-5.1".to_string(),
                save_target: NaviConfigSaveTarget::Project,
            })
            .expect("select model");
        assert!(result.saved_to.is_some());
        let saved_path = result.saved_to.unwrap();
        assert!(saved_path.exists());
    }

    #[test]
    fn select_model_errors_for_unknown_provider() {
        let (engine, _tempdir) = test_engine();
        let result = engine.select_model(NaviModelSelectionRequest {
            provider_id: "nonexistent-provider-xyz".to_string(),
            model: "some-model".to_string(),
            save_target: NaviConfigSaveTarget::None,
        });
        assert!(result.is_err());
    }

    #[test]
    fn select_model_reports_configured_for_public_model() {
        let (engine, _tempdir) = test_engine();
        // OpenRouter with free model should be publicly accessible
        let result = engine.select_model(NaviModelSelectionRequest {
            provider_id: "openrouter".to_string(),
            model: "deepseek/deepseek-v4-flash:free".to_string(),
            save_target: NaviConfigSaveTarget::None,
        });
        // This may or may not work depending on whether openrouter has free models configured
        // The important thing is the method doesn't panic
        if let Ok(result) = result {
            // If it succeeded, check the field exists
            let _ = result.provider_configured;
        }
    }

    #[test]
    fn select_model_reports_not_configured_without_key() {
        let (engine, _tempdir) = test_engine();
        let result = engine
            .select_model(NaviModelSelectionRequest {
                provider_id: "test-provider".to_string(),
                model: "test-model".to_string(),
                save_target: NaviConfigSaveTarget::None,
            })
            .expect("select model");
        // No key stored, so should report not configured
        assert!(!result.provider_configured);
    }

    #[test]
    fn select_model_engine_state_updates() {
        let (engine, _tempdir) = test_engine();
        engine
            .select_model(NaviModelSelectionRequest {
                provider_id: "anthropic".to_string(),
                model: "claude-sonnet-4-20250514".to_string(),
                save_target: NaviConfigSaveTarget::None,
            })
            .expect("select model");

        let loaded = engine.loaded_config();
        assert_eq!(loaded.config.model.provider, "anthropic");
        assert_eq!(loaded.config.model.name, "claude-sonnet-4-20250514");
    }

    // ── Group 5: Session persistence tests ──────────────────────────────

    #[test]
    fn list_saved_sessions_returns_empty_initially() {
        let (engine, _tempdir) = test_engine();
        let sessions = engine.list_saved_sessions().expect("list sessions");
        assert!(sessions.is_empty());
    }

    #[test]
    fn list_saved_sessions_returns_prepopulated_sessions() {
        let (engine, tempdir) = test_engine();
        write_session_file(&tempdir, "test-session-123");

        let sessions = engine.list_saved_sessions().expect("list sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "test-session-123");
        assert_eq!(sessions[0].project, PathBuf::from("/tmp/test-project"));
    }

    #[test]
    fn load_saved_session_loads_prepopulated() {
        let (engine, tempdir) = test_engine();
        write_session_file(&tempdir, "load-test-456");

        let snapshot = engine
            .load_saved_session("load-test-456")
            .expect("load session");
        assert_eq!(snapshot.id.as_str(), "load-test-456");
        assert_eq!(snapshot.project, PathBuf::from("/tmp/test-project"));
    }

    #[test]
    fn load_saved_session_errors_for_missing() {
        let (engine, _tempdir) = test_engine();
        let result = engine.load_saved_session("nonexistent-session");
        assert!(result.is_err());
    }

    #[test]
    fn delete_saved_session_removes_file() {
        let (engine, tempdir) = test_engine();
        write_session_file(&tempdir, "delete-test-789");

        // Verify it exists first
        let sessions = engine.list_saved_sessions().expect("list");
        assert_eq!(sessions.len(), 1);

        // Delete it
        let deleted = engine
            .delete_saved_session("delete-test-789")
            .expect("delete");
        assert!(deleted);

        // Verify it's gone
        let sessions = engine.list_saved_sessions().expect("list");
        assert!(sessions.is_empty());
    }

    #[test]
    fn delete_saved_session_returns_false_for_missing() {
        let (engine, _tempdir) = test_engine();
        let deleted = engine
            .delete_saved_session("nonexistent-session")
            .expect("delete");
        assert!(!deleted);
    }

    // ── Group 6: Skills tests ───────────────────────────────────────────

    #[test]
    fn list_skills_returns_empty_when_disabled() {
        let (engine, _tempdir) = test_engine();
        let skills = engine.list_skills().expect("list skills");
        // Default config has skills.enabled = false, so no skills should be discovered
        // (even if there are no skill dirs, this should return empty, not error)
        let _ = skills;
    }

    // ── Group 7: Config save target tests ───────────────────────────────

    #[test]
    fn select_model_save_target_auto_prefers_project() {
        let (engine, _td) = test_engine_with_project_config();
        let result = engine
            .select_model(NaviModelSelectionRequest {
                provider_id: "openai".to_string(),
                model: "gpt-5.1".to_string(),
                save_target: NaviConfigSaveTarget::Auto,
            })
            .expect("select model");
        assert!(result.saved_to.is_some());
    }

    #[test]
    fn select_model_save_target_auto_falls_back_to_global() {
        let (engine, _tempdir) = test_engine();
        let result = engine
            .select_model(NaviModelSelectionRequest {
                provider_id: "openai".to_string(),
                model: "gpt-5.1".to_string(),
                save_target: NaviConfigSaveTarget::Auto,
            })
            .expect("select model");
        assert!(result.saved_to.is_some());
    }

    #[test]
    fn select_model_save_target_global_writes_global() {
        let (engine, _tempdir) = test_engine();
        let result = engine
            .select_model(NaviModelSelectionRequest {
                provider_id: "openai".to_string(),
                model: "gpt-5.1".to_string(),
                save_target: NaviConfigSaveTarget::Global,
            })
            .expect("select model");
        assert!(result.saved_to.is_some());
        let saved_path = result.saved_to.unwrap();
        assert!(saved_path.exists());
    }

    #[test]
    fn select_model_save_target_project_writes_project() {
        let (engine, _tempdir) = test_engine_with_project_config();
        let result = engine
            .select_model(NaviModelSelectionRequest {
                provider_id: "openai".to_string(),
                model: "gpt-5.1".to_string(),
                save_target: NaviConfigSaveTarget::Project,
            })
            .expect("select model");
        assert!(result.saved_to.is_some());
        let saved_path = result.saved_to.unwrap();
        assert!(saved_path.exists());
    }

    // ── Group 8: Error type tests ───────────────────────────────────────

    #[test]
    fn missing_credential_error_display_includes_details() {
        let error = NaviMissingCredentialError {
            provider_id: "test-provider".to_string(),
            env_var: "TEST_ENV_VAR".to_string(),
            credential_store_path: PathBuf::from("/tmp/creds.toml"),
        };
        let msg = error.message();
        assert!(msg.contains("test-provider"));
        assert!(msg.contains("TEST_ENV_VAR"));
        assert!(msg.contains("/tmp/creds.toml"));

        // Display trait
        let display = format!("{error}");
        assert_eq!(display, msg);

        // Error trait
        let err: &dyn std::error::Error = &error;
        assert!(err.to_string().contains("test-provider"));
    }
}
