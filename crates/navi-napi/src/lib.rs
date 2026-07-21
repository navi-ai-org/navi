use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
    ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue,
};
use napi_derive::napi;
// `Either` is used by startSession for string | request-object overloads.

/// Global Tokio runtime used by synchronous N-API constructors so that
/// `tokio::spawn` calls inside `NaviEngineBuilder::build()` find a reactor.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        // Invariant: the N-API module cannot operate without a Tokio reactor.
        // `Runtime::new` only fails on OS resource exhaustion (threads/FDs), which is
        // unrecoverable here — `OnceLock::get_or_init` cannot propagate `Result`.
        tokio::runtime::Runtime::new().expect("failed to create Tokio runtime for navi-napi")
    })
}

/// Installs a panic hook that logs panics instead of aborting the Node.js process.
/// This provides a layer of crash isolation when running in-process via N-API:
/// panics in the agent runtime are logged and the error is returned to JS,
/// rather than taking down the entire Electron application.
///
/// Called automatically on module load via `#[napi_derive::module_init]`.
#[napi_derive::module_init]
fn install_panic_guard() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        eprintln!("[navi-napi] panic in agent runtime: {info}");
        default_hook(info);
    }));
}
use navi_core::{
    ContentPart, ContextPacket, ThinkingConfig, ToolInvocation, ToolKind, ToolResult,
};
use navi_sdk::{
    ApprovalDecision, HostToolDefinition, HostToolHandler, HostToolInvocation, NaviAcpTurnRequest,
    NaviConfigSaveTarget, NaviEngineBuilder, NaviModelSelectionRequest, NaviPromptProfile,
    NaviSecurityProfile, NaviSessionRequest, NaviToolProfile, NaviTurnRequest, ProviderConfig,
    ProviderKind, ProviderModelConfig, QuestionResponse, RuntimeEvent, SdkHostTool,
    SdkHostToolResult,
};
use serde_json::{Value as JsonValue, json};
use tokio::sync::{Mutex as AsyncMutex, broadcast};

type JsHostToolCallback = ThreadsafeFunction<JsonValue, Promise<JsonValue>>;
type JsHookCallback = ThreadsafeFunction<JsonValue, UnknownReturnValue>;

#[napi(object)]
pub struct JsSessionInfo {
    pub id: String,
    pub project_dir: String,
    pub model: String,
    pub provider: String,
}

#[napi(object)]
pub struct JsTurnResponse {
    pub session_id: String,
    pub text: String,
}

#[napi(object)]
pub struct JsHostToolDefinition {
    pub name: String,
    pub description: String,
    pub kind: Option<String>,
    pub input_schema: Option<JsonValue>,
}

#[derive(Clone, Default)]
#[napi(object)]
pub struct JsTurnOptions {
    pub content_parts: Option<Vec<JsonValue>>,
    pub context_packets: Option<Vec<JsonValue>>,
    pub thinking: Option<String>,
}

#[napi]
pub struct NaviNapiEngine {
    inner: navi_sdk::NaviEngine,
}

#[napi]
pub struct NaviNapiEventStream {
    receiver: AsyncMutex<broadcast::Receiver<RuntimeEvent>>,
}

#[napi]
pub struct NaviNapiVoiceEventStream {
    receiver: AsyncMutex<broadcast::Receiver<navi_sdk::VoiceEvent>>,
}

#[napi(object)]
#[derive(Clone, Default)]
pub struct JsSessionRequest {
    pub session_id: Option<String>,
    pub project_dir: Option<String>,
    pub context_packets: Option<Vec<JsonValue>>,
    pub active_skills: Option<Vec<String>>,
    pub initial_messages: Option<Vec<JsonValue>>,
    pub initial_events: Option<Vec<JsonValue>>,
    pub initial_created_at: Option<i64>,
    pub initial_updated_at: Option<i64>,
    pub initial_goal: Option<JsonValue>,
}

#[napi(object)]
#[derive(Clone, Default)]
pub struct JsProviderUpsert {
    pub id: String,
    pub label: Option<String>,
    pub description: Option<String>,
    /// `openai-chat-completions` (default) or `openai-responses`.
    pub kind: Option<String>,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    /// Optional model names to seed the provider catalog (e.g. Ollama tags).
    pub models: Option<Vec<String>>,
}

#[napi]
pub struct NaviNapiEngineBuilder {
    project_dir: String,
    hooks: JsHookCallbacks,
    host_tools: Vec<Arc<dyn navi_core::Tool>>,
    data_dir: Option<String>,
    loaded_config_json: Option<JsonValue>,
    tool_profile: Option<String>,
    allow_tools: Option<Vec<String>>,
    deny_tools: Option<Vec<String>>,
    prompt_profile: Option<String>,
    security_profile: Option<String>,
    permission_mode: Option<String>,
}

#[derive(Clone, Default)]
struct JsHookCallbacks {
    session_start: Option<Arc<JsHookCallback>>,
    turn_start: Option<Arc<JsHookCallback>>,
    tool_call: Option<Arc<JsHookCallback>>,
    tool_result: Option<Arc<JsHookCallback>>,
    turn_end: Option<Arc<JsHookCallback>>,
    session_end: Option<Arc<JsHookCallback>>,
}

impl JsHookCallbacks {
    fn is_empty(&self) -> bool {
        self.session_start.is_none()
            && self.turn_start.is_none()
            && self.tool_call.is_none()
            && self.tool_result.is_none()
            && self.turn_end.is_none()
            && self.session_end.is_none()
    }
}

#[napi]
impl NaviNapiEngineBuilder {
    #[napi(constructor)]
    pub fn new(project_dir: String) -> Self {
        Self {
            project_dir,
            hooks: JsHookCallbacks::default(),
            host_tools: Vec::new(),
            data_dir: None,
            loaded_config_json: None,
            tool_profile: None,
            allow_tools: None,
            deny_tools: None,
            prompt_profile: None,
            security_profile: None,
            permission_mode: None,
        }
    }

    /// Durable app data directory (sessions, credentials, plugins, registry).
    #[napi(js_name = "dataDir")]
    pub fn data_dir(&mut self, path: String) {
        self.data_dir = Some(path);
    }

    /// Inject a config payload mapping to `LoadedConfig` / `NaviConfig`.
    ///
    /// Accepts either `{ config, dataDir?, globalConfigPath?, projectConfigPath? }`
    /// or a bare `NaviConfig` object. Invalid payloads fail at `build()` with a
    /// clear error (no panic).
    #[napi(js_name = "loadedConfig")]
    pub fn loaded_config(&mut self, config: JsonValue) {
        self.loaded_config_json = Some(config);
    }

    /// Tool profile: `code_agent` | `host_tools_only` | `chat_only`.
    #[napi(js_name = "toolProfile")]
    pub fn tool_profile(&mut self, profile: String) {
        self.tool_profile = Some(profile);
    }

    #[napi(js_name = "allowTools")]
    pub fn allow_tools(&mut self, names: Vec<String>) {
        self.allow_tools = Some(names);
    }

    #[napi(js_name = "denyTools")]
    pub fn deny_tools(&mut self, names: Vec<String>) {
        self.deny_tools = Some(names);
    }

    /// Prompt profile: `code_agent` | `assistant`.
    #[napi(js_name = "promptProfile")]
    pub fn prompt_profile(&mut self, profile: String) {
        self.prompt_profile = Some(profile);
    }

    /// Security profile: `code_agent` | `host_app`.
    #[napi(js_name = "securityProfile")]
    pub fn security_profile(&mut self, profile: String) {
        self.security_profile = Some(profile);
    }

    /// Permission mode: `restricted` | `accept-edits` | `auto` | `yolo`.
    #[napi(js_name = "permissionMode")]
    pub fn permission_mode(&mut self, mode: String) {
        self.permission_mode = Some(mode);
    }

    #[napi(js_name = "onSessionStart")]
    pub fn on_session_start(
        &mut self,
        handler: Function<JsonValue, UnknownReturnValue>,
    ) -> Result<()> {
        self.hooks.session_start = Some(Arc::new(build_hook_callback(handler)?));
        Ok(())
    }

    #[napi(js_name = "onTurnStart")]
    pub fn on_turn_start(
        &mut self,
        handler: Function<JsonValue, UnknownReturnValue>,
    ) -> Result<()> {
        self.hooks.turn_start = Some(Arc::new(build_hook_callback(handler)?));
        Ok(())
    }

    #[napi(js_name = "onToolCall")]
    pub fn on_tool_call(&mut self, handler: Function<JsonValue, UnknownReturnValue>) -> Result<()> {
        self.hooks.tool_call = Some(Arc::new(build_hook_callback(handler)?));
        Ok(())
    }

    #[napi(js_name = "onToolResult")]
    pub fn on_tool_result(
        &mut self,
        handler: Function<JsonValue, UnknownReturnValue>,
    ) -> Result<()> {
        self.hooks.tool_result = Some(Arc::new(build_hook_callback(handler)?));
        Ok(())
    }

    #[napi(js_name = "onTurnEnd")]
    pub fn on_turn_end(&mut self, handler: Function<JsonValue, UnknownReturnValue>) -> Result<()> {
        self.hooks.turn_end = Some(Arc::new(build_hook_callback(handler)?));
        Ok(())
    }

    #[napi(js_name = "onSessionEnd")]
    pub fn on_session_end(
        &mut self,
        handler: Function<JsonValue, UnknownReturnValue>,
    ) -> Result<()> {
        self.hooks.session_end = Some(Arc::new(build_hook_callback(handler)?));
        Ok(())
    }

