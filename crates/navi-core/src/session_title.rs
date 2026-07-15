//! Session-title state shared between the runtime and the agent tool.

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

/// Session-local title state. The tool writes a pending title and the runtime
/// applies it to the persisted session at the next safe lifecycle boundary.
#[derive(Clone, Default)]
pub struct SessionTitleHandle {
    pending: Arc<Mutex<Option<String>>>,
    assigned: Arc<AtomicBool>,
}

impl SessionTitleHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_assigned(&self) -> bool {
        self.assigned.load(Ordering::SeqCst)
    }

    pub fn set(&self, title: &str) -> Result<String> {
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        let title = title.trim();
        if title.is_empty() {
            anyhow::bail!("session title must not be empty");
        }
        let title: String = title.chars().take(120).collect();
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(title.clone());
        self.assigned.store(true, Ordering::SeqCst);
        Ok(title)
    }

    pub fn take(&self) -> Option<String> {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

/// Lets the chat model name the current session without starting a separate
/// completion. The first turn is instructed to call this before doing work;
/// later calls are reserved for material changes of topic.
pub struct SessionTitleTool {
    handle: SessionTitleHandle,
}

impl SessionTitleTool {
    pub fn new(handle: SessionTitleHandle) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl Tool for SessionTitleTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "set_session_title",
            "Set a concise title for the current session. Call this as the first action of a new session using the user's initial request. Call it again only when the session's primary objective changes materially.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Concise, specific session title (maximum 120 characters)."
                    }
                },
                "required": ["title"],
                "additionalProperties": false
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let title = invocation
            .input
            .get("title")
            .and_then(serde_json::Value::as_str)
            .context("missing required string `title`")?;
        let title = self.handle.set(title)?;
        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: true,
            output: json!({ "title": title }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tool_normalizes_and_records_title() {
        let handle = SessionTitleHandle::new();
        let tool = SessionTitleTool::new(handle.clone());
        let result = tool
            .invoke(ToolInvocation {
                id: "title-1".into(),
                tool_name: "set_session_title".into(),
                input: json!({ "title": "  Repair   prompt   cache  " }),
            })
            .await
            .expect("title tool succeeds");

        assert!(result.ok);
        assert!(handle.is_assigned());
        assert_eq!(handle.take().as_deref(), Some("Repair prompt cache"));
    }
}
