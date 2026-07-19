use crate::error::{AcpError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcErrorObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcErrorObject {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    Response(JsonRpcResponse),
    Request(JsonRpcRequest),
    Notification(JsonRpcNotification),
}

/// Parse a JSON-RPC line. Prefer explicit field discrimination so requests
/// (which have `method` + `id`) are not misread as empty responses.
pub fn parse_jsonrpc_line(line: &str) -> Result<JsonRpcMessage> {
    let value: Value = serde_json::from_str(line)?;
    let obj = value
        .as_object()
        .ok_or_else(|| AcpError::Protocol("JSON-RPC message must be an object".into()))?;

    if obj.contains_key("method") {
        if obj.contains_key("id") {
            Ok(JsonRpcMessage::Request(serde_json::from_value(value)?))
        } else {
            Ok(JsonRpcMessage::Notification(serde_json::from_value(value)?))
        }
    } else if obj.contains_key("result") || obj.contains_key("error") {
        Ok(JsonRpcMessage::Response(serde_json::from_value(value)?))
    } else if obj.contains_key("id") {
        // Empty success-style response without result (some servers send {}).
        Ok(JsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: obj
                .get("jsonrpc")
                .and_then(|v| v.as_str())
                .unwrap_or("2.0")
                .to_string(),
            id: obj.get("id").cloned().unwrap_or(Value::Null),
            result: Some(Value::Null),
            error: None,
        }))
    } else {
        Err(AcpError::Protocol(format!(
            "unrecognized JSON-RPC message: {line}"
        )))
    }
}

/// Inbound messages that are not responses to client requests.
#[derive(Debug, Clone)]
pub enum InboundMessage {
    Notification {
        method: String,
        params: Value,
    },
    Request {
        id: Value,
        method: String,
        params: Value,
    },
}

/// Pure JSON-RPC 2.0 framing + id routing over async line-delimited streams.
pub struct JsonRpcTransport {
    next_id: AtomicU64,
    write: Mutex<Box<dyn AsyncWrite + Unpin + Send>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<std::result::Result<Value, AcpError>>>>>,
    closed: Arc<RwLock<bool>>,
}

impl JsonRpcTransport {
    /// Builds a transport from already-opened read/write halves.
    /// Spawns a background reader that demuxes responses vs notifications/requests.
    ///
    /// The reader task is detached on drop of the returned transport (tokio
    /// `JoinHandle` detach semantics); pending RPCs are failed with
    /// [`AcpError::TransportClosed`] when the stream ends.
    pub fn new(
        reader: impl tokio::io::AsyncRead + Unpin + Send + 'static,
        writer: impl AsyncWrite + Unpin + Send + 'static,
    ) -> (Self, mpsc::UnboundedReceiver<InboundMessage>) {
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        let pending: Arc<
            Mutex<HashMap<u64, oneshot::Sender<std::result::Result<Value, AcpError>>>>,
        > = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(RwLock::new(false));

        let transport = Self {
            next_id: AtomicU64::new(0),
            write: Mutex::new(Box::new(writer)),
            pending: pending.clone(),
            closed: closed.clone(),
        };

        // Detached reader: dropping JoinHandle does not abort the task.
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        if let Err(err) = dispatch_line(&line, &pending, &inbound_tx).await {
                            tracing::warn!(error = %err, "ACP transport dispatch error");
                        }
                    }
                    Ok(None) => break,
                    Err(err) => {
                        tracing::warn!(error = %err, "ACP transport read error");
                        break;
                    }
                }
            }
            *closed.write().await = true;
            let mut pending = pending.lock().await;
            for (_, tx) in pending.drain() {
                let _ = tx.send(Err(AcpError::TransportClosed));
            }
        });

        (transport, inbound_rx)
    }

    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        if *self.closed.read().await {
            return Err(AcpError::TransportClosed);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

        let msg = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Value::from(id),
            method: method.into(),
            params,
        };
        self.write_json(&msg).await?;

        match rx.await {
            Ok(result) => result,
            Err(_) => Err(AcpError::TransportClosed),
        }
    }

    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        if *self.closed.read().await {
            return Err(AcpError::TransportClosed);
        }
        let msg = JsonRpcNotification {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params,
        };
        self.write_json(&msg).await
    }

    pub async fn respond(&self, id: Value, result: Value) -> Result<()> {
        let msg = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        };
        self.write_json(&msg).await
    }

    pub async fn respond_error(
        &self,
        id: Value,
        code: i64,
        message: impl Into<String>,
    ) -> Result<()> {
        let msg = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcErrorObject {
                code,
                message: message.into(),
                data: None,
            }),
        };
        self.write_json(&msg).await
    }

    async fn write_json<T: Serialize>(&self, value: &T) -> Result<()> {
        let mut line = serde_json::to_vec(value)?;
        line.push(b'\n');
        let mut write = self.write.lock().await;
        write.write_all(&line).await?;
        write.flush().await?;
        Ok(())
    }

    pub async fn is_closed(&self) -> bool {
        *self.closed.read().await
    }
}