    #[napi(js_name = "hostTool")]
    pub fn host_tool(
        &mut self,
        definition: JsHostToolDefinition,
        handler: Function<JsonValue, Promise<JsonValue>>,
    ) -> Result<()> {
        let callback = handler
            .build_threadsafe_function::<JsonValue>()
            .callee_handled::<true>()
            .weak::<false>()
            .build()
            .map_err(to_napi_error)?;
        let tool = SdkHostTool::new(
            HostToolDefinition {
                name: definition.name,
                description: definition.description,
                kind: parse_tool_kind(definition.kind.as_deref())?,
                input_schema: definition
                    .input_schema
                    .unwrap_or_else(|| json!({ "type": "object" })),
            },
            Arc::new(JsHostToolHandler { callback }),
        );
        self.host_tools.push(Arc::new(tool));
        Ok(())
    }

    #[napi]
    pub fn build(&mut self) -> Result<NaviNapiEngine> {
        let mut builder = NaviEngineBuilder::from_project(self.project_dir.clone());

        if let Some(cfg_json) = self.loaded_config_json.take() {
            let loaded = parse_loaded_config_payload(cfg_json).map_err(to_napi_error)?;
            builder = builder.loaded_config(loaded);
        }
        if let Some(dir) = self.data_dir.take() {
            builder = builder.data_dir(dir);
        }
        if let Some(profile) = self.tool_profile.take() {
            let p = NaviToolProfile::parse(&profile).ok_or_else(|| {
                to_napi_error(anyhow::anyhow!(
                    "invalid tool profile `{profile}`; expected code_agent, host_tools_only, or chat_only"
                ))
            })?;
            builder = builder.tool_profile(p);
        }
        if let Some(names) = self.allow_tools.take() {
            builder = builder.allow_tools(names);
        }
        if let Some(names) = self.deny_tools.take() {
            builder = builder.deny_tools(names);
        }
        if let Some(profile) = self.prompt_profile.take() {
            let p = NaviPromptProfile::parse(&profile).ok_or_else(|| {
                to_napi_error(anyhow::anyhow!(
                    "invalid prompt profile `{profile}`; expected code_agent or assistant"
                ))
            })?;
            builder = builder.prompt_profile(p);
        }
        if let Some(profile) = self.security_profile.take() {
            let p = NaviSecurityProfile::parse(&profile).ok_or_else(|| {
                to_napi_error(anyhow::anyhow!(
                    "invalid security profile `{profile}`; expected code_agent or host_app"
                ))
            })?;
            builder = builder.security_profile(p);
        }
        if let Some(mode) = self.permission_mode.take() {
            let pm = parse_permission_mode_str(&mode).map_err(to_napi_error)?;
            builder = builder.permission_mode(pm);
        }

        if !self.hooks.is_empty() {
            // Use hooks() only — runtime_components() would wipe prompt profile.
            builder = builder.hooks(Arc::new(JsSessionHooks {
                callbacks: self.hooks.clone(),
            }));
        }
        for tool in self.host_tools.drain(..) {
            builder = builder.host_tool(tool);
        }
        let inner = runtime()
            .block_on(async { builder.build() })
            .map_err(to_napi_error)?;
        Ok(NaviNapiEngine { inner })
    }
}

#[napi]
impl NaviNapiEngine {
    #[napi(constructor)]
    pub fn new(project_dir: String) -> Result<Self> {
        let builder = NaviEngineBuilder::from_project(project_dir);
        let inner = runtime()
            .block_on(async { builder.build() })
            .map_err(to_napi_error)?;
        Ok(Self { inner })
    }

    /// Start a session. Prefer a full request object; two-arg form is kept for
    /// backward compatibility (`sessionId`, `projectDir`).
    #[napi]
    pub async fn start_session(
        &self,
        request: Option<Either<String, JsSessionRequest>>,
        project_dir: Option<String>,
    ) -> Result<JsSessionInfo> {
        let req = match request {
            None => NaviSessionRequest {
                project_dir: project_dir.map(std::path::PathBuf::from),
                ..NaviSessionRequest::default()
            },
            Some(Either::A(session_id)) => NaviSessionRequest {
                session_id: Some(session_id),
                project_dir: project_dir.map(std::path::PathBuf::from),
                ..NaviSessionRequest::default()
            },
            Some(Either::B(opts)) => parse_session_request(opts)?,
        };
        let info = self
            .inner
            .start_session(req)
            .await
            .map_err(to_napi_error)?;
        Ok(session_info_to_js(info))
    }

    /// Full `NaviSessionRequest` surface (camelCase options object).
    #[napi(js_name = "startSessionWithRequest")]
    pub async fn start_session_with_request(
        &self,
        request: JsSessionRequest,
    ) -> Result<JsSessionInfo> {
        let info = self
            .inner
            .start_session(parse_session_request(request)?)
            .await
            .map_err(to_napi_error)?;
        Ok(session_info_to_js(info))
    }

    /// Reopen a saved snapshot JSON with full history + attachment rehydration.
    #[napi(js_name = "startSessionFromSnapshot")]
    pub async fn start_session_from_snapshot(&self, snapshot: JsonValue) -> Result<JsSessionInfo> {
        let snapshot: navi_core::SessionSnapshot =
            serde_json::from_value(snapshot).map_err(to_napi_error)?;
        let info = self
            .inner
            .start_session_from_snapshot(&snapshot)
            .await
            .map_err(to_napi_error)?;
        Ok(session_info_to_js(info))
    }

    /// Force-compact session history (same path as Rust `compact_session`).
    #[napi(js_name = "compactSession")]
    pub async fn compact_session(&self, session_id: String) -> Result<JsonValue> {
        let outcome = self
            .inner
            .compact_session(&session_id)
            .await
            .map_err(to_napi_error)?;
        Ok(json!({
            "tokensSaved": outcome.tokens_saved,
            "summary": outcome.summary,
            "keptRecentMessages": outcome.kept_recent_messages,
        }))
    }

    /// Tool names visible on an active session after profile filtering.
    #[napi(js_name = "listSessionTools")]
    pub async fn list_session_tools(&self, session_id: String) -> Result<Vec<String>> {
        self.inner
            .list_session_tools(&session_id)
            .await
            .map_err(to_napi_error)
    }

    #[napi(js_name = "toolProfile")]
    pub fn tool_profile(&self) -> String {
        self.inner.tool_profile().as_str().to_string()
    }

    #[napi(js_name = "promptProfile")]
    pub fn prompt_profile(&self) -> String {
        self.inner.prompt_profile().as_str().to_string()
    }

    #[napi(js_name = "securityProfile")]
    pub fn security_profile(&self) -> String {
        self.inner.security_profile().as_str().to_string()
    }

    /// Upsert a custom OpenAI-compatible provider (e.g. Ollama at localhost).
    #[napi(js_name = "upsertProvider")]
    pub fn upsert_provider(
        &self,
        provider: JsProviderUpsert,
        save_target: Option<String>,
    ) -> Result<JsonValue> {
        let cfg = provider_from_upsert(provider).map_err(to_napi_error)?;
        let result = self
            .inner
            .upsert_provider(cfg, parse_save_target(save_target.as_deref()))
            .map_err(to_napi_error)?;
        Ok(json!({
            "providerId": result.provider_id,
            "savedTo": result.saved_to.map(|p| p.display().to_string()),
        }))
    }

    /// List configured ACP external agents (not model providers).
    #[napi(js_name = "listAcpAgents")]
    pub fn list_acp_agents(&self) -> Result<JsonValue> {
        serde_json::to_value(self.inner.list_acp_agents()).map_err(to_napi_error)
    }

