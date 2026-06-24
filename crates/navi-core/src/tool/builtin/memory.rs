use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

use crate::config::NaviConfig;
use crate::memory::MemoryManager;
use crate::tool::builtin::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

/// Tool to append observations to notes.md safely.
pub(crate) struct AppendNoteTool {
    project_root: PathBuf,
}

impl AppendNoteTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }
}

#[async_trait]
impl Tool for AppendNoteTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "append_note",
            "Append a note, temporary observation, or status update to the notes.md scratchpad.",
            ToolKind::Write,
            helpers::json_schema(
                &[("content", "The text content to append to notes.md.")],
                &["content"],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let content = helpers::required_string(&invocation.input, "content")?;

        let loaded_config = NaviConfig::load(&self.project_root).unwrap_or_default();
        let manager = MemoryManager::new(self.project_root.clone(), &loaded_config.config.memory)?;

        manager.store.append_note(content)?;

        let output = json!({
            "status": "success",
            "message": "Note successfully appended to notes.md",
            "path": manager.store.notes_path().to_string_lossy().to_string()
        });

        Ok(helpers::ok(invocation.id, output))
    }
}

/// Tool to query SQLite session histories.
pub(crate) struct HistoryOpsTool {
    project_root: PathBuf,
}

impl HistoryOpsTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }
}

#[async_trait]
impl Tool for HistoryOpsTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "history_ops",
            "Expose raw trace history and search capabilities from the SQLite database. Actions: search, recent, get, summaries.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["search", "recent", "get", "summaries"],
                        "description": "The action to perform: 'search', 'recent', 'get', or 'summaries'."
                    },
                    "query": {
                        "type": "string",
                        "description": "The search term (required for 'search')."
                    },
                    "session_id": {
                        "type": "string",
                        "description": "Filter results by session ID (optional/required for 'recent')."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max number of events to return."
                    },
                    "event_id": {
                        "type": "integer",
                        "description": "Event ID to retrieve (required for 'get')."
                    }
                },
                "required": ["action"],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let action = helpers::required_string(&invocation.input, "action")?;

        let loaded_config = NaviConfig::load(&self.project_root).unwrap_or_default();
        let manager = MemoryManager::new(self.project_root.clone(), &loaded_config.config.memory)?;

        let output = match action {
            "search" => {
                let query = helpers::required_string(&invocation.input, "query")?;
                let session_id = invocation.input.get("session_id").and_then(|v| v.as_str());
                let limit = invocation.input.get("limit").and_then(|v| v.as_i64());
                let results = manager.history.search_history(query, session_id, limit)?;
                json!({ "results": results })
            }
            "recent" => {
                let session_id = helpers::required_string(&invocation.input, "session_id")?;
                let limit = invocation.input.get("limit").and_then(|v| v.as_i64());
                let results = manager.history.get_recent_events(session_id, limit)?;
                json!({ "results": results })
            }
            "get" => {
                let event_id = invocation
                    .input
                    .get("event_id")
                    .and_then(|v| v.as_i64())
                    .context("Missing 'event_id' for 'get' action")?;
                let result = manager.history.get_event(event_id)?;
                json!({ "event": result })
            }
            "summaries" => {
                let results = manager.history.list_sessions()?;
                json!({ "sessions": results })
            }
            _ => anyhow::bail!("Unsupported action: {}", action),
        };

        Ok(helpers::ok(invocation.id, output))
    }
}
