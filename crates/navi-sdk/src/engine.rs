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
            let executor = Arc::get_mut(&mut tool_executor.tool_executor).ok_or_else(|| {
                NaviError::Config("cannot register host tool after tool executor is shared".into())
            })?;
            executor.register_tool(tool.clone());
        }
        let mcp = load_configured_mcp_servers(&loaded_config.config.mcp).await;
        for tool in &mcp.tools {
            let executor = Arc::get_mut(&mut tool_executor.tool_executor).ok_or_else(|| {
                NaviError::Config("cannot register MCP tool after tool executor is shared".into())
            })?;
            executor.register_tool(tool.clone());
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
        let model_provider = build_model_provider(&loaded_config)?;

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

    /// Lists all persisted sessions without blocking the async runtime.
    pub async fn list_saved_sessions_async(&self) -> Result<Vec<NaviSavedSessionInfo>> {
        let loaded_config = self.loaded_config();
        let store = SessionStore::with_redaction(
            loaded_config.data_dir.clone(),
            loaded_config.config.security.redact_secrets_in_sessions,
        );
        Ok(store
            .list_async()
            .await
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
