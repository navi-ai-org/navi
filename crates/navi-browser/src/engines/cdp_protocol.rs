// Minimal CDP client over WebSocket (page target).

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};

pub struct CdpConnection {
    write: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    read: futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    next_id: u64,
}

impl CdpConnection {
    pub async fn connect(ws_url: &str) -> Result<Self> {
        let (ws, _) = connect_async(ws_url)
            .await
            .with_context(|| format!("CDP websocket connect failed: {ws_url}"))?;
        let (write, read) = ws.split();
        Ok(Self {
            write,
            read,
            next_id: 1,
        })
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        self.write
            .send(Message::Text(msg.to_string().into()))
            .await
            .context("CDP send")?;

        let deadline = Duration::from_secs(30);
        timeout(deadline, async {
            while let Some(frame) = self.read.next().await {
                let frame = frame.context("CDP read")?;
                let text = match frame {
                    Message::Text(t) => t.to_string(),
                    Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
                    Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
                    Message::Close(_) => bail!("CDP connection closed"),
                };
                let value: Value = serde_json::from_str(&text).context("CDP json")?;
                if value.get("id").and_then(|v| v.as_u64()) == Some(id) {
                    if let Some(err) = value.get("error") {
                        bail!("CDP error for {method}: {err}");
                    }
                    return Ok(value.get("result").cloned().unwrap_or(Value::Null));
                }
                // Ignore events (no id) or other ids.
            }
            bail!("CDP stream ended waiting for {method}")
        })
        .await
        .map_err(|_| anyhow::anyhow!("CDP timeout waiting for {method}"))?
    }

    pub async fn navigate(&mut self, url: &str) -> Result<()> {
        let _ = self
            .call("Page.enable", json!({}))
            .await
            .context("Page.enable")?;
        let _ = self
            .call(
                "Page.navigate",
                json!({
                    "url": url,
                }),
            )
            .await
            .context("Page.navigate")?;
        // Best-effort load wait via Runtime.
        let _ = self
            .evaluate(
                r#"
                new Promise((resolve) => {
                  if (document.readyState === 'complete') resolve(true);
                  else window.addEventListener('load', () => resolve(true), { once: true });
                  setTimeout(() => resolve(false), 15000);
                })
                "#,
            )
            .await;
        Ok(())
    }

    pub async fn evaluate(&mut self, expression: &str) -> Result<Value> {
        let result = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;
        if let Some(ex) = result.get("exceptionDetails") {
            bail!("JS exception: {ex}");
        }
        Ok(result
            .pointer("/result/value")
            .cloned()
            .unwrap_or(Value::Null))
    }

    pub async fn screenshot_png_base64(&mut self) -> Result<String> {
        let _ = self.call("Page.enable", json!({})).await;
        let result = self
            .call(
                "Page.captureScreenshot",
                json!({
                    "format": "png",
                    "fromSurface": true,
                }),
            )
            .await?;
        result
            .get("data")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .context("screenshot missing data")
    }
}

/// Create a new page target via DevTools HTTP API and return its WS debugger URL.
pub async fn new_page_ws(cdp_http_base: &str, initial_url: &str) -> Result<(String, String)> {
    let base = cdp_http_base.trim_end_matches('/');
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    // Prefer /json/new (Chrome).
    let new_url = format!("{base}/json/new?{}", urlencoding_minimal(initial_url));
    if let Ok(resp) = client.put(&new_url).send().await
        && resp.status().is_success() {
            let body: Value = resp.json().await.context("json/new body")?;
            let id = body
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let ws = body
                .get("webSocketDebuggerUrl")
                .and_then(|v| v.as_str())
                .context("json/new missing webSocketDebuggerUrl")?
                .to_string();
            return Ok((id, ws));
        }

    // Fallback: list existing about:blank tabs.
    let list_url = format!("{base}/json/list");
    let tabs: Value = client
        .get(&list_url)
        .send()
        .await
        .context("json/list")?
        .json()
        .await
        .context("json/list body")?;
    let arr = tabs.as_array().context("json/list not array")?;
    let page = arr
        .iter()
        .find(|t| t.get("type").and_then(|v| v.as_str()) == Some("page"))
        .context("no page target available — is the browser running?")?;
    let id = page
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let ws = page
        .get("webSocketDebuggerUrl")
        .and_then(|v| v.as_str())
        .context("page missing webSocketDebuggerUrl")?
        .to_string();
    Ok((id, ws))
}

fn urlencoding_minimal(s: &str) -> String {
    // Enough for query string on /json/new.
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | ':' | '/' | '?' | '='
            | '&' => c.to_string(),
            _ => format!("%{:02X}", c as u8),
        })
        .collect()
}
