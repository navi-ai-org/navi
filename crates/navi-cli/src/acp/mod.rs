mod events;
mod handlers;
mod permissions;
mod prompt_runner;
mod schema;
mod state;

use agent_client_protocol::schema::CancelNotification;
use agent_client_protocol::{
    Agent, Client, ConnectionTo, Dispatch, Stdio, on_receive_dispatch, on_receive_notification,
    on_receive_request,
};
use navi_sdk::LoadedConfig;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use state::AcpState;

pub async fn run_acp_server(
    loaded_config: LoadedConfig,
    default_project_dir: PathBuf,
) -> anyhow::Result<()> {
    let engine = navi_sdk::NaviEngineBuilder::from_project(default_project_dir.clone())
        .loaded_config(loaded_config)
        .build()?;
    let state = AcpState {
        engine,
        default_project_dir,
        sessions: Arc::new(Mutex::new(HashMap::new())),
    };

    Agent
        .builder()
        .name("navi")
        .on_receive_request(handlers::handle_initialize, on_receive_request!())
        .on_receive_request(
            {
                let state = state.clone();
                async move |request, responder, _connection| {
                    responder.respond(handlers::handle_new_session(state.clone(), request).await?)
                }
            },
            on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |request, responder, connection| {
                    handlers::handle_prompt(state.clone(), request, responder, connection).await
                }
            },
            on_receive_request!(),
        )
        .on_receive_notification(
            {
                let state = state.clone();
                async move |notification, _connection| {
                    handle_cancel(state.clone(), notification).await
                }
            },
            on_receive_notification!(),
        )
        .on_receive_dispatch(
            async move |message: Dispatch, cx: ConnectionTo<Client>| {
                message.respond_with_error(
                    agent_client_protocol::util::internal_error("unhandled ACP message"),
                    cx,
                )
            },
            on_receive_dispatch!(),
        )
        .connect_to(Stdio::new())
        .await
        .map_err(|e| anyhow::anyhow!("ACP server failed: {e:?}"))
}

async fn handle_cancel(
    state: AcpState,
    notification: CancelNotification,
) -> agent_client_protocol::Result<()> {
    handlers::handle_cancel(state, notification).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::{ContentBlock, TextContent, ToolCallStatus, ToolKind};
    use navi_sdk::ToolResult;

    #[test]
    fn maps_builtin_tools_to_acp_kinds() {
        assert_eq!(schema::acp_tool_kind("read_file"), ToolKind::Read);
        assert_eq!(schema::acp_tool_kind("write_file"), ToolKind::Edit);
        assert_eq!(schema::acp_tool_kind("grep"), ToolKind::Search);
        assert_eq!(schema::acp_tool_kind("bash"), ToolKind::Execute);
        assert_eq!(schema::acp_tool_kind("custom"), ToolKind::Other);
    }

    #[test]
    fn extracts_text_prompt_blocks() {
        let prompt = schema::prompt_to_text(vec![
            ContentBlock::Text(TextContent::new("first")),
            ContentBlock::Text(TextContent::new("second")),
        ]);

        assert_eq!(prompt, "first\n\nsecond");
    }

    #[test]
    fn serializes_tool_result_update() {
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!({"status": "ok"}),
        };

        let update = schema::tool_result_update(&result);

        assert_eq!(update.tool_call_id.0.as_ref(), "call-1");
        assert_eq!(update.fields.status, Some(ToolCallStatus::Completed));
        assert!(update.fields.content.is_some());
        assert!(update.fields.raw_output.is_some());
    }
}