    /// Delegate a turn to an ACP peer. Options: `{ agentId, prompt, cwd?, sessionId? }`.
    #[napi(js_name = "delegateAcpTurn")]
    pub async fn delegate_acp_turn(&self, request: JsonValue) -> Result<JsonValue> {
        let agent_id = request
            .get("agentId")
            .or_else(|| request.get("agent_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| to_napi_error(anyhow::anyhow!("delegateAcpTurn requires agentId")))?
            .to_string();
        let prompt = request
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| to_napi_error(anyhow::anyhow!("delegateAcpTurn requires prompt")))?
            .to_string();
        let cwd = request
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from);
        let session_id = request
            .get("sessionId")
            .or_else(|| request.get("session_id"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let response = self
            .inner
            .delegate_acp_turn(NaviAcpTurnRequest {
                agent_id,
                prompt,
                cwd,
                session_id,
            })
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(response).map_err(to_napi_error)
    }

    /// Simple ACP delegate: agent id + prompt only.
    #[napi(js_name = "delegateAcpTurnSimple")]
    pub async fn delegate_acp_turn_simple(
        &self,
        agent_id: String,
        prompt: String,
    ) -> Result<JsonValue> {
        let result = self
            .inner
            .delegate_acp_turn_simple(&agent_id, prompt)
            .await
            .map_err(to_napi_error)?;
        Ok(json!({
            "stopReason": format!("{:?}", result.stop_reason),
            "text": result.text,
        }))
    }

    #[napi]
    pub async fn send_turn(
        &self,
        session_id: String,
        message: String,
        options: Option<JsTurnOptions>,
    ) -> Result<JsTurnResponse> {
        let (content_parts, context_packets, thinking) = match options {
            Some(opts) => {
                let cp = opts
                    .content_parts
                    .unwrap_or_default()
                    .into_iter()
                    .map(|v| serde_json::from_value::<ContentPart>(v).map_err(to_napi_error))
                    .collect::<Result<Vec<_>>>()?;
                let ctx = opts
                    .context_packets
                    .unwrap_or_default()
                    .into_iter()
                    .map(|v| serde_json::from_value::<ContextPacket>(v).map_err(to_napi_error))
                    .collect::<Result<Vec<_>>>()?;
                let th = opts
                    .thinking
                    .as_deref()
                    .map(parse_thinking_config)
                    .transpose()?;
                (cp, ctx, th)
            }
            None => (Vec::new(), Vec::new(), None),
        };
        let response = self
            .inner
            .send_turn(NaviTurnRequest {
                session_id,
                message,
                content_parts,
                context_packets,
                thinking,
            })
            .await
            .map_err(to_napi_error)?;
        Ok(JsTurnResponse {
            session_id: response.session_id,
            text: response.text,
        })
    }

    #[napi]
    pub async fn snapshot_session(&self, session_id: String) -> Result<String> {
        let snapshot = self
            .inner
            .snapshot_session(&session_id)
            .await
            .map_err(to_napi_error)?;
        serde_json::to_string(&snapshot).map_err(to_napi_error)
    }

    #[napi]
    pub async fn close_session(&self, session_id: String) -> Result<bool> {
        self.inner
            .close_session(&session_id)
            .await
            .map_err(to_napi_error)
    }

    #[napi]
    pub async fn cancel_turn(&self, session_id: String) -> Result<()> {
        self.inner
            .cancel_turn(&session_id)
            .await
            .map_err(to_napi_error)
    }

    /// Rewind live history for edit-message: keep the first `keepUserTurns`
    /// user turns, drop everything after. Caller then sends the new text.
    #[napi]
    pub async fn rewind_session(&self, session_id: String, keep_user_turns: u32) -> Result<u32> {
        let remaining = self
            .inner
            .rewind_session(&session_id, keep_user_turns as usize)
            .await
            .map_err(to_napi_error)?;
        Ok(remaining as u32)
    }

    #[napi]
    pub async fn resolve_approval(
        &self,
        session_id: String,
        approval_id: String,
        approved: bool,
    ) -> Result<bool> {
        let decision = if approved {
            ApprovalDecision::Approved { id: approval_id }
        } else {
            ApprovalDecision::Denied { id: approval_id }
        };
        self.inner
            .resolve_approval(&session_id, decision)
            .await
            .map_err(to_napi_error)
    }

    #[napi]
    pub async fn add_context_packet(&self, session_id: String, packet: JsonValue) -> Result<()> {
        self.inner
            .add_context_packet(&session_id, parse_context_packet(packet)?)
            .await
            .map_err(to_napi_error)
    }

    #[napi]
    pub fn list_models(&self) -> Result<JsonValue> {
        serde_json::to_value(self.inner.list_models()).map_err(to_napi_error)
    }

    #[napi]
    pub fn list_tui_components(&self, session_id: String) -> Result<Vec<String>> {
        self.inner
            .list_tui_components(&session_id)
            .map_err(to_napi_error)
    }

    /// Installed plugin packages that ship a `tui.json` extension spec.
    #[napi]
    pub fn list_tui_extensions(&self) -> Result<JsonValue> {
        let list = self.inner.list_tui_extensions().map_err(to_napi_error)?;
        serde_json::to_value(list).map_err(to_napi_error)
    }

    /// Flattened palette commands from all installed `tui.json` specs.
    #[napi]
    pub fn list_tui_extension_commands(&self) -> Result<JsonValue> {
        let list = self
            .inner
            .list_tui_extension_commands()
            .map_err(to_napi_error)?;
        serde_json::to_value(list).map_err(to_napi_error)
    }

    #[napi]
    pub async fn set_model(
        &self,
        session_id: String,
        provider: String,
        model: String,
    ) -> Result<()> {
        self.inner
            .set_model(&session_id, &provider, &model)
            .await
            .map_err(to_napi_error)
    }

    #[napi]
    pub fn subscribe_events(&self, session_id: String) -> Result<NaviNapiEventStream> {
        let receiver = self
            .inner
            .subscribe_events(&session_id)
            .map_err(to_napi_error)?;
        Ok(NaviNapiEventStream {
            receiver: AsyncMutex::new(receiver),
        })
    }

    // ── Goals ──────────────────────────────────────────────────────────

    #[napi]
    pub async fn get_goal(&self, session_id: String) -> Result<JsonValue> {
        let goal = self
            .inner
            .get_goal(&session_id)
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(goal).map_err(to_napi_error)
    }

    #[napi]
    pub async fn set_goal(
        &self,
        session_id: String,
        objective: String,
        token_budget: Option<i64>,
    ) -> Result<JsonValue> {
        let goal = self
            .inner
            .set_goal(&session_id, objective, token_budget)
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(goal).map_err(to_napi_error)
    }

    #[napi]
    pub async fn clear_goal(&self, session_id: String) -> Result<()> {
        self.inner
            .clear_goal(&session_id)
            .await
            .map_err(to_napi_error)
    }

    #[napi]
    pub async fn update_goal_status(
        &self,
        session_id: String,
        status: String,
    ) -> Result<JsonValue> {
        let goal_status = match status.as_str() {
            "active" => navi_sdk::GoalStatus::Active,
            "paused" => navi_sdk::GoalStatus::Paused,
            "blocked" => navi_sdk::GoalStatus::Blocked,
            "usage_limited" => navi_sdk::GoalStatus::UsageLimited,
            "budget_limited" => navi_sdk::GoalStatus::BudgetLimited,
            "complete" => navi_sdk::GoalStatus::Complete,
            other => {
                return Err(to_napi_error(anyhow::anyhow!(
                    "unknown goal status: {other}"
                )));
            }
        };
        let goal = self
            .inner
            .update_goal_status(&session_id, goal_status)
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(goal).map_err(to_napi_error)
    }

    #[napi]
    pub async fn update_goal_checklist(
        &self,
        session_id: String,
        tasks: JsonValue,
    ) -> Result<JsonValue> {
        let tasks: Vec<navi_sdk::GoalTask> =
            serde_json::from_value(tasks).map_err(to_napi_error)?;
        let goal = self
            .inner
            .update_goal_checklist(&session_id, tasks)
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(goal).map_err(to_napi_error)
    }

    #[napi]
    pub async fn update_goal_task_status(
        &self,
        session_id: String,
        task_id: u32,
        status: String,
    ) -> Result<JsonValue> {
        let task_status = match status.as_str() {
            "pending" => navi_sdk::TaskStatus::Pending,
            "in_progress" => navi_sdk::TaskStatus::InProgress,
            "done" => navi_sdk::TaskStatus::Done,
            "verified" => navi_sdk::TaskStatus::Verified,
            "skipped" => navi_sdk::TaskStatus::Skipped,
            other => {
                return Err(to_napi_error(anyhow::anyhow!(
                    "unknown task status: {other}"
                )));
            }
        };
        let goal = self
            .inner
            .update_goal_task_status(&session_id, task_id as usize, task_status)
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(goal).map_err(to_napi_error)
    }

    // ── Plan Mode ───────────────────────────────────────────────────

    #[napi]
    pub fn agent_mode(&self, session_id: String) -> Result<String> {
        let mode = self.inner.agent_mode(&session_id).map_err(to_napi_error)?;
        Ok(mode.to_string())
    }

    #[napi]
    pub async fn enter_plan_mode(&self, session_id: String) -> Result<()> {
        self.inner
            .enter_plan_mode(&session_id)
            .await
            .map_err(to_napi_error)
    }

    #[napi]
    pub async fn exit_plan_mode(&self, session_id: String) -> Result<()> {
        self.inner
            .exit_plan_mode(&session_id)
            .await
            .map_err(to_napi_error)
    }

    // ── Questions ──────────────────────────────────────────────────────

    #[napi]
    pub async fn resolve_question(&self, session_id: String, response: JsonValue) -> Result<bool> {
        let qr: QuestionResponse = serde_json::from_value(response).map_err(to_napi_error)?;
        self.inner
            .resolve_question(&session_id, qr)
            .await
            .map_err(to_napi_error)
    }

    #[napi]
    pub async fn resolve_plan_review(
        &self,
        session_id: String,
        response: JsonValue,
    ) -> Result<bool> {
        let pr: navi_sdk::PlanReviewResponse =
            serde_json::from_value(response).map_err(to_napi_error)?;
        self.inner
            .resolve_plan_review(&session_id, pr)
            .await
            .map_err(to_napi_error)
    }

    /// Resolve a sudo password prompt. Pass `{ kind: "submitted", id, password }` or
    /// `{ kind: "cancelled", id }`. Password must never be logged by the client.
    #[napi]
    pub async fn resolve_sudo_password(
        &self,
        session_id: String,
        response: JsonValue,
    ) -> Result<bool> {
        let sr: navi_sdk::SudoPasswordResponse =
            serde_json::from_value(response).map_err(to_napi_error)?;
        self.inner
            .resolve_sudo_password(&session_id, sr)
            .await
            .map_err(to_napi_error)
    }

    // ── Background Tasks ───────────────────────────────────────────────

    #[napi]
    pub async fn list_background_commands(&self, session_id: String) -> Result<JsonValue> {
        let commands = self
            .inner
            .list_background_commands(&session_id)
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(commands).map_err(to_napi_error)
    }

    #[napi]
    pub async fn poll_background_command(
        &self,
        session_id: String,
        task_id: String,
    ) -> Result<JsonValue> {
        let snapshot = self
            .inner
            .poll_background_command(&session_id, &task_id)
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(snapshot).map_err(to_napi_error)
    }

    #[napi]
    pub async fn cancel_background_command(
        &self,
        session_id: String,
        task_id: String,
    ) -> Result<JsonValue> {
        let snapshot = self
            .inner
            .cancel_background_command(&session_id, &task_id)
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(snapshot).map_err(to_napi_error)
    }

    // ── Provider Accounts & Credentials ────────────────────────────────

    #[napi]
    pub fn list_provider_accounts(&self) -> Result<JsonValue> {
        let accounts = self.inner.list_provider_accounts().map_err(to_napi_error)?;
        serde_json::to_value(accounts).map_err(to_napi_error)
    }

    #[napi]
    pub fn credential_status(&self, provider_id: String) -> Result<JsonValue> {
        let status = self
            .inner
            .credential_status(&provider_id)
            .map_err(to_napi_error)?;
        serde_json::to_value(status).map_err(to_napi_error)
    }

    #[napi]
    pub fn set_provider_api_key(&self, provider_id: String, api_key: String) -> Result<()> {
        self.inner
            .set_provider_api_key(&provider_id, &api_key)
            .map_err(to_napi_error)
    }

    #[napi]
    pub fn delete_provider_api_key(&self, provider_id: String) -> Result<bool> {
        self.inner
            .delete_provider_api_key(&provider_id)
            .map_err(to_napi_error)
    }

    /// Multi-account: list stored credential accounts for a provider.
    #[napi]
    pub fn list_credential_accounts(&self, provider_id: String) -> Result<JsonValue> {
        let accounts = self
            .inner
            .list_credential_accounts(&provider_id)
            .map_err(to_napi_error)?;
        serde_json::to_value(accounts).map_err(to_napi_error)
    }

    /// Multi-account: add an API key account (does not wipe siblings).
    #[napi]
    pub fn add_provider_account(
        &self,
        provider_id: String,
        api_key: String,
        label: Option<String>,
    ) -> Result<JsonValue> {
        let account_id = self
            .inner
            .add_provider_account(&provider_id, &api_key, label.as_deref())
            .map_err(to_napi_error)?;
        Ok(json!({ "accountId": account_id }))
    }

    /// Multi-account: select active account (default + project binding).
    #[napi]
    pub fn select_provider_account(
        &self,
        provider_id: String,
        account_id: String,
    ) -> Result<JsonValue> {
        self.inner
            .select_provider_account(&provider_id, &account_id)
            .map_err(to_napi_error)?;
        Ok(json!({ "selected": true, "accountId": account_id }))
    }

    /// Multi-account: delete one credential account.
    #[napi]
    pub fn delete_provider_account(&self, provider_id: String, account_id: String) -> Result<bool> {
        self.inner
            .delete_provider_account(&provider_id, &account_id)
            .map_err(to_napi_error)
    }

    #[napi]
    pub fn provider_supports_device_oauth(&self, provider_id: String) -> bool {
        self.inner.provider_supports_device_oauth(&provider_id)
    }

    /// Run device/browser OAuth. Optional `onStarted` receives
    /// `{ verificationUri, userCode }` when the user must authorize.
    /// Blocks until the flow completes; returns optional secondary token.
    #[napi]
    pub async fn start_device_oauth(
        &self,
        provider_id: String,
        on_started: Option<ThreadsafeFunction<JsonValue, UnknownReturnValue>>,
    ) -> Result<Option<String>> {
        let cb = on_started.map(Arc::new);
        self.inner
            .start_device_oauth(&provider_id, move |info| {
                if let Some(ref tsfn) = cb {
                    let payload = json!({
                        "verificationUri": info.verification_uri,
                        "userCode": info.user_code,
                    });
                    let _ = tsfn.call(Ok(payload), ThreadsafeFunctionCallMode::NonBlocking);
                }
            })
            .await
            .map_err(to_napi_error)
    }

    /// Device OAuth without a progress callback.
    #[napi]
    pub async fn start_device_oauth_simple(&self, provider_id: String) -> Result<Option<String>> {
        self.inner
            .start_device_oauth_simple(&provider_id)
            .await
            .map_err(to_napi_error)
    }

    // ── Usage ──────────────────────────────────────────────────────────

    #[napi]
    pub async fn usage_report(&self) -> Result<JsonValue> {
        let report = self.inner.usage_report().await.map_err(to_napi_error)?;
        serde_json::to_value(report).map_err(to_napi_error)
    }

    // ── Skills ─────────────────────────────────────────────────────────

    #[napi]
    pub fn list_skills(&self) -> Result<JsonValue> {
        let skills = self.inner.list_skills().map_err(to_napi_error)?;
        serde_json::to_value(skills).map_err(to_napi_error)
    }

    #[napi]
    pub fn get_skill(&self, skill_id: String) -> Result<JsonValue> {
        let skill = self.inner.get_skill(&skill_id).map_err(to_napi_error)?;
        serde_json::to_value(skill).map_err(to_napi_error)
    }

    /// Create or update a skill as standard SKILL.md (shared with TUI/CLI).
    ///
    /// `params` keys (camelCase): id?, name, description?, version?, author?,
    /// tags?, requires?, instructions, scope? ("user" | "project").
    #[napi]
    pub fn save_skill(&self, params: JsonValue) -> Result<JsonValue> {
        let request: navi_core::SkillWriteRequest =
            serde_json::from_value(params).map_err(to_napi_error)?;
        let result = self.inner.save_skill(request).map_err(to_napi_error)?;
        // Return a UI-friendly payload with full skill info + flags.
        let loaded = self
            .inner
            .get_skill(&result.skill.id)
            .map_err(to_napi_error)?;
        Ok(json!({
            "created": result.created,
            "path": result.path.display().to_string(),
            "skill": loaded,
        }))
    }

    #[napi]
    pub fn delete_skill(&self, skill_id: String) -> Result<JsonValue> {
        let deleted = self.inner.delete_skill(&skill_id).map_err(to_napi_error)?;
        Ok(json!({ "deleted": deleted }))
    }

    #[napi]
    pub async fn set_session_skills(&self, session_id: String, skills: Vec<String>) -> Result<()> {
        self.inner
            .set_session_skills(&session_id, skills)
            .await
            .map_err(to_napi_error)
    }

    // ── MCP ────────────────────────────────────────────────────────────

    #[napi]
    pub fn list_mcp_servers(&self, session_id: String) -> Result<JsonValue> {
        let servers = self
            .inner
            .list_mcp_servers(&session_id)
            .map_err(to_napi_error)?;
        serde_json::to_value(servers).map_err(to_napi_error)
    }

    #[napi]
    pub fn list_mcp_tools(&self, session_id: String) -> Result<Vec<String>> {
        self.inner
            .list_mcp_tools(&session_id)
            .map_err(to_napi_error)
    }

    /// Configured MCP servers from TOML (not live session connections).
    #[napi]
    pub fn list_mcp_config(&self) -> Result<JsonValue> {
        serde_json::to_value(self.inner.list_mcp_config()).map_err(to_napi_error)
    }

    #[napi]
    pub fn set_mcp_enabled(&self, enabled: bool, save_target: Option<String>) -> Result<JsonValue> {
        let path = self
            .inner
            .set_mcp_enabled(enabled, parse_save_target(save_target.as_deref()))
            .map_err(to_napi_error)?;
        Ok(json!({ "savedTo": path.map(|p| p.display().to_string()) }))
    }

    /// Upsert MCP server. `server` is a full McpServerConfig JSON object.
    #[napi]
    pub fn upsert_mcp_server(
        &self,
        server: JsonValue,
        save_target: Option<String>,
    ) -> Result<JsonValue> {
        let server: navi_core::McpServerConfig =
            serde_json::from_value(server).map_err(to_napi_error)?;
        let path = self
            .inner
            .upsert_mcp_server(server, parse_save_target(save_target.as_deref()))
            .map_err(to_napi_error)?;
        Ok(json!({ "savedTo": path.map(|p| p.display().to_string()) }))
    }

    #[napi]
    pub fn remove_mcp_server(
        &self,
        server_id: String,
        save_target: Option<String>,
    ) -> Result<JsonValue> {
        let (removed, path) = self
            .inner
            .remove_mcp_server(&server_id, parse_save_target(save_target.as_deref()))
            .map_err(to_napi_error)?;
        Ok(json!({
            "removed": removed,
            "savedTo": path.map(|p| p.display().to_string()),
        }))
    }

    /// Replace the entire MCP config block.
    #[napi]
    pub fn set_mcp_config(&self, mcp: JsonValue, save_target: Option<String>) -> Result<JsonValue> {
        let mcp: navi_core::McpConfig = serde_json::from_value(mcp).map_err(to_napi_error)?;
        let path = self
            .inner
            .set_mcp_config(mcp, parse_save_target(save_target.as_deref()))
            .map_err(to_napi_error)?;
        Ok(json!({ "savedTo": path.map(|p| p.display().to_string()) }))
    }

    // ── Model Selection ────────────────────────────────────────────────

    #[napi]
    pub fn select_model(
        &self,
        provider_id: String,
        model: String,
        save_target: Option<String>,
    ) -> Result<JsonValue> {
        let request = NaviModelSelectionRequest {
            provider_id,
            model,
            save_target: parse_save_target(save_target.as_deref()),
        };
        let result = self.inner.select_model(request).map_err(to_napi_error)?;
        Ok(json!({
            "provider_id": result.provider_id,
            "model": result.model,
            "context_window_tokens": result.context_window_tokens,
            "provider_configured": result.provider_configured,
            "saved_to": result.saved_to,
        }))
    }

    /// Set attachment fallback model for a modality (image|audio|video|document).
    #[napi]
    pub fn set_attachment_model(
        &self,
        modality: String,
        provider: String,
        model: String,
        save_target: Option<String>,
    ) -> Result<JsonValue> {
        let path = self
            .inner
            .set_attachment_model(
                &modality,
                &provider,
                &model,
                parse_save_target(save_target.as_deref()),
            )
            .map_err(to_napi_error)?;
        Ok(json!({ "savedTo": path.map(|p| p.display().to_string()) }))
    }

    /// Clear attachment fallback for a modality.
    #[napi]
    pub fn clear_attachment_model(
        &self,
        modality: String,
        save_target: Option<String>,
    ) -> Result<JsonValue> {
        let path = self
            .inner
            .clear_attachment_model(&modality, parse_save_target(save_target.as_deref()))
            .map_err(to_napi_error)?;
        Ok(json!({ "savedTo": path.map(|p| p.display().to_string()) }))
    }

    /// Set background-task model override (memory_extraction|compaction|repo_search|…).
    #[napi]
    pub fn set_background_model(
        &self,
        task: String,
        provider: String,
        model: String,
        save_target: Option<String>,
    ) -> Result<JsonValue> {
        let path = self
            .inner
            .set_background_model(
                &task,
                &provider,
                &model,
                parse_save_target(save_target.as_deref()),
            )
            .map_err(to_napi_error)?;
        Ok(json!({ "savedTo": path.map(|p| p.display().to_string()) }))
    }

    /// Clear background-task model override.
    #[napi]
    pub fn clear_background_model(
        &self,
        task: String,
        save_target: Option<String>,
    ) -> Result<JsonValue> {
        let path = self
            .inner
            .clear_background_model(&task, parse_save_target(save_target.as_deref()))
            .map_err(to_napi_error)?;
        Ok(json!({ "savedTo": path.map(|p| p.display().to_string()) }))
    }

    // ── Provider Model Sync ────────────────────────────────────────────

    #[napi]
    pub async fn sync_provider_models(
        &self,
        provider_id: String,
        save_target: Option<String>,
    ) -> Result<JsonValue> {
        let report = self
            .inner
            .sync_provider_models(&provider_id, parse_save_target(save_target.as_deref()))
            .await
            .map_err(to_napi_error)?;
        Ok(json!({
            "saved_to": report.saved_to,
            "updated": report.updated,
            "failed": report.failed,
            "skipped": report.skipped,
        }))
    }

    #[napi]
    pub async fn sync_models(&self, save_target: Option<String>) -> Result<JsonValue> {
        let report = self
            .inner
            .sync_models(parse_save_target(save_target.as_deref()))
            .await
            .map_err(to_napi_error)?;
        Ok(json!({
            "saved_to": report.saved_to,
            "updated": report.updated,
            "failed": report.failed,
            "skipped": report.skipped,
        }))
    }

    // ── Registry ───────────────────────────────────────────────────────

    #[napi]
    pub async fn sync_registry(&self, force: Option<bool>) -> Result<bool> {
        self.inner
            .sync_registry(force.unwrap_or(false))
            .await
            .map_err(to_napi_error)
    }

    #[napi]
    pub fn list_registry(&self) -> Result<JsonValue> {
        self.inner.list_registry().map_err(to_napi_error)
    }

    // ── Plugins (install / marketplace) ────────────────────────────────

    #[napi]
    pub fn plugin_list(&self) -> Result<JsonValue> {
        let list = self.inner.plugin_list().map_err(to_napi_error)?;
        serde_json::to_value(list).map_err(to_napi_error)
    }

    #[napi]
    pub fn plugin_info(&self, plugin_id: String) -> Result<JsonValue> {
        let info = self.inner.plugin_info(&plugin_id).map_err(to_napi_error)?;
        serde_json::to_value(info).map_err(to_napi_error)
    }

    #[napi]
    pub async fn plugin_search(&self, query: Option<String>) -> Result<JsonValue> {
        let hits = self
            .inner
            .plugin_search(query.as_deref())
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(hits).map_err(to_napi_error)
    }

    #[napi]
    pub fn plugin_install_path(&self, path: String, confirm: bool) -> Result<JsonValue> {
        let result = self
            .inner
            .plugin_install_path(std::path::Path::new(&path), confirm)
            .map_err(to_napi_error)?;
        serde_json::to_value(result).map_err(to_napi_error)
    }

    /// Install from a local path with explicit trust level and marketplace kind.
    ///
    /// `trust`: `local-dev` (default) | `community` | `signed` | `core`
    /// `kind`: `plugin` (default) | `skill` | `mcp` | `integration`
    #[napi]
    pub fn plugin_install_path_with_meta(
        &self,
        path: String,
        confirm: bool,
        trust: Option<String>,
        kind: Option<String>,
    ) -> Result<JsonValue> {
        use navi_plugin_manifest::{PluginCatalogKind, TrustLevel};
        let trust = match trust.as_deref().unwrap_or("local-dev") {
            "local-dev" | "local_dev" | "localdev" => TrustLevel::LocalDev,
            "community" => TrustLevel::Community,
            "signed" => TrustLevel::Signed,
            "core" => TrustLevel::Core,
            other => {
                return Err(to_napi_error(anyhow::anyhow!(
                    "invalid trust level '{other}' (expected local-dev|community|signed|core)"
                )));
            }
        };
        let kind = match kind.as_deref().unwrap_or("plugin") {
            "plugin" => PluginCatalogKind::Plugin,
            "skill" => PluginCatalogKind::Skill,
            "mcp" => PluginCatalogKind::Mcp,
            "integration" => PluginCatalogKind::Integration,
            other => {
                return Err(to_napi_error(anyhow::anyhow!(
                    "invalid package kind '{other}' (expected plugin|skill|mcp|integration)"
                )));
            }
        };
        let result = self
            .inner
            .plugin_install_path_with_meta(std::path::Path::new(&path), confirm, trust, kind)
            .map_err(to_napi_error)?;
        serde_json::to_value(result).map_err(to_napi_error)
    }

    #[napi]
    pub async fn plugin_install_marketplace(
        &self,
        plugin_id: String,
        confirm: bool,
    ) -> Result<JsonValue> {
        let result = self
            .inner
            .plugin_install_marketplace(&plugin_id, confirm)
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(result).map_err(to_napi_error)
    }

    #[napi]
    pub fn plugin_update_path(
        &self,
        path: String,
        force: Option<bool>,
        confirm: Option<bool>,
    ) -> Result<JsonValue> {
        let result = self
            .inner
            .plugin_update_path(
                std::path::Path::new(&path),
                force.unwrap_or(false),
                confirm.unwrap_or(false),
            )
            .map_err(to_napi_error)?;
        serde_json::to_value(result).map_err(to_napi_error)
    }

    #[napi]
    pub async fn plugin_update_marketplace(
        &self,
        plugin_id: String,
        force: Option<bool>,
        confirm: Option<bool>,
    ) -> Result<JsonValue> {
        let result = self
            .inner
            .plugin_update_marketplace(&plugin_id, force.unwrap_or(false), confirm.unwrap_or(false))
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(result).map_err(to_napi_error)
    }

    #[napi]
    pub fn plugin_remove(&self, plugin_id: String) -> Result<()> {
        self.inner.plugin_remove(&plugin_id).map_err(to_napi_error)
    }

    // ── Wasm Plugins ───────────────────────────────────────────────────

    #[napi]
    pub async fn reload_wasm_plugins(&self) -> Result<Vec<String>> {
        self.inner
            .reload_wasm_plugins()
            .await
            .map_err(to_napi_error)
    }

    // ── Saved Sessions ─────────────────────────────────────────────────

    #[napi]
    pub async fn list_saved_sessions(&self) -> Result<JsonValue> {
        let sessions = self
            .inner
            .list_saved_sessions_async()
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(sessions).map_err(to_napi_error)
    }

    /// Restores a saved session into the live runtime.
    ///
    /// Returns a **lightweight** payload (no event history) so desktop shells can
    /// paint the UI from `load_saved_session_async` / a UI cache and restore in the
    /// background without re-serializing multi‑MB event logs across the N-API boundary.
    #[napi]
    pub async fn load_saved_session(&self, session_id: String) -> Result<JsonValue> {
        // Already live — free path for re-focusing a session.
        if self.inner.session_ids().iter().any(|id| id == &session_id) {
            return Ok(serde_json::json!({
                "id": session_id,
                "restored": true,
                "already_active": true,
            }));
        }

        let snapshot = self
            .inner
            .load_saved_session_async(&session_id)
            .await
            .map_err(to_napi_error)?;

        let project = snapshot.project.clone();
        let created_at = snapshot.created_at;
        let updated_at = snapshot.updated_at;
        // Rebuild provider history (path first, then durable attachment store).
        let data_dir = self.inner.loaded_config().data_dir;
        let req = navi_sdk::session_request_from_snapshot(&snapshot, Some(data_dir.as_path()));

        self.inner.start_session(req).await.map_err(to_napi_error)?;

        Ok(serde_json::json!({
            "id": session_id,
            "project": project,
            "created_at": created_at,
            "updated_at": updated_at,
            "restored": true,
            "already_active": false,
        }))
    }

    #[napi]
    pub async fn delete_saved_session(&self, session_id: String) -> Result<bool> {
        self.inner
            .delete_saved_session_async(&session_id)
            .await
            .map_err(to_napi_error)
    }

    #[napi]
    pub async fn rename_saved_session(&self, session_id: String, title: String) -> Result<bool> {
        self.inner
            .rename_saved_session_async(&session_id, &title)
            .await
            .map_err(to_napi_error)
    }

    #[napi]
    pub async fn rename_saved_session_async(
        &self,
        session_id: String,
        title: String,
    ) -> Result<bool> {
        self.inner
            .rename_saved_session_async(&session_id, &title)
            .await
            .map_err(to_napi_error)
    }

    #[napi]
    pub async fn list_saved_sessions_async(&self) -> Result<JsonValue> {
        let sessions = self
            .inner
            .list_saved_sessions_async()
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(sessions).map_err(to_napi_error)
    }

    #[napi]
    pub async fn load_saved_session_async(&self, session_id: String) -> Result<JsonValue> {
        let snapshot = self
            .inner
            .load_saved_session_async(&session_id)
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(snapshot).map_err(to_napi_error)
    }

    #[napi]
    pub async fn delete_saved_session_async(&self, session_id: String) -> Result<bool> {
        self.inner
            .delete_saved_session_async(&session_id)
            .await
            .map_err(to_napi_error)
    }

    #[napi]
    pub fn take_tui_panels(&self, session_id: String) -> Result<Vec<JsonValue>> {
        let panels = self
            .inner
            .take_tui_panels(&session_id)
            .map_err(to_napi_error)?;
        // TuiComponent is not serializable, return metadata only
        Ok(panels
            .iter()
            .map(|_| serde_json::json!({"taken": true}))
            .collect())
    }

    // ── Auto-Memory ─────────────────────────────────────────────────────

    #[napi]
    pub fn memory_write(
        &self,
        id: String,
        memory_type: String,
        name: String,
        description: String,
        body: String,
    ) -> Result<()> {
        let mt = navi_sdk::MemoryType::from_str(&memory_type)
            .ok_or_else(|| to_napi_error(anyhow::anyhow!("invalid memory_type: {memory_type}")))?;
        self.inner
            .memory_write(&id, mt, &name, &description, &body)
            .map_err(to_napi_error)
    }

    #[napi]
    pub fn memory_read(&self, id: String) -> Result<JsonValue> {
        let entry = self.inner.memory_read(&id).map_err(to_napi_error)?;
        match entry {
            Some(e) => serde_json::to_value(e).map_err(to_napi_error),
            None => Ok(json!(null)),
        }
    }

    #[napi]
    pub fn memory_list(&self, status: Option<String>) -> Result<JsonValue> {
        let filter = status.as_deref().and_then(navi_sdk::MemoryStatus::from_str);
        let memories = self.inner.memory_list(filter).map_err(to_napi_error)?;
        serde_json::to_value(memories).map_err(to_napi_error)
    }

    #[napi]
    pub fn memory_search(&self, query: String, limit: Option<i32>) -> Result<JsonValue> {
        let lim = limit.map(|l| l as usize).unwrap_or(20);
        let results = self
            .inner
            .memory_search(&query, lim)
            .map_err(to_napi_error)?;
        serde_json::to_value(results).map_err(to_napi_error)
    }

    #[napi]
    pub fn memory_update(
        &self,
        id: String,
        name: Option<String>,
        description: Option<String>,
        body: Option<String>,
        status: Option<String>,
    ) -> Result<()> {
        let st = status.as_deref().and_then(navi_sdk::MemoryStatus::from_str);
        self.inner
            .memory_update(
                &id,
                name.as_deref(),
                description.as_deref(),
                body.as_deref(),
                st,
            )
            .map_err(to_napi_error)
    }

    #[napi]
    pub fn memory_delete(&self, id: String) -> Result<()> {
        self.inner.memory_delete(&id).map_err(to_napi_error)
    }

    #[napi]
    pub fn memory_count(&self) -> Result<i32> {
        self.inner
            .memory_count()
            .map(|c| c as i32)
            .map_err(to_napi_error)
    }

    #[napi]
    pub fn memory_index(&self) -> String {
        self.inner.memory_index()
    }

    #[napi]
    pub fn memory_status(&self) -> Result<JsonValue> {
        let report = self.inner.memory_status().map_err(to_napi_error)?;
        serde_json::to_value(report).map_err(to_napi_error)
    }

    #[napi]
    pub fn memory_doctor(&self) -> Result<JsonValue> {
        let report = self.inner.memory_doctor().map_err(to_napi_error)?;
        serde_json::to_value(report).map_err(to_napi_error)
    }

    #[napi]
    pub async fn memory_init(
        &self,
        embeddings: Option<bool>,
        force: Option<bool>,
    ) -> Result<JsonValue> {
        let report = self
            .inner
            .memory_init(embeddings.unwrap_or(false), force.unwrap_or(false))
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(report).map_err(to_napi_error)
    }

    #[napi]
    pub fn memory_history_search(
        &self,
        query: String,
        session_id: Option<String>,
        limit: Option<i64>,
    ) -> Result<JsonValue> {
        let events = self
            .inner
            .memory_history_search(&query, session_id.as_deref(), limit)
            .map_err(to_napi_error)?;
        serde_json::to_value(events).map_err(to_napi_error)
    }

    #[napi]
    pub async fn memory_dream(
        &self,
        apply: Option<bool>,
        sessions: Option<u32>,
        instructions: Option<String>,
    ) -> Result<JsonValue> {
        let report = self
            .inner
            .memory_dream(
                apply.unwrap_or(false),
                sessions.unwrap_or(10) as usize,
                instructions,
            )
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(report).map_err(to_napi_error)
    }

    #[napi]
    pub async fn memory_distill(&self) -> Result<()> {
        self.inner.memory_distill().await.map_err(to_napi_error)
    }

    #[napi]
    pub async fn memory_checkpoint(&self) -> Result<String> {
        self.inner.memory_checkpoint().await.map_err(to_napi_error)
    }

    #[napi]
    pub fn memory_rebuild_preview(&self) -> Result<String> {
        self.inner.memory_rebuild_preview().map_err(to_napi_error)
    }

    // ── Voice / dictation ────────────────────────────────────────────────

    #[napi]
    pub fn voice_status(&self) -> Result<JsonValue> {
        let status = self.inner.voice_status().map_err(to_napi_error)?;
        serde_json::to_value(status).map_err(to_napi_error)
    }

    /// Registry transcription providers (OpenAI / Groq, …).
    #[napi]
    pub fn voice_transcription_providers(&self) -> Result<JsonValue> {
        serde_json::to_value(self.inner.voice_transcription_providers()).map_err(to_napi_error)
    }

    /// Partial update of `[voice]` settings. `update` keys are camelCase optional fields.
    #[napi]
    pub fn set_voice_config(
        &self,
        update: JsonValue,
        save_target: Option<String>,
    ) -> Result<JsonValue> {
        let update: navi_sdk::VoiceConfigUpdate =
            serde_json::from_value(update).map_err(to_napi_error)?;
        let path = self
            .inner
            .set_voice_config(update, parse_save_target(save_target.as_deref()))
            .map_err(to_napi_error)?;
        Ok(json!({ "savedTo": path.map(|p| p.display().to_string()) }))
    }

    #[napi]
    pub fn voice_doctor(&self) -> Result<JsonValue> {
        let report = self.inner.voice_doctor().map_err(to_napi_error)?;
        serde_json::to_value(report).map_err(to_napi_error)
    }

    #[napi]
    pub fn voice_engine_installed(&self, engine: Option<String>) -> Result<bool> {
        self.inner
            .voice_engine_installed(engine.as_deref())
            .map_err(to_napi_error)
    }

    #[napi]
    pub async fn voice_init(&self, engine: Option<String>, force: Option<bool>) -> Result<String> {
        let path = self
            .inner
            .voice_init(engine.as_deref(), force.unwrap_or(false))
            .await
            .map_err(to_napi_error)?;
        Ok(path.display().to_string())
    }

    /// Transcribe a WAV file (blocking ONNX runs on a worker thread).
    #[napi]
    pub async fn voice_transcribe_file(
        &self,
        path: String,
        language: Option<String>,
    ) -> Result<JsonValue> {
        let engine = self.inner.clone();
        let lang = language.clone();
        let result = tokio::task::spawn_blocking(move || {
            engine.voice_transcribe_file(&path, lang.as_deref())
        })
        .await
        .map_err(|e| to_napi_error(anyhow::anyhow!("voice_transcribe_file join: {e}")))?
        .map_err(to_napi_error)?;
        serde_json::to_value(json!({
            "text": result.text,
            "tokenIds": result.token_ids,
        }))
        .map_err(to_napi_error)
    }

    /// Prefer async remote transcription path when `[voice].provider` is remote.
    #[napi]
    pub async fn voice_transcribe_file_async(
        &self,
        path: String,
        language: Option<String>,
    ) -> Result<JsonValue> {
        let result = self
            .inner
            .voice_transcribe_file_async(&path, language.as_deref())
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(json!({
            "text": result.text,
            "tokenIds": result.token_ids,
        }))
        .map_err(to_napi_error)
    }

    #[napi]
    pub fn voice_start_stream(&self, language: Option<String>) -> Result<()> {
        self.inner
            .voice_start_stream(language.as_deref())
            .map_err(to_napi_error)
    }

    /// Push 16 kHz mono PCM samples (`number[]` or Float32Array via JS).
    #[napi]
    pub fn voice_push_pcm(&self, samples: Vec<f64>) -> Result<String> {
        let pcm: Vec<f32> = samples.iter().map(|s| *s as f32).collect();
        self.inner.voice_push_pcm(&pcm).map_err(to_napi_error)
    }

    #[napi]
    pub fn voice_end_stream(&self) -> Result<String> {
        self.inner.voice_end_stream().map_err(to_napi_error)
    }

    #[napi]
    pub fn voice_cancel_stream(&self) -> Result<()> {
        self.inner.voice_cancel_stream().map_err(to_napi_error)
    }

    #[napi]
    pub fn subscribe_voice_events(&self) -> NaviNapiVoiceEventStream {
        NaviNapiVoiceEventStream {
            receiver: AsyncMutex::new(self.inner.subscribe_voice_events()),
        }
    }

    // ── Permission Mode ──────────────────────────────────────────────────

    #[napi]
    pub fn get_permission_mode(&self) -> String {
        let mode = self.inner.get_permission_mode();
        match mode {
            navi_sdk::PermissionMode::Restricted => "restricted".to_string(),
            navi_sdk::PermissionMode::AcceptEdits => "accept-edits".to_string(),
            navi_sdk::PermissionMode::Auto => "auto".to_string(),
            navi_sdk::PermissionMode::Yolo => "yolo".to_string(),
        }
    }

    #[napi]
    pub async fn set_permission_mode(&self, mode: String) -> Result<()> {
        let pm = match mode.as_str() {
            "restricted" => navi_sdk::PermissionMode::Restricted,
            "accept-edits" => navi_sdk::PermissionMode::AcceptEdits,
            "auto" => navi_sdk::PermissionMode::Auto,
            "yolo" => navi_sdk::PermissionMode::Yolo,
            _ => {
                return Err(to_napi_error(anyhow::anyhow!(
                    "invalid permission mode: {}",
                    mode
                )));
            }
        };
        self.inner
            .set_permission_mode(pm)
            .await
            .map_err(to_napi_error)
    }

    // ── Session Management ─────────────────────────────────────────────

    #[napi]
    pub fn session_ids(&self) -> Vec<String> {
        self.inner.session_ids()
    }

    #[napi]
    pub fn loaded_config(&self) -> Result<JsonValue> {
        let config = self.inner.loaded_config();
        let mcp_servers: Vec<JsonValue> = config
            .config
            .mcp
            .servers
            .iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "enabled": s.enabled,
                    "command": s.command,
                    "url": s.url,
                    "args": s.args,
                })
            })
            .collect();
        Ok(json!({
            "model": {
                "provider": config.config.model.provider,
                "name": config.config.model.name,
            },
            "attachmentModels": {
                "image": config.config.attachment_models.image,
                "audio": config.config.attachment_models.audio,
                "video": config.config.attachment_models.video,
                "document": config.config.attachment_models.document,
            },
            "backgroundModels": {
                "default": config.config.background_models.default,
                "naming": config.config.background_models.naming,
                "memoryExtraction": config.config.background_models.memory_extraction,
                "repoSearch": config.config.background_models.repo_search,
                "compaction": config.config.background_models.compaction,
                "subagentResearch": config.config.background_models.subagent_research,
                "simpleCodeEdit": config.config.background_models.simple_code_edit,
            },
            "tui": {
                "theme": config.config.tui.theme,
                "showThinking": config.config.tui.show_thinking,
                "fullToolView": config.config.tui.full_tool_view,
                "compactToolVisibleLimit": config.config.tui.compact_tool_visible_limit,
                "thinkingLevel": config.config.tui.thinking_level,
                "yoloMode": config.config.tui.yolo_mode,
                "llmRecap": config.config.tui.llm_recap,
                "desktopNotifications": config.config.tui.desktop_notifications,
            },
            "updates": {
                "checkEnabled": config.config.updates.check_enabled,
                "autoUpdate": config.config.updates.auto_update,
                "includePrerelease": config.config.updates.include_prerelease,
                "checkIntervalHours": config.config.updates.check_interval_hours,
                "repo": config.config.updates.repo,
            },
            "global_config_path": config.global_config_path,
            "project_config_path": config.project_config_path,
            "data_dir": config.data_dir,
            "mcp_servers": mcp_servers,
        }))
    }

    // ── Notifications / self-update ────────────────────────────────────
    // Browser hosts: call `notify` with desktop=false and show Web
    // Notifications from the returned payload (or listen for
    // AgentEvent.NotificationRequested on the event stream).

    /// Show a notification. When `desktop` is true, also attempt an OS toast.
    /// Returns the payload so browser hosts can use the Web Notifications API.
    #[napi]
    pub fn notify(
        &self,
        title: String,
        body: String,
        desktop: Option<bool>,
        urgency: Option<String>,
        category: Option<String>,
    ) -> Result<JsonValue> {
        use navi_core::{NotificationUrgency, NotifyRequest};
        let urgency = match urgency.as_deref() {
            Some("low") => NotificationUrgency::Low,
            Some("critical") => NotificationUrgency::Critical,
            _ => NotificationUrgency::Normal,
        };
        let mut req = NotifyRequest::new(title, body).with_urgency(urgency);
        if let Some(cat) = category {
            req = req.with_category(cat);
        }
        let delivered = self
            .inner
            .notify(req, desktop.unwrap_or(true))
            .map_err(to_napi_error)?;
        serde_json::to_value(delivered).map_err(to_napi_error)
    }

    #[napi]
    pub fn notify_simple(
        &self,
        title: String,
        body: String,
        desktop: Option<bool>,
    ) -> Result<JsonValue> {
        let delivered = self
            .inner
            .notify_simple(title, body, desktop.unwrap_or(true))
            .map_err(to_napi_error)?;
        serde_json::to_value(delivered).map_err(to_napi_error)
    }

    #[napi]
    pub fn open_url(&self, url: String) -> Result<()> {
        self.inner.open_url(&url).map_err(to_napi_error)
    }

    #[napi]
    pub fn app_version(&self) -> String {
        self.inner.app_version()
    }

    #[napi]
    pub async fn check_for_update(&self) -> Result<Option<JsonValue>> {
        match self.inner.check_for_update().await.map_err(to_napi_error)? {
            Some(info) => Ok(Some(serde_json::to_value(info).map_err(to_napi_error)?)),
            None => Ok(None),
        }
    }

    #[napi]
    pub async fn check_for_update_with(
        &self,
        current: String,
        repo: Option<String>,
        include_prerelease: Option<bool>,
    ) -> Result<Option<JsonValue>> {
        match self
            .inner
            .check_for_update_with(
                &current,
                repo.as_deref(),
                include_prerelease.unwrap_or(false),
            )
            .await
            .map_err(to_napi_error)?
        {
            Some(info) => Ok(Some(serde_json::to_value(info).map_err(to_napi_error)?)),
            None => Ok(None),
        }
    }

    #[napi]
    pub async fn apply_update(&self, info: JsonValue) -> Result<()> {
        let info: navi_core::UpdateInfo = serde_json::from_value(info).map_err(to_napi_error)?;
        self.inner.apply_update(&info).await.map_err(to_napi_error)
    }

    #[napi]
    pub fn auto_update_enabled(&self) -> bool {
        self.inner.auto_update_enabled()
    }

    #[napi]
    pub fn set_auto_update(&self, enabled: bool) -> Result<()> {
        self.inner.set_auto_update(enabled).map_err(to_napi_error)
    }
}

