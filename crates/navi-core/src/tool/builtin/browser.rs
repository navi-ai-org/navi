//! Built-in multi-action browser tool (CDP-backed via `navi-browser`).

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
#[cfg(feature = "browser")]
use std::sync::OnceLock;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

#[cfg(feature = "browser")]
use crate::config::NaviConfig;
#[cfg(feature = "browser")]
use navi_browser::{BrowserRuntimeConfig, BrowserSession, doctor_report};

/// Global default session used when the runtime does not inject a per-session handle.
/// Tools are registered without session ids today; this still enables multi-step browsing
/// within one process (TUI/headless run).
#[cfg(feature = "browser")]
static SHARED_SESSION: OnceLock<std::sync::Mutex<Option<BrowserSession>>> = OnceLock::new();

pub(crate) struct BrowserTool {
    project_root: PathBuf,
}

impl BrowserTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }

    #[cfg(feature = "browser")]
    fn runtime_config(&self) -> BrowserRuntimeConfig {
        let loaded = NaviConfig::load(&self.project_root).unwrap_or_default();
        let c = &loaded.config.browser;
        BrowserRuntimeConfig {
            enabled: c.enabled,
            backend: c.backend.clone(),
            cdp_url: c.cdp_url.clone(),
            headless: c.headless,
            allow_private_network: c.allow_private_network,
            proxy: c.proxy.clone(),
            timeout_ms: c.timeout_ms,
            binary_path: c.binary_path.clone(),
            humanize: c.humanize,
        }
    }

    #[cfg(feature = "browser")]
    fn data_dir(&self) -> PathBuf {
        NaviConfig::load(&self.project_root)
            .map(|l| l.data_dir)
            .unwrap_or_else(|_| {
                directories::ProjectDirs::from("dev", "navi", "navi")
                    .map(|d| d.data_local_dir().to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("/tmp/navi"))
            })
    }

    #[cfg(feature = "browser")]
    fn session(&self) -> Result<BrowserSession> {
        let slot = SHARED_SESSION.get_or_init(|| std::sync::Mutex::new(None));
        let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = guard.as_ref() {
            return Ok(s.clone());
        }
        let session = BrowserSession::new(self.runtime_config(), self.data_dir(), "default");
        *guard = Some(session.clone());
        Ok(session)
    }
}

#[async_trait]
impl Tool for BrowserTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "browser",
            "Control a headless browser via the pluggable NAVI browser engine (CloakBrowser Rust binding when registered; CDP fallback otherwise). \
Actions: status, open, goto, snapshot, screenshot, click, type, press, content, evaluate, close, doctor. \
Use to test local web UIs, research pages, and capture screenshots. Screenshots are saved under the NAVI data directory.",
            ToolKind::Command,
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": [
                            "status", "open", "goto", "snapshot", "screenshot",
                            "click", "type", "press", "content", "evaluate", "close", "doctor"
                        ],
                        "description": "Browser action to perform."
                    },
                    "url": {
                        "type": "string",
                        "description": "URL for goto (http/https)."
                    },
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for click/type."
                    },
                    "text": {
                        "type": "string",
                        "description": "Text to type into the selected element."
                    },
                    "key": {
                        "type": "string",
                        "description": "Key name for press (e.g. Enter, Tab, Escape)."
                    },
                    "expression": {
                        "type": "string",
                        "description": "JavaScript expression for evaluate."
                    },
                    "kind": {
                        "type": "string",
                        "enum": ["text", "html"],
                        "description": "Content kind for content action (default text)."
                    },
                    "max_chars": {
                        "type": "integer",
                        "description": "Max characters for snapshot/content (default 12000)."
                    }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        #[cfg(not(feature = "browser"))]
        {
            return Ok(helpers::ok(
                invocation.id,
                json!({
                    "error": "browser feature disabled",
                    "message": "Rebuild NAVI with --features browser (default on most builds).",
                }),
            ));
        }

        #[cfg(feature = "browser")]
        {
            let action =
                helpers::required_string(&invocation.input, "action")?.to_ascii_lowercase();
            let max_chars = invocation
                .input
                .get("max_chars")
                .and_then(|v| v.as_u64())
                .unwrap_or(12_000) as usize;

            let output = match action.as_str() {
                "doctor" => doctor_report(&self.runtime_config()),
                "status" => {
                    let session = self.session()?;
                    session.status().await
                }
                "open" => {
                    let session = self.session()?;
                    session.ensure_open().await?;
                    session.status().await
                }
                "goto" => {
                    let url = helpers::required_string(&invocation.input, "url")?;
                    let session = self.session()?;
                    session.goto(url).await?
                }
                "snapshot" => {
                    let session = self.session()?;
                    session.snapshot(max_chars).await?
                }
                "screenshot" => {
                    let session = self.session()?;
                    session.screenshot().await?
                }
                "click" => {
                    let selector = helpers::required_string(&invocation.input, "selector")?;
                    let session = self.session()?;
                    session.click(selector).await?
                }
                "type" => {
                    let selector = helpers::required_string(&invocation.input, "selector")?;
                    let text = helpers::required_string(&invocation.input, "text")?;
                    let session = self.session()?;
                    session.type_text(selector, text).await?
                }
                "press" => {
                    let key = helpers::required_string(&invocation.input, "key")?;
                    let session = self.session()?;
                    session.press(key).await?
                }
                "content" => {
                    let kind = invocation
                        .input
                        .get("kind")
                        .and_then(|v| v.as_str())
                        .unwrap_or("text");
                    let session = self.session()?;
                    session.content(kind, max_chars).await?
                }
                "evaluate" => {
                    let expression = helpers::required_string(&invocation.input, "expression")?;
                    let session = self.session()?;
                    session.evaluate(expression).await?
                }
                "close" => {
                    let session = self.session()?;
                    session.close().await?
                }
                other => bail!("unknown browser action '{other}'"),
            };

            Ok(helpers::ok(invocation.id, output))
        }
    }
}

#[cfg(all(test, feature = "browser"))]
mod tests {
    use super::*;
    use crate::tool::ToolInvocation;

    #[tokio::test]
    async fn doctor_action_returns_json() {
        let tool = BrowserTool::new(PathBuf::from("/tmp"));
        let result = tool
            .invoke(ToolInvocation {
                id: "t1".into(),
                tool_name: "browser".into(),
                input: json!({ "action": "doctor" }),
            })
            .await
            .expect("doctor");
        assert!(result.ok);
        assert!(
            result.output.get("enabled").is_some()
                || result.output.get("backend").is_some()
                || result.output.get("hints").is_some()
        );
    }

    #[tokio::test]
    async fn definition_is_command_kind() {
        let tool = BrowserTool::new(PathBuf::from("/tmp"));
        let def = tool.definition();
        assert_eq!(def.name, "browser");
        assert_eq!(def.kind, ToolKind::Command);
    }
}
