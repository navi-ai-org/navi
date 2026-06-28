use std::sync::Arc;

use async_trait::async_trait;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::ThreadsafeFunction;
use napi_derive::napi;
use navi_core::ToolKind;
use navi_sdk::{
    HostToolDefinition, HostToolHandler, HostToolInvocation, NaviEngineBuilder, NaviSessionRequest,
    NaviTurnRequest, SdkHostTool, SdkHostToolResult,
};
use serde_json::{Value as JsonValue, json};

type JsHostToolCallback = ThreadsafeFunction<JsonValue, Promise<JsonValue>>;

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

#[napi]
pub struct NaviNapiEngine {
    inner: navi_sdk::NaviEngine,
}

#[napi]
pub struct NaviNapiEngineBuilder {
    project_dir: String,
    learning_tutor: bool,
    host_tools: Vec<Arc<dyn navi_core::Tool>>,
}

#[napi]
impl NaviNapiEngineBuilder {
    #[napi(constructor)]
    pub fn new(project_dir: String) -> Self {
        Self {
            project_dir,
            learning_tutor: false,
            host_tools: Vec::new(),
        }
    }

    #[napi]
    pub fn set_learning_tutor(&mut self, enabled: Option<bool>) {
        self.learning_tutor = enabled.unwrap_or(true);
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
        if self.learning_tutor {
            builder = builder.learning_tutor();
        }
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
    pub async fn send_turn(&self, session_id: String, message: String) -> Result<JsTurnResponse> {
        let response = self
            .inner
            .send_turn(NaviTurnRequest {
                session_id,
                message,
                content_parts: Vec::new(),
                context_packets: Vec::new(),
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
}

struct JsHostToolHandler {
    callback: JsHostToolCallback,
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
}