#[napi]
impl NaviNapiEventStream {
    #[napi]
    pub async fn next(&self) -> Result<Option<JsonValue>> {
        let mut receiver = self.receiver.lock().await;
        loop {
            match receiver.recv().await {
                Ok(event) => return runtime_event_to_json(event).map(Some),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return Ok(None),
            }
        }
    }
}

#[napi]
impl NaviNapiVoiceEventStream {
    #[napi]
    pub async fn next(&self) -> Result<Option<JsonValue>> {
        let mut receiver = self.receiver.lock().await;
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    return serde_json::to_value(event).map(Some).map_err(to_napi_error);
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return Ok(None),
            }
        }
    }
}

struct JsHostToolHandler {
    callback: JsHostToolCallback,
}

struct JsSessionHooks {
    callbacks: JsHookCallbacks,
}

impl navi_core::SessionHooks for JsSessionHooks {
    fn on_session_start(&self, session_id: &str) {
        emit_hook(
            &self.callbacks.session_start,
            json!({ "sessionId": session_id }),
        );
    }

    fn on_turn_start(&self, session_id: &str, task: &str) {
        emit_hook(
            &self.callbacks.turn_start,
            json!({ "sessionId": session_id, "task": task }),
        );
    }

    fn on_tool_call(&self, invocation: &ToolInvocation) {
        emit_hook(
            &self.callbacks.tool_call,
            json!({ "invocation": invocation }),
        );
    }

