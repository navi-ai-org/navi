//! NAVI Agent Client Protocol (ACP) client.
//!
//! Spawns external ACP agent servers over JSON-RPC stdio and delegates full
//! turns. This is **not** a [`navi_core::ModelProvider`] — see `DESIGN.md`.

mod client;
mod error;
mod event;
mod peer;
mod process;
mod transport;
mod types;

pub use client::{AcpClient, AcpConnectOptions, PermissionHandler};
pub use error::{AcpError, Result};
pub use event::AcpEvent;
pub use peer::{AcpAgentSpec, AcpTurnResult, ExternalAgentPeer, SpawnedAcpPeer, event_label};
pub use process::AcpProcessConfig;
pub use transport::{
    InboundMessage, JsonRpcErrorObject, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, JsonRpcTransport, decode_message_line, encode_request_line,
};
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, duplex};

    #[test]
    fn parses_agent_message_chunk_update() {
        let raw = json!({
            "sessionId": "sess_1",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": "hello" }
            }
        });
        let note: SessionNotification = serde_json::from_value(raw).unwrap();
        assert_eq!(note.session_id, "sess_1");
        assert_eq!(note.update.agent_text(), Some("hello"));
    }

    #[test]
    fn parses_tool_call_update() {
        let raw = json!({
            "sessionId": "s",
            "update": {
                "sessionUpdate": "tool_call",
                "toolCallId": "call_1",
                "title": "Read file",
                "kind": "read",
                "status": "pending"
            }
        });
        let note: SessionNotification = serde_json::from_value(raw).unwrap();
        match note.update {
            SessionUpdate::ToolCall {
                tool_call_id,
                title,
                ..
            } => {
                assert_eq!(tool_call_id, "call_1");
                assert_eq!(title.as_deref(), Some("Read file"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_stop_reason() {
        let raw = json!({ "stopReason": "end_turn" });
        let result: PromptResult = serde_json::from_value(raw).unwrap();
        assert_eq!(result.stop_reason, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn mock_agent_full_lifecycle() {
        let (client_read, mut server_write) = duplex(16384);
        let (server_read, client_write) = duplex(16384);

        // Minimal mock ACP server.
        tokio::spawn(async move {
            let mut lines = BufReader::new(server_read).lines();
            let mut session = String::new();
            while let Ok(Some(line)) = lines.next_line().await {
                let v: serde_json::Value = serde_json::from_str(&line).unwrap();
                let id = v.get("id").cloned();
                let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
                match method {
                    "initialize" => {
                        let resp = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "protocolVersion": 1,
                                "agentCapabilities": {},
                                "authMethods": [],
                                "agentInfo": { "name": "mock", "version": "0.0.1" }
                            }
                        });
                        write_line(&mut server_write, &resp).await;
                    }
                    "session/new" => {
                        session = "sess_mock_1".into();
                        let resp = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "sessionId": session }
                        });
                        write_line(&mut server_write, &resp).await;
                    }
                    "session/prompt" => {
                        // Stream two text chunks, then complete.
                        let note1 = json!({
                            "jsonrpc": "2.0",
                            "method": "session/update",
                            "params": {
                                "sessionId": session,
                                "update": {
                                    "sessionUpdate": "agent_message_chunk",
                                    "content": { "type": "text", "text": "Hello " }
                                }
                            }
                        });
                        let note2 = json!({
                            "jsonrpc": "2.0",
                            "method": "session/update",
                            "params": {
                                "sessionId": session,
                                "update": {
                                    "sessionUpdate": "agent_message_chunk",
                                    "content": { "type": "text", "text": "world" }
                                }
                            }
                        });
                        write_line(&mut server_write, &note1).await;
                        write_line(&mut server_write, &note2).await;
                        let resp = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "stopReason": "end_turn" }
                        });
                        write_line(&mut server_write, &resp).await;
                    }
                    "session/cancel" => {
                        // notification — no response
                    }
                    _ => {
                        if let Some(id) = id {
                            let resp = json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": -32601, "message": format!("unknown {method}") }
                            });
                            write_line(&mut server_write, &resp).await;
                        }
                    }
                }
            }
        });

        let mut client = AcpClient::connect_io_for_test(
            client_read,
            client_write,
            PermissionHandler::AutoApprove,
        )
        .await
        .expect("connect");
        assert_eq!(
            client.initialize_result().agent_info.as_ref().unwrap().name,
            "mock"
        );

        let session = client.session_new("/tmp").await.expect("session");
        assert_eq!(session.session_id, "sess_mock_1");

        let (result, text) = client
            .prompt_collect_text(&session.session_id, "hi")
            .await
            .expect("prompt");
        assert_eq!(result.stop_reason, StopReason::EndTurn);
        assert_eq!(text, "Hello world");

        client
            .session_cancel(&session.session_id)
            .await
            .expect("cancel");
    }

    #[tokio::test]
    async fn permission_request_auto_approved() {
        let (client_read, mut server_write) = duplex(16384);
        let (server_read, client_write) = duplex(16384);

        let (perm_tx, mut perm_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

        let server = tokio::spawn(async move {
            let mut lines = BufReader::new(server_read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let v: serde_json::Value = serde_json::from_str(&line).unwrap();
                // Client response to our permission request.
                if v.get("method").is_none() {
                    if let Some(outcome) = v
                        .pointer("/result/outcome/optionId")
                        .and_then(|x| x.as_str())
                    {
                        let _ = perm_tx.send(outcome.to_string());
                    } else if v
                        .pointer("/result/outcome/outcome")
                        .and_then(|x| x.as_str())
                        == Some("selected")
                    {
                        if let Some(oid) = v
                            .pointer("/result/outcome/optionId")
                            .and_then(|x| x.as_str())
                        {
                            let _ = perm_tx.send(oid.to_string());
                        } else {
                            let _ = perm_tx.send("selected".into());
                        }
                    }
                    continue;
                }
                let id = v.get("id").cloned();
                let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
                if method == "initialize" {
                    write_line(
                        &mut server_write,
                        &json!({
                            "jsonrpc":"2.0","id":id,
                            "result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[]}
                        }),
                    )
                    .await;
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    write_line(
                        &mut server_write,
                        &json!({
                            "jsonrpc":"2.0",
                            "id": 42,
                            "method":"session/request_permission",
                            "params":{
                                "sessionId":"s",
                                "toolCall":{"toolCallId":"c1"},
                                "options":[
                                    {"optionId":"allow-once","name":"Allow","kind":"allow_once"},
                                    {"optionId":"reject-once","name":"Reject","kind":"reject_once"}
                                ]
                            }
                        }),
                    )
                    .await;
                }
            }
        });

        let mut client = AcpClient::connect_io_for_test(
            client_read,
            client_write,
            PermissionHandler::AutoApprove,
        )
        .await
        .expect("connect");

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), perm_rx.recv())
            .await
            .expect("permission response timeout")
            .expect("permission channel closed");
        assert_eq!(outcome, "allow-once");

        let mut got_perm = false;
        for _ in 0..10 {
            match client.try_recv_event() {
                Some(AcpEvent::PermissionRequired { .. }) => {
                    got_perm = true;
                    break;
                }
                Some(_) => continue,
                None => {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            }
        }
        assert!(got_perm, "expected PermissionRequired event");

        drop(client);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), server).await;
    }

    async fn write_line(w: &mut (impl AsyncWriteExt + Unpin), value: &serde_json::Value) {
        let mut bytes = serde_json::to_vec(value).unwrap();
        bytes.push(b'\n');
        w.write_all(&bytes).await.unwrap();
        w.flush().await.unwrap();
    }
}
