use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
    ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue,
};
use napi_derive::napi;

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
    AgentEvent, ContentPart, ContextPacket, LearningHarness, LearningHarnessConfig, ModelMessage,
    RuntimeComponents, StudyCompactionConfig, StudyCompactionStrategy, ThinkingConfig,
    ToolInvocation, ToolKind, ToolResult, TutorPromptBuilder, TutorPromptOptions,
};
use navi_sdk::{
    ApprovalDecision, HostToolDefinition, HostToolHandler, HostToolInvocation,
    NaviConfigSaveTarget, NaviEngineBuilder, NaviModelSelectionRequest, NaviSessionRequest,
    NaviTurnRequest, QuestionResponse, RuntimeEvent, SdkHostTool, SdkHostToolResult,
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
pub struct JsLearningRuntimeConfig {
    pub max_consecutive_errors: Option<u32>,
    pub stop_on_repeated_tool: Option<bool>,
    pub compact_observation_max_bytes: Option<u32>,
    pub role: Option<String>,
    pub style: Option<String>,
    pub language: Option<String>,
    pub keep_all_assessments: Option<bool>,
    pub exempt_tool_names: Option<Vec<String>>,
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
pub struct NaviNapiEngineBuilder {
    project_dir: String,
    learning_tutor: bool,
    learning_config: Option<JsLearningRuntimeConfig>,
    hooks: JsHookCallbacks,
    host_tools: Vec<Arc<dyn navi_core::Tool>>,
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
            learning_tutor: false,
            learning_config: None,
            hooks: JsHookCallbacks::default(),
            host_tools: Vec::new(),
        }
    }

    #[napi]
    pub fn set_learning_tutor(&mut self, enabled: Option<bool>) {
        self.learning_tutor = enabled.unwrap_or(true);
    }

    #[napi(js_name = "configureLearning")]
    pub fn configure_learning(&mut self, config: JsLearningRuntimeConfig) {
        self.learning_tutor = true;
        self.learning_config = Some(config);
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
        let mut components = if self.learning_tutor {
            learning_components(self.learning_config.as_ref())
        } else {
            RuntimeComponents::default()
        };
        if !self.hooks.is_empty() {
            components.hooks = Arc::new(JsSessionHooks {
                callbacks: self.hooks.clone(),
            });
        }
        builder = builder.runtime_components(components);
        for tool in self.host_tools.drain(..) {
            builder = builder.host_tool(tool);
        }
        let inner = builder.build().map_err(to_napi_error)?;
        Ok(NaviNapiEngine { inner })
    }
}

#[napi]
impl NaviNapiEngine {
    #[napi(constructor)]
    pub fn new(project_dir: String, learning_tutor: Option<bool>) -> Result<Self> {
        let mut builder = NaviEngineBuilder::from_project(project_dir);
        if learning_tutor.unwrap_or(false) {
            builder = builder.learning_tutor();
        }
        let inner = builder.build().map_err(to_napi_error)?;
        Ok(Self { inner })
    }

    #[napi(factory)]
    pub fn learning_tutor(project_dir: String) -> Result<Self> {
        let inner = NaviEngineBuilder::from_project(project_dir)
            .learning_tutor()
            .build()
            .map_err(to_napi_error)?;
        Ok(Self { inner })
    }