    fn on_tool_result(&self, result: &ToolResult) {
        emit_hook(&self.callbacks.tool_result, json!({ "result": result }));
    }

    fn on_turn_end(&self, session_id: &str, output: &str) {
        emit_hook(
            &self.callbacks.turn_end,
            json!({ "sessionId": session_id, "output": output }),
        );
    }

    fn on_session_end(&self, session_id: &str) {
        emit_hook(
            &self.callbacks.session_end,
            json!({ "sessionId": session_id }),
        );
    }
}

#[async_trait]
impl HostToolHandler for JsHostToolHandler {
    async fn invoke(&self, invocation: HostToolInvocation) -> anyhow::Result<SdkHostToolResult> {
        let request = json!({
            "invocationId": invocation.invocation_id,
            "input": invocation.input,
        });
        let promise = self
            .callback
            .call_async(Ok(request))
            .await
            .map_err(|err| anyhow::anyhow!("failed to call JavaScript host tool: {err}"))?;
        let value = promise
            .await
            .map_err(|err| anyhow::anyhow!("JavaScript host tool rejected: {err}"))?;
        parse_host_tool_result(value)
    }
}

fn parse_tool_kind(kind: Option<&str>) -> Result<ToolKind> {
    match kind.unwrap_or("read") {
        "read" => Ok(ToolKind::Read),
        "write" => Ok(ToolKind::Write),
        "command" => Ok(ToolKind::Command),
        "custom" => Ok(ToolKind::Custom),
        other => Err(Error::from_reason(format!(
            "unsupported host tool kind '{other}', expected read, write, command, or custom"
        ))),
    }
}

