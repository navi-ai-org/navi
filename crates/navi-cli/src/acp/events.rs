use agent_client_protocol::schema::{
    ContentBlock, ContentChunk, SessionNotification, SessionUpdate, TextContent, ToolCall,
    ToolCallStatus,
};
use agent_client_protocol::{Client, ConnectionTo};
use anyhow::Result;
use navi_sdk::{RuntimeEvent, RuntimeEventKind, ToolInvocation, ToolResult};

use crate::acp::schema::{acp_error_to_anyhow, acp_tool_kind, tool_result_update};

pub(crate) fn forward_runtime_event(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    event: &RuntimeEvent,
) -> Result<bool> {
    match &event.kind {
        RuntimeEventKind::AssistantDelta { text } => {
            send_text_update(connection, session_id, text.clone(), false)?;
            Ok(true)
        }
        RuntimeEventKind::AssistantThinkingDelta { text } => {
            send_text_update(connection, session_id, text.clone(), true)?;
            Ok(false)
        }
        RuntimeEventKind::ToolRequested(invocation) => {
            send_tool_requested(connection, session_id, invocation)?;
            Ok(false)
        }
        RuntimeEventKind::ToolCompleted(result) => {
            send_tool_completed(connection, session_id, result)?;
            Ok(false)
        }
        _ => Ok(false),
    }
}

pub(crate) fn send_tool_requested(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    invocation: &ToolInvocation,
) -> Result<()> {
    connection
        .send_notification(SessionNotification::new(
            session_id.to_string(),
            SessionUpdate::ToolCall(
                ToolCall::new(invocation.id.clone(), invocation.tool_name.clone())
                    .kind(acp_tool_kind(&invocation.tool_name))
                    .status(ToolCallStatus::InProgress)
                    .raw_input(invocation.input.clone()),
            ),
        ))
        .map_err(acp_error_to_anyhow)
}

pub(crate) fn send_tool_completed(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    result: &ToolResult,
) -> Result<()> {
    connection
        .send_notification(SessionNotification::new(
            session_id.to_string(),
            SessionUpdate::ToolCallUpdate(tool_result_update(result)),
        ))
        .map_err(acp_error_to_anyhow)
}

pub(crate) fn send_text_update(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    text: String,
    thinking: bool,
) -> Result<()> {
    let update = if thinking {
        SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        ))))
    } else {
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        ))))
    };
    connection
        .send_notification(SessionNotification::new(session_id.to_string(), update))
        .map_err(acp_error_to_anyhow)
}