async fn dispatch_line(
    line: &str,
    pending: &Mutex<HashMap<u64, oneshot::Sender<std::result::Result<Value, AcpError>>>>,
    inbound_tx: &mpsc::UnboundedSender<InboundMessage>,
) -> Result<()> {
    let msg = parse_jsonrpc_line(line)?;
    match msg {
        JsonRpcMessage::Response(resp) => {
            let id = match &resp.id {
                Value::Number(n) => n.as_u64(),
                Value::String(s) => s.parse().ok(),
                _ => None,
            };
            let Some(id) = id else {
                return Err(AcpError::Protocol(format!(
                    "response with non-numeric id: {}",
                    resp.id
                )));
            };
            let tx = {
                let mut pending = pending.lock().await;
                pending.remove(&id)
            };
            if let Some(tx) = tx {
                let result = if let Some(err) = resp.error {
                    Err(AcpError::Rpc {
                        code: err.code,
                        message: err.message,
                    })
                } else {
                    Ok(resp.result.unwrap_or(Value::Null))
                };
                let _ = tx.send(result);
            }
        }
        JsonRpcMessage::Request(req) => {
            let _ = inbound_tx.send(InboundMessage::Request {
                id: req.id,
                method: req.method,
                params: req.params.unwrap_or(Value::Null),
            });
        }
        JsonRpcMessage::Notification(note) => {
            let _ = inbound_tx.send(InboundMessage::Notification {
                method: note.method,
                params: note.params.unwrap_or(Value::Null),
            });
        }
    }
    Ok(())
}

/// Encode a JSON-RPC request line (for unit tests without a live transport).
pub fn encode_request_line(id: u64, method: &str, params: Option<Value>) -> Result<String> {
    let msg = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Value::from(id),
        method: method.into(),
        params,
    };
    Ok(serde_json::to_string(&msg)?)
}