fn parse_host_tool_result(value: JsonValue) -> anyhow::Result<SdkHostToolResult> {
    let Some(object) = value.as_object() else {
        return Ok(SdkHostToolResult::success(value));
    };
    if !object.contains_key("ok") && !object.contains_key("output") {
        return Ok(SdkHostToolResult::success(value));
    }
    let ok = object
        .get("ok")
        .and_then(JsonValue::as_bool)
        .unwrap_or(true);
    let output = object.get("output").cloned().unwrap_or(JsonValue::Null);
    Ok(SdkHostToolResult { ok, output })
}

fn runtime_event_to_json(event: RuntimeEvent) -> Result<JsonValue> {
    serde_json::to_value(event).map_err(to_napi_error)
}

fn parse_context_packet(value: JsonValue) -> Result<ContextPacket> {
    serde_json::from_value(value).map_err(to_napi_error)
}

fn parse_save_target(value: Option<&str>) -> NaviConfigSaveTarget {
    match value {
        Some("project") => NaviConfigSaveTarget::Project,
        Some("global") => NaviConfigSaveTarget::Global,
        Some("none") => NaviConfigSaveTarget::None,
        _ => NaviConfigSaveTarget::Auto,
    }
}

fn session_info_to_js(info: navi_sdk::NaviSessionInfo) -> JsSessionInfo {
    JsSessionInfo {
        id: info.id,
        project_dir: info.project_dir.display().to_string(),
        model: info.model,
        provider: info.provider,
    }
}