    #[napi]
    pub async fn start_session(&self, session_id: Option<String>) -> Result<JsSessionInfo> {
        let info = self
            .inner
            .start_session(NaviSessionRequest {
                session_id,
                ..NaviSessionRequest::default()
            })
            .await
            .map_err(to_napi_error)?;
        Ok(JsSessionInfo {
            id: info.id,
            project_dir: info.project_dir.display().to_string(),
            model: info.model,
            provider: info.provider,
        })
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

    // ── Questions ──────────────────────────────────────────────────────

    #[napi]
    pub async fn resolve_question(&self, session_id: String, response: JsonValue) -> Result<bool> {
        let qr: QuestionResponse = serde_json::from_value(response).map_err(to_napi_error)?;
        self.inner
            .resolve_question(&session_id, qr)
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

    #[napi]
    pub async fn load_saved_session(&self, session_id: String) -> Result<JsonValue> {
        let snapshot = self
            .inner
            .load_saved_session_async(&session_id)
            .await
            .map_err(to_napi_error)?;
        self.inner
            .start_session(NaviSessionRequest {
                project_dir: Some(snapshot.project.clone()),
                session_id: Some(session_id),
                initial_messages: initial_messages_from_events(&snapshot.events),
                initial_events: snapshot.events.clone(),
                initial_created_at: Some(snapshot.created_at),
                initial_updated_at: Some(snapshot.updated_at),
                ..NaviSessionRequest::default()
            })
            .await
            .map_err(to_napi_error)?;
        serde_json::to_value(&snapshot).map_err(to_napi_error)
    }

    #[napi]
    pub async fn delete_saved_session(&self, session_id: String) -> Result<bool> {
        self.inner
            .delete_saved_session_async(&session_id)
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
        Ok(json!({
            "model": {
                "provider": config.config.model.provider,
                "name": config.config.model.name,
            },
            "global_config_path": config.global_config_path,
            "project_config_path": config.project_config_path,
            "data_dir": config.data_dir,
        }))
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

fn parse_thinking_config(value: &str) -> Result<ThinkingConfig> {
    match value {
        "max" => Ok(ThinkingConfig::Max),
        "high" => Ok(ThinkingConfig::High),
        "medium" => Ok(ThinkingConfig::Medium),
        "low" => Ok(ThinkingConfig::Low),
        "off" => Ok(ThinkingConfig::Off),
        "adaptive" => Ok(ThinkingConfig::Adaptive),
        other => Err(Error::from_reason(format!(
            "unsupported thinking config '{other}', expected max, high, medium, low, off, or adaptive"
        ))),
    }
}

fn initial_messages_from_events(events: &[AgentEvent]) -> Vec<ModelMessage> {
    let mut messages = Vec::new();
    let mut tool_names = HashMap::new();

    for event in events {
        match event {
            AgentEvent::UserTaskSubmitted {
                text,
                content_parts,
            } => {
                if content_parts.is_empty() {
                    messages.push(ModelMessage::user(text.clone()));
                } else {
                    messages.push(ModelMessage::user_multimodal(
                        text.clone(),
                        content_parts.clone(),
                    ));
                }
            }
            AgentEvent::ModelOutput { text, thinking } => {
                messages.push(ModelMessage::assistant_with_thinking(
                    text.clone(),
                    thinking.clone(),
                ));
            }
            AgentEvent::ToolRequested(invocation) => {
                tool_names.insert(invocation.id.clone(), invocation.tool_name.clone());
                messages.push(ModelMessage::assistant_tool_call(invocation.clone()));
            }
            AgentEvent::ToolCompleted(result) => {
                let tool_name = tool_names
                    .get(&result.invocation_id)
                    .cloned()
                    .unwrap_or_else(|| "tool".to_string());
                let output = result
                    .output
                    .as_str()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| result.output.to_string());
                messages.push(ModelMessage::tool_result(
                    result.invocation_id.clone(),
                    tool_name,
                    output,
                ));
            }
            _ => {}
        }
    }

    messages
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

fn learning_components(config: Option<&JsLearningRuntimeConfig>) -> RuntimeComponents {
    let mut components = navi_core::learning_runtime_components();
    let Some(config) = config else {
        return components;
    };

    let harness_defaults = LearningHarnessConfig::default();
    components.harness = Arc::new(LearningHarness::new(LearningHarnessConfig {
        max_consecutive_errors: config
            .max_consecutive_errors
            .map(|value| value as usize)
            .unwrap_or(harness_defaults.max_consecutive_errors),
        stop_on_repeated_tool: config
            .stop_on_repeated_tool
            .unwrap_or(harness_defaults.stop_on_repeated_tool),
        compact_observation_max_bytes: config
            .compact_observation_max_bytes
            .map(|value| value as usize)
            .or(harness_defaults.compact_observation_max_bytes),
    }));

    let prompt_defaults = TutorPromptOptions::default();
    components.prompt = Arc::new(TutorPromptBuilder::new(TutorPromptOptions {
        role: config
            .role
            .clone()
            .unwrap_or_else(|| prompt_defaults.role.clone()),
        style: config
            .style
            .clone()
            .unwrap_or_else(|| prompt_defaults.style.clone()),
        language: config
            .language
            .clone()
            .unwrap_or_else(|| prompt_defaults.language.clone()),
    }));

    let compaction_defaults = StudyCompactionConfig::default();
    components.compaction = Arc::new(StudyCompactionStrategy::new(StudyCompactionConfig {
        keep_all_assessments: config
            .keep_all_assessments
            .unwrap_or(compaction_defaults.keep_all_assessments),
        exempt_tool_names: config
            .exempt_tool_names
            .clone()
            .unwrap_or(compaction_defaults.exempt_tool_names),
    }));

    components
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
    fn learning_components_accept_structured_js_options() {
        let _components = learning_components(Some(&JsLearningRuntimeConfig {
            max_consecutive_errors: Some(7),
            stop_on_repeated_tool: Some(true),
            compact_observation_max_bytes: Some(4096),
            role: Some("professor".to_string()),
            style: Some("socratico".to_string()),
            language: Some("pt-BR".to_string()),
            keep_all_assessments: Some(true),
            exempt_tool_names: Some(vec!["questionario".to_string()]),
        }));
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
        assert_eq!(
            parse_thinking_config("adaptive").unwrap(),
            ThinkingConfig::Adaptive
        );
    }

    #[test]
    fn parse_thinking_config_rejects_invalid() {
        assert!(parse_thinking_config("turbo").is_err());
    }
}