/// Decode a single JSON-RPC line into a typed message.
pub fn decode_message_line(line: &str) -> Result<JsonRpcMessage> {
    parse_jsonrpc_line(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn request_response_routes_by_id() {
        let (client_read, server_write) = duplex(8192);
        let (server_read, client_write) = duplex(8192);

        let (transport, mut inbound) = JsonRpcTransport::new(client_read, client_write);

        // Server side: echo responses for requests, ignore notifications.
        tokio::spawn(async move {
            let mut lines = BufReader::new(server_read).lines();
            let mut writer = server_write;
            while let Ok(Some(line)) = lines.next_line().await {
                let req: JsonRpcRequest = serde_json::from_str(&line).expect("req");
                let resp = JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id,
                    result: Some(serde_json::json!({
                        "echo_method": req.method,
                        "params": req.params,
                    })),
                    error: None,
                };
                let mut bytes = serde_json::to_vec(&resp).unwrap();
                bytes.push(b'\n');
                writer.write_all(&bytes).await.unwrap();
                writer.flush().await.unwrap();
            }
        });

        let r1 = transport
            .request(
                "initialize",
                Some(serde_json::json!({"protocolVersion": 1})),
            )
            .await
            .expect("r1");
        assert_eq!(r1["echo_method"], "initialize");

        let r2 = transport
            .request("session/new", Some(serde_json::json!({"cwd": "/tmp"})))
            .await
            .expect("r2");
        assert_eq!(r2["echo_method"], "session/new");

        // Out-of-band notification should land on inbound, not steal responses.
        // Simulate by writing a notification from a second writer — use notify path reverse:
        // We'll just ensure inbound is empty for pure request/response.
        assert!(inbound.try_recv().is_err());
        let _ = r1;
        let _ = r2;
    }

    #[tokio::test]
    async fn concurrent_requests_route_correctly() {
        let (client_read, server_write) = duplex(8192);
        let (server_read, client_write) = duplex(8192);
        let (transport, _) = JsonRpcTransport::new(client_read, client_write);
        let transport = Arc::new(transport);

        tokio::spawn(async move {
            let mut lines = BufReader::new(server_read).lines();
            let mut writer = server_write;
            while let Ok(Some(line)) = lines.next_line().await {
                let req: JsonRpcRequest = serde_json::from_str(&line).unwrap();
                // Delay odd ids so responses arrive out of order.
                let id_num = req.id.as_u64().unwrap_or(0);
                if id_num % 2 == 1 {
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                }
                let resp = JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id,
                    result: Some(serde_json::json!({"id": id_num, "method": req.method})),
                    error: None,
                };
                let mut bytes = serde_json::to_vec(&resp).unwrap();
                bytes.push(b'\n');
                writer.write_all(&bytes).await.unwrap();
                writer.flush().await.unwrap();
            }
        });

        let t1 = transport.clone();
        let t2 = transport.clone();
        let t3 = transport.clone();
        let (a, b, c) = tokio::join!(
            t1.request("a", None),
            t2.request("b", None),
            t3.request("c", None),
        );
        let a = a.unwrap();
        let b = b.unwrap();
        let c = c.unwrap();
        assert_eq!(a["method"], "a");
        assert_eq!(b["method"], "b");
        assert_eq!(c["method"], "c");
    }

    #[tokio::test]
    async fn notifications_and_server_requests_go_inbound() {
        let (client_read, mut peer_write) = duplex(8192);
        let (_peer_read, client_write) = duplex(8192);
        let (transport, mut inbound) = JsonRpcTransport::new(client_read, client_write);

        let note = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}}}}"#;
        peer_write.write_all(note.as_bytes()).await.unwrap();
        peer_write.write_all(b"\n").await.unwrap();

        let req = r#"{"jsonrpc":"2.0","id":99,"method":"session/request_permission","params":{"sessionId":"s1","toolCall":{},"options":[]}}"#;
        peer_write.write_all(req.as_bytes()).await.unwrap();
        peer_write.write_all(b"\n").await.unwrap();

        let msg1 = tokio::time::timeout(std::time::Duration::from_secs(2), inbound.recv())
            .await
            .expect("timeout")
            .expect("note");
        match msg1 {
            InboundMessage::Notification { method, .. } => {
                assert_eq!(method, "session/update");
            }
            other => panic!("expected notification, got {other:?}"),
        }

        let msg2 = tokio::time::timeout(std::time::Duration::from_secs(2), inbound.recv())
            .await
            .expect("timeout")
            .expect("req");
        match msg2 {
            InboundMessage::Request { id, method, .. } => {
                assert_eq!(id, Value::from(99));
                assert_eq!(method, "session/request_permission");
            }
            other => panic!("expected request, got {other:?}"),
        }

        // Respond to the server request so the API is exercised.
        transport
            .respond(
                Value::from(99),
                serde_json::json!({"outcome":{"outcome":"cancelled"}}),
            )
            .await
            .unwrap();
    }

    #[test]
    fn encode_decode_roundtrip() {
        let line = encode_request_line(7, "session/prompt", Some(serde_json::json!({"x": 1})))
            .expect("encode");
        let msg = decode_message_line(&line).unwrap();
        match msg {
            JsonRpcMessage::Request(r) => {
                assert_eq!(r.id, Value::from(7));
                assert_eq!(r.method, "session/prompt");
            }
            _ => panic!("expected request"),
        }
    }
}