fn parse_permission_mode_str(mode: &str) -> anyhow::Result<navi_sdk::PermissionMode> {
    match mode {
        "restricted" => Ok(navi_sdk::PermissionMode::Restricted),
        "accept-edits" | "accept_edits" => Ok(navi_sdk::PermissionMode::AcceptEdits),
        "auto" => Ok(navi_sdk::PermissionMode::Auto),
        "yolo" => Ok(navi_sdk::PermissionMode::Yolo),
        _ => Err(anyhow::anyhow!(
            "invalid permission mode: {mode}; expected restricted, accept-edits, auto, or yolo"
        )),
    }
}

fn parse_session_request(opts: JsSessionRequest) -> Result<NaviSessionRequest> {
    let context_packets = opts
        .context_packets
        .unwrap_or_default()
        .into_iter()
        .map(parse_context_packet)
        .collect::<Result<Vec<_>>>()?;
    let initial_messages = opts
        .initial_messages
        .unwrap_or_default()
        .into_iter()
        .map(|v| {
            serde_json::from_value::<navi_core::ModelMessage>(v).map_err(to_napi_error)
        })
        .collect::<Result<Vec<_>>>()?;
    let initial_events = opts
        .initial_events
        .unwrap_or_default()
        .into_iter()
        .map(|v| serde_json::from_value::<navi_core::AgentEvent>(v).map_err(to_napi_error))
        .collect::<Result<Vec<_>>>()?;
    let initial_goal = match opts.initial_goal {
        Some(v) => Some(serde_json::from_value(v).map_err(to_napi_error)?),
        None => None,
    };
    Ok(NaviSessionRequest {
        session_id: opts.session_id,
        project_dir: opts.project_dir.map(std::path::PathBuf::from),
        context_packets,
        active_skills: opts.active_skills.unwrap_or_default(),
        initial_messages,
        initial_events,
        initial_created_at: opts.initial_created_at.map(|n| n as u64),
        initial_updated_at: opts.initial_updated_at.map(|n| n as u64),
        initial_goal,
    })
}

fn parse_loaded_config_payload(
    value: JsonValue,
) -> anyhow::Result<navi_sdk::LoadedConfig> {
    let default_data_dir = navi_sdk::LoadedConfig::default().data_dir;
    // Full envelope: { config, dataDir?, globalConfigPath?, projectConfigPath? }
    if value.get("config").is_some()
        || value.get("dataDir").is_some()
        || value.get("data_dir").is_some()
    {
        let config_val = value
            .get("config")
            .cloned()
            .unwrap_or_else(|| value.clone());
        // If the top-level object has config-like fields but no nested config,
        // treat the whole object as NaviConfig when dataDir is the only extra.
        let config: navi_sdk::NaviConfig = if value.get("config").is_some() {
            serde_json::from_value(config_val)
                .map_err(|e| anyhow::anyhow!("invalid loadedConfig.config: {e}"))?
        } else {
            // Strip host-only keys then parse remaining as NaviConfig.
            let mut bare = value.clone();
            if let Some(obj) = bare.as_object_mut() {
                obj.remove("dataDir");
                obj.remove("data_dir");
                obj.remove("globalConfigPath");
                obj.remove("global_config_path");
                obj.remove("projectConfigPath");
                obj.remove("project_config_path");
            }
            serde_json::from_value(bare)
                .map_err(|e| anyhow::anyhow!("invalid loadedConfig payload: {e}"))?
        };
        let data_dir = value
            .get("dataDir")
            .or_else(|| value.get("data_dir"))
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from)
            .unwrap_or(default_data_dir);
        let global_config_path = value
            .get("globalConfigPath")
            .or_else(|| value.get("global_config_path"))
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from);
        let project_config_path = value
            .get("projectConfigPath")
            .or_else(|| value.get("project_config_path"))
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from);
        return Ok(navi_sdk::LoadedConfig {
            config,
            global_config_path,
            project_config_path,
            data_dir,
        });
    }
    // Bare NaviConfig
    let config: navi_sdk::NaviConfig = serde_json::from_value(value)
        .map_err(|e| anyhow::anyhow!("invalid loadedConfig payload: {e}"))?;
    Ok(navi_sdk::LoadedConfig {
        config,
        global_config_path: None,
        project_config_path: None,
        data_dir: default_data_dir,
    })
}

fn provider_model_entry(name: String) -> ProviderModelConfig {
    ProviderModelConfig {
        name,
        task_size: None,
        context_window_tokens: None,
        max_output_tokens: None,
        recommended_temperature: None,
        supports_thinking: None,
        reasoning_levels: Vec::new(),
        default_reasoning_effort: None,
        supports_images: None,
        supports_audio: None,
        supports_video: None,
        supports_documents: None,
        tool_prompt_manifest: None,
        pricing_input_per_1m: None,
        pricing_output_per_1m: None,
    }
}

fn provider_from_upsert(provider: JsProviderUpsert) -> anyhow::Result<ProviderConfig> {
    if provider.id.trim().is_empty() {
        return Err(anyhow::anyhow!("provider id must not be empty"));
    }
    let kind = match provider.kind.as_deref().unwrap_or("openai-chat-completions") {
        "openai-responses" | "openai_responses" | "responses" => ProviderKind::OpenAiResponses,
        "openai-chat-completions" | "openai_chat_completions" | "chat" | "ollama" => {
            ProviderKind::OpenAiChatCompletions
        }
        other => {
            return Err(anyhow::anyhow!(
                "unsupported provider kind `{other}`; use openai-chat-completions or openai-responses"
            ));
        }
    };
    let models = provider
        .models
        .unwrap_or_default()
        .into_iter()
        .map(provider_model_entry)
        .collect();
    Ok(ProviderConfig {
        id: provider.id.clone(),
        label: provider.label.unwrap_or_else(|| provider.id.clone()),
        description: provider.description.unwrap_or_default(),
        kind,
        api_key_env: provider.api_key_env.unwrap_or_else(|| {
            format!(
                "{}_API_KEY",
                provider.id.to_ascii_uppercase().replace('-', "_")
            )
        }),
        base_url: provider.base_url,
        models,
        ..Default::default()
    })
}

fn parse_thinking_config(value: &str) -> Result<ThinkingConfig> {
    // Prefer shared registry/config parser (includes "on" → medium for binary effort).
    if let Some(level) = navi_sdk::parse_reasoning_level(value) {
        return Ok(level);
    }
    Err(Error::from_reason(format!(
        "unsupported effort/thinking config '{value}', expected max, high, medium, low, off, or on (legacy adaptive maps to max)"
    )))
}

fn build_hook_callback(handler: Function<JsonValue, UnknownReturnValue>) -> Result<JsHookCallback> {
    handler
        .build_threadsafe_function::<JsonValue>()
        .callee_handled::<true>()
        .weak::<false>()
        .build()
        .map_err(to_napi_error)
}

fn emit_hook(callback: &Option<Arc<JsHookCallback>>, payload: JsonValue) {
    if let Some(callback) = callback {
        let _ = callback.call(Ok(payload), ThreadsafeFunctionCallMode::NonBlocking);
    }
}

fn to_napi_error(error: impl std::fmt::Display) -> Error {
    Error::from_reason(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_tool_result_contract() {
        let result = parse_host_tool_result(json!({
            "ok": false,
            "output": { "reason": "missing material" }
        }))
        .expect("result");

        assert!(!result.ok);
        assert_eq!(result.output["reason"], "missing material");
    }

    #[test]
    fn plain_callback_value_is_success_output() {
        let result = parse_host_tool_result(json!({ "value": 42 })).expect("result");

        assert!(result.ok);
        assert_eq!(result.output["value"], 42);
    }

    #[test]
    fn parses_tool_kind_strings() {
        assert_eq!(parse_tool_kind(None).expect("default"), ToolKind::Read);
        assert_eq!(
            parse_tool_kind(Some("command")).expect("command"),
            ToolKind::Command
        );
        assert!(parse_tool_kind(Some("unknown")).is_err());
    }

    #[test]
    fn runtime_event_serializes_for_js_clients() {
        let event = RuntimeEvent::new(navi_core::RuntimeEventKind::AssistantDelta {
            text: "oi".to_string(),
        });

        let value = runtime_event_to_json(event).expect("json");

        assert_eq!(value["version"], 1);
        assert_eq!(value["kind"]["AssistantDelta"]["text"], "oi");
    }

    #[test]
    fn parses_context_packet_from_json() {
        let packet = parse_context_packet(json!({
            "source": "StudyBlock",
            "title": "Limites",
            "content": "definicao formal",
            "priority": 3,
        }))
        .expect("packet");

        assert_eq!(packet.title.as_deref(), Some("Limites"));
        assert_eq!(packet.content, "definicao formal");
        assert_eq!(packet.priority, 3);
    }

    #[test]
    fn hook_callbacks_default_to_empty() {
        assert!(JsHookCallbacks::default().is_empty());
    }

    #[test]
    fn parse_save_target_auto_default() {
        assert!(matches!(
            parse_save_target(None),
            NaviConfigSaveTarget::Auto
        ));
    }

    #[test]
    fn parse_save_target_project() {
        assert!(matches!(
            parse_save_target(Some("project")),
            NaviConfigSaveTarget::Project
        ));
    }

    #[test]
    fn parse_save_target_global() {
        assert!(matches!(
            parse_save_target(Some("global")),
            NaviConfigSaveTarget::Global
        ));
    }

    #[test]
    fn parse_save_target_none() {
        assert!(matches!(
            parse_save_target(Some("none")),
            NaviConfigSaveTarget::None
        ));
    }

    #[test]
    fn parse_save_target_auto_unknown() {
        assert!(matches!(
            parse_save_target(Some("unknown")),
            NaviConfigSaveTarget::Auto
        ));
    }

    #[test]
    fn parse_thinking_config_parses_all_levels() {
        assert_eq!(parse_thinking_config("max").unwrap(), ThinkingConfig::Max);
        assert_eq!(parse_thinking_config("high").unwrap(), ThinkingConfig::High);
        assert_eq!(
            parse_thinking_config("medium").unwrap(),
            ThinkingConfig::Medium
        );
        assert_eq!(parse_thinking_config("low").unwrap(), ThinkingConfig::Low);
        assert_eq!(parse_thinking_config("off").unwrap(), ThinkingConfig::Off);
        // Legacy adaptive maps to max (highest fixed effort).
        assert_eq!(
            parse_thinking_config("adaptive").unwrap(),
            ThinkingConfig::Max
        );
        // Binary effort "thinking on" aliases to max.
        assert_eq!(parse_thinking_config("on").unwrap(), ThinkingConfig::Max);
    }

    #[test]
    fn parse_thinking_config_rejects_invalid() {
        assert!(parse_thinking_config("turbo").is_err());
    }
}
