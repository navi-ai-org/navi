use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::PathBuf;

use crate::config::NaviConfig;
use crate::memory::AutoMemoryStore;
use crate::memory::MemoryManager;
use crate::memory::MemoryStatus;
use crate::memory::MemoryType;
use crate::memory::auto_memory::{new_entry, sanitize_id};
use crate::memory::embedding::{embeddings_available, get_cached_embedder};
use crate::tool::builtin::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

/// Tool to append observations to the session notes scratchpad (SQLite).
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
            "Append a note, temporary observation, or status update to the session notes scratchpad.",
            ToolKind::Write,
            helpers::json_schema(
                &[(
                    "content",
                    "The text content to append to the session notes.",
                )],
                &["content"],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let content = helpers::required_string(&invocation.input, "content")?;

        let loaded_config = NaviConfig::load(&self.project_root).unwrap_or_default();
        let manager = MemoryManager::new(
            self.project_root.clone(),
            loaded_config.data_dir.clone(),
            &loaded_config.config.memory,
        )?;

        manager.auto_memory.append_note(content)?;

        let output = json!({
            "status": "success",
            "message": "Note successfully appended to session notes",
            "db_path": manager.auto_memory.db_path.to_string_lossy().to_string()
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
        let manager = MemoryManager::new(
            self.project_root.clone(),
            loaded_config.data_dir.clone(),
            &loaded_config.config.memory,
        )?;

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

/// Unified memory tool for the model to write, read, list, search,
/// update, and delete persistent auto-memories.
///
/// All memories are stored in SQLite with structured fields.
/// Search uses semantic embeddings (Qwen3-Embedding-0.6B via candle)
/// when the `embeddings` feature is enabled and the model is present,
/// falling back to text matching (LIKE) otherwise.
///
/// Memory types:
/// - `user` — preferences, identity, working style
/// - `feedback` — behaviors to repeat or avoid
/// - `project` — non-derivable project context (deadlines, decisions)
/// - `reference` — links to dashboards, external docs
pub(crate) struct MemoryTool {
    project_root: PathBuf,
}

impl MemoryTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }

    /// Compute the memories.db path from the current config.
    ///
    /// Recomputed on every call so that config changes (e.g. `data_dir`
    /// edited mid-session) are picked up without restarting the tool.
    fn db_path(&self) -> PathBuf {
        let config = NaviConfig::load(&self.project_root).unwrap_or_default();
        let manager = MemoryManager::new(
            self.project_root.clone(),
            config.data_dir.clone(),
            &config.config.memory,
        );
        match manager {
            Ok(m) => m.store.memory_root.join("memories.db"),
            Err(_) => config.data_dir.join("memory").join("memories.db"),
        }
    }

    fn open_store(&self) -> Result<AutoMemoryStore> {
        AutoMemoryStore::open(&self.db_path())
    }

    fn resolve_model_paths(&self) -> (PathBuf, PathBuf) {
        let config = NaviConfig::load(&self.project_root).unwrap_or_default();
        let manager = MemoryManager::new(
            self.project_root.clone(),
            config.data_dir.clone(),
            &config.config.memory,
        );

        let models_dir = match &manager {
            Ok(m) => m.store.memory_root.join("models"),
            Err(_) => config.data_dir.join("memory").join("models"),
        };

        // Use config override if set, otherwise use default path in models dir
        let model_path = if config.config.memory.embedding_model_path.is_empty() {
            models_dir.join("qwen3-embedding-0.6b-q8_0.gguf")
        } else {
            PathBuf::from(&config.config.memory.embedding_model_path)
        };

        let tokenizer_path = if config.config.memory.embedding_tokenizer_path.is_empty() {
            models_dir.join("tokenizer.json")
        } else {
            PathBuf::from(&config.config.memory.embedding_tokenizer_path)
        };

        (model_path, tokenizer_path)
    }

    fn try_generate_embedding(&self, text: &str) -> Option<Vec<f32>> {
        if !embeddings_available() {
            return None;
        }

        let (model_path, tokenizer_path) = self.resolve_model_paths();

        let embedder = get_cached_embedder(&model_path, &tokenizer_path)?;

        match embedder.embed(text) {
            Ok(emb) => Some(emb),
            Err(e) => {
                tracing::debug!(
                    "Embedding generation failed: {}, falling back to text search",
                    e
                );
                None
            }
        }
    }
}

#[async_trait]
impl Tool for MemoryTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "memory",
            "Persistent auto-memory system with search. Save, retrieve, search, update, and delete memories that survive across sessions. Use `search` when you need to find relevant memories, `write` to save new learnings, `list` to see everything stored.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["write", "read", "list", "search", "update", "delete"],
                        "description": "Action to perform."
                    },
                    "id": {
                        "type": "string",
                        "description": "Memory id (sanitized: lowercase, alphanumeric, hyphens). Required for write, read, update, delete. Example: 'redis_tests'."
                    },
                    "memory_type": {
                        "type": "string",
                        "enum": ["user", "feedback", "project", "reference"],
                        "description": "Memory type. Required for write. user=preferences/identity, feedback=behaviors to repeat/avoid, project=non-derivable context, reference=external links."
                    },
                    "name": {
                        "type": "string",
                        "description": "Human-readable title. Required for write."
                    },
                    "description": {
                        "type": "string",
                        "description": "One-line summary. Required for write."
                    },
                    "body": {
                        "type": "string",
                        "description": "Markdown body content. Required for write."
                    },
                    "query": {
                        "type": "string",
                        "description": "Search query (text matching). Required for search."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max results for search/list. Default: 20."
                    },
                    "status": {
                        "type": "string",
                        "enum": ["active", "needs_review", "obsolete"],
                        "description": "Filter by status (for list) or set new status (for update)."
                    }
                },
                "required": ["action"],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let action = helpers::required_string(&invocation.input, "action")?;
        let store = self.open_store()?;

        let output: Value = match action {
            "write" => {
                let raw_id = helpers::required_string(&invocation.input, "id")?;
                let id = sanitize_id(raw_id);
                let memory_type_str = helpers::required_string(&invocation.input, "memory_type")?;
                let memory_type = MemoryType::from_str(memory_type_str)
                    .context(format!("Invalid memory_type: {memory_type_str}"))?;
                let name = helpers::required_string(&invocation.input, "name")?;
                let description = helpers::required_string(&invocation.input, "description")?;
                let body = helpers::required_string(&invocation.input, "body")?;

                let entry = new_entry(&id, memory_type, name, description, body);
                store.upsert(&entry)?;

                // Generate and store embedding if available
                let embed_text = format!("{name}\n{description}\n{body}");
                let has_embedding = if let Some(emb) = self.try_generate_embedding(&embed_text) {
                    store.set_embedding(&id, &emb).is_ok()
                } else {
                    false
                };

                json!({
                    "status": "success",
                    "message": format!("Memory '{}' saved", name),
                    "id": id,
                    "type": memory_type.as_str(),
                    "embedded": has_embedding,
                })
            }

            "read" => {
                let id = sanitize_id(helpers::required_string(&invocation.input, "id")?);
                let entry = store
                    .get(&id)?
                    .context(format!("Memory '{}' not found", id))?;

                json!({
                    "status": "success",
                    "id": entry.id,
                    "name": entry.name,
                    "description": entry.description,
                    "type": entry.memory_type.as_str(),
                    "body": entry.body,
                    "confidence": entry.confidence,
                    "memory_status": entry.status.as_str(),
                    "created_at": entry.created_at,
                    "updated_at": entry.updated_at,
                })
            }

            "list" => {
                let limit = invocation
                    .input
                    .get("limit")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(50) as usize;
                let status_filter = invocation
                    .input
                    .get("status")
                    .and_then(|v| v.as_str())
                    .and_then(MemoryStatus::from_str);

                let memories = store.list(status_filter)?;
                let count = memories.len();
                let limited: Vec<_> = memories.into_iter().take(limit).collect();

                json!({
                    "status": "success",
                    "count": count,
                    "returned": limited.len(),
                    "memories": limited.iter().map(|m| json!({
                        "id": m.id,
                        "name": m.name,
                        "description": m.description,
                        "type": m.memory_type.as_str(),
                        "confidence": m.confidence,
                        "memory_status": m.status.as_str(),
                        "updated_at": m.updated_at,
                    })).collect::<Vec<_>>(),
                })
            }

            "search" => {
                let query = helpers::required_string(&invocation.input, "query")?;
                let limit = invocation
                    .input
                    .get("limit")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(20) as usize;

                // Try semantic search first (embeddings), fall back to text matching
                let search_results: Vec<(
                    String,
                    String,
                    String,
                    crate::memory::MemoryType,
                    f64,
                    String,
                )> = if let Some(query_emb) = self.try_generate_embedding(query) {
                    let semantic = store.search_semantic(&query_emb, 0.3, limit)?;
                    if !semantic.is_empty() {
                        semantic
                            .into_iter()
                            .map(|(m, score)| {
                                (
                                    m.id,
                                    m.name,
                                    m.description,
                                    m.memory_type,
                                    m.confidence,
                                    format!("semantic:{:.3}", score),
                                )
                            })
                            .collect()
                    } else {
                        // Semantic returned nothing — fall back to text
                        let text_results = store.search_text(query, limit)?;
                        text_results
                            .into_iter()
                            .map(|m| {
                                (
                                    m.id,
                                    m.name,
                                    m.description,
                                    m.memory_type,
                                    m.confidence,
                                    "text_match".to_string(),
                                )
                            })
                            .collect()
                    }
                } else {
                    // No embeddings available — text search only
                    let text_results = store.search_text(query, limit)?;
                    text_results
                        .into_iter()
                        .map(|m| {
                            (
                                m.id,
                                m.name,
                                m.description,
                                m.memory_type,
                                m.confidence,
                                "text_match".to_string(),
                            )
                        })
                        .collect()
                };

                json!({
                    "status": "success",
                    "query": query,
                    "count": search_results.len(),
                    "results": search_results.iter().map(|(id, name, desc, mtype, conf, rel)| json!({
                        "id": id,
                        "name": name,
                        "description": desc,
                        "type": mtype.as_str(),
                        "confidence": conf,
                        "relevance": rel,
                    })).collect::<Vec<_>>(),
                })
            }

            "update" => {
                let id = sanitize_id(helpers::required_string(&invocation.input, "id")?);

                if let Some(status_str) = invocation.input.get("status").and_then(|v| v.as_str())
                    && let Some(status) = MemoryStatus::from_str(status_str)
                {
                    store.set_status(&id, status)?;
                }

                let name = invocation.input.get("name").and_then(|v| v.as_str());
                let description = invocation.input.get("description").and_then(|v| v.as_str());
                let body = invocation.input.get("body").and_then(|v| v.as_str());

                store.update(&id, name, description, body)?;

                // Regenerate embedding if body/description/name changed
                let content_changed = name.is_some() || description.is_some() || body.is_some();
                let re_embedded = if content_changed {
                    if let Some(entry) = store.get(&id)? {
                        let embed_text =
                            format!("{}\n{}\n{}", entry.name, entry.description, entry.body);
                        if let Some(emb) = self.try_generate_embedding(&embed_text) {
                            store.set_embedding(&id, &emb).is_ok()
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };

                json!({
                    "status": "success",
                    "message": format!("Memory '{}' updated", id),
                    "id": id,
                    "re_embedded": re_embedded,
                })
            }

            "delete" => {
                let id = sanitize_id(helpers::required_string(&invocation.input, "id")?);
                store.delete(&id)?;

                json!({
                    "status": "success",
                    "message": format!("Memory '{}' deleted", id),
                    "id": id,
                })
            }

            _ => anyhow::bail!("Unsupported action: {}", action),
        };

        Ok(helpers::ok(invocation.id, output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Tool, ToolInvocation, ToolKind};
    use serde_json::json;

    fn make_append_note_tool(root: &std::path::Path) -> AppendNoteTool {
        AppendNoteTool::new(root.to_path_buf())
    }

    fn make_history_ops_tool(root: &std::path::Path) -> HistoryOpsTool {
        HistoryOpsTool::new(root.to_path_buf())
    }

    fn make_memory_tool(root: &std::path::Path) -> MemoryTool {
        MemoryTool::new(root.to_path_buf())
    }

    fn make_invocation(id: &str, tool_name: &str, input: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            id: id.to_string(),
            tool_name: tool_name.to_string(),
            input,
        }
    }

    // ── AppendNoteTool definition ─────────────────────────────────────────

    #[test]
    fn append_note_definition_name() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_append_note_tool(temp.path());
        let def = tool.definition();
        assert_eq!(def.name, "append_note");
    }

    #[test]
    fn append_note_definition_kind_is_write() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_append_note_tool(temp.path());
        let def = tool.definition();
        assert_eq!(def.kind, ToolKind::Write);
    }

    #[test]
    fn append_note_definition_requires_content() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_append_note_tool(temp.path());
        let def = tool.definition();
        let required = def.input_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(required.contains(&"content"));
    }

    // ── AppendNoteTool invoke ─────────────────────────────────────────────

    #[tokio::test]
    async fn append_note_invoke_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_append_note_tool(temp.path());
        let inv = make_invocation("an1", "append_note", json!({"content": "test note"}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "success");
        assert!(
            result.output["db_path"].as_str().is_some(),
            "should have db_path: {result:?}"
        );
    }

    #[tokio::test]
    async fn append_note_invoke_missing_content_returns_err() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_append_note_tool(temp.path());
        let inv = make_invocation("an2", "append_note", json!({}));
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "missing content should return Err");
    }

    // ── HistoryOpsTool definition ─────────────────────────────────────────

    #[test]
    fn history_ops_definition_name() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_history_ops_tool(temp.path());
        let def = tool.definition();
        assert_eq!(def.name, "history_ops");
    }

    #[test]
    fn history_ops_definition_kind_is_read() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_history_ops_tool(temp.path());
        let def = tool.definition();
        assert_eq!(def.kind, ToolKind::Read);
    }

    #[test]
    fn history_ops_definition_action_enum() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_history_ops_tool(temp.path());
        let def = tool.definition();
        let actions = def.input_schema["properties"]["action"]["enum"]
            .as_array()
            .unwrap();
        let names: Vec<&str> = actions.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(names.contains(&"search"));
        assert!(names.contains(&"recent"));
        assert!(names.contains(&"get"));
        assert!(names.contains(&"summaries"));
    }

    #[test]
    fn history_ops_definition_requires_action() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_history_ops_tool(temp.path());
        let def = tool.definition();
        let required = def.input_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(required.contains(&"action"));
    }

    // ── HistoryOpsTool invoke ─────────────────────────────────────────────

    #[tokio::test]
    async fn history_ops_summaries_returns_empty() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_history_ops_tool(temp.path());
        let inv = make_invocation("h1", "history_ops", json!({"action": "summaries"}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert!(result.output["sessions"].is_array());
    }

    #[tokio::test]
    async fn history_ops_search_with_query() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_history_ops_tool(temp.path());
        let inv = make_invocation(
            "h2",
            "history_ops",
            json!({"action": "search", "query": "test"}),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert!(result.output["results"].is_array());
    }

    #[tokio::test]
    async fn history_ops_recent_with_session_id() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_history_ops_tool(temp.path());
        let inv = make_invocation(
            "h3",
            "history_ops",
            json!({"action": "recent", "session_id": "test-session"}),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert!(result.output["results"].is_array());
    }

    #[tokio::test]
    async fn history_ops_recent_missing_session_id_returns_err() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_history_ops_tool(temp.path());
        let inv = make_invocation("h4", "history_ops", json!({"action": "recent"}));
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "missing session_id should return Err");
    }

    #[tokio::test]
    async fn history_ops_search_missing_query_returns_err() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_history_ops_tool(temp.path());
        let inv = make_invocation("h5", "history_ops", json!({"action": "search"}));
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "missing query should return Err");
    }

    #[tokio::test]
    async fn history_ops_get_with_event_id() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_history_ops_tool(temp.path());
        let inv = make_invocation("h6", "history_ops", json!({"action": "get", "event_id": 1}));
        // The event likely doesn't exist, but the tool should still return Ok.
        let result = tool.invoke(inv).await;
        if let Ok(result) = result {
            assert!(result.ok);
        }
    }

    #[tokio::test]
    async fn history_ops_get_missing_event_id_returns_err() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_history_ops_tool(temp.path());
        let inv = make_invocation("h7", "history_ops", json!({"action": "get"}));
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "missing event_id should return Err");
    }

    #[tokio::test]
    async fn history_ops_unsupported_action_returns_err() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_history_ops_tool(temp.path());
        let inv = make_invocation("h8", "history_ops", json!({"action": "unsupported"}));
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "unsupported action should return Err");
    }

    #[tokio::test]
    async fn history_ops_missing_action_returns_err() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_history_ops_tool(temp.path());
        let inv = make_invocation("h9", "history_ops", json!({}));
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "missing action should return Err");
    }

    // ── MemoryTool definition ─────────────────────────────────────────────

    #[test]
    fn memory_tool_definition_name() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let def = tool.definition();
        assert_eq!(def.name, "memory");
    }

    #[test]
    fn memory_tool_definition_kind_is_read() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let def = tool.definition();
        assert_eq!(def.kind, ToolKind::Read);
    }

    #[test]
    fn memory_tool_definition_action_enum() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let def = tool.definition();
        let actions = def.input_schema["properties"]["action"]["enum"]
            .as_array()
            .unwrap();
        let names: Vec<&str> = actions.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(names.contains(&"write"));
        assert!(names.contains(&"read"));
        assert!(names.contains(&"list"));
        assert!(names.contains(&"search"));
        assert!(names.contains(&"update"));
        assert!(names.contains(&"delete"));
    }

    #[test]
    fn memory_tool_definition_memory_type_enum() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let def = tool.definition();
        let types = def.input_schema["properties"]["memory_type"]["enum"]
            .as_array()
            .unwrap();
        let names: Vec<&str> = types.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(names.contains(&"user"));
        assert!(names.contains(&"feedback"));
        assert!(names.contains(&"project"));
        assert!(names.contains(&"reference"));
    }

    #[test]
    fn memory_tool_definition_status_enum() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let def = tool.definition();
        let statuses = def.input_schema["properties"]["status"]["enum"]
            .as_array()
            .unwrap();
        let names: Vec<&str> = statuses.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(names.contains(&"active"));
        assert!(names.contains(&"needs_review"));
        assert!(names.contains(&"obsolete"));
    }

    // ── MemoryTool invoke: write ──────────────────────────────────────────

    #[tokio::test]
    async fn memory_write_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let inv = make_invocation(
            "m1",
            "memory",
            json!({
                "action": "write",
                "id": "test-memory",
                "memory_type": "user",
                "name": "Test Memory",
                "description": "A test memory entry",
                "body": "This is the body content.",
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "success");
        assert_eq!(result.output["id"], "test-memory");
        assert_eq!(result.output["type"], "user");
    }

    #[tokio::test]
    async fn memory_write_missing_id_returns_err() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let inv = make_invocation(
            "m2",
            "memory",
            json!({
                "action": "write",
                "memory_type": "user",
                "name": "Test",
                "description": "desc",
                "body": "body",
            }),
        );
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "missing id should return Err");
    }

    #[tokio::test]
    async fn memory_write_invalid_memory_type_returns_err() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let inv = make_invocation(
            "m3",
            "memory",
            json!({
                "action": "write",
                "id": "test",
                "memory_type": "invalid_type",
                "name": "Test",
                "description": "desc",
                "body": "body",
            }),
        );
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "invalid memory_type should return Err");
    }

    #[tokio::test]
    async fn memory_write_missing_name_returns_err() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let inv = make_invocation(
            "m4",
            "memory",
            json!({
                "action": "write",
                "id": "test",
                "memory_type": "user",
                "description": "desc",
                "body": "body",
            }),
        );
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "missing name should return Err");
    }

    // ── MemoryTool invoke: read ───────────────────────────────────────────

    #[tokio::test]
    async fn memory_read_after_write() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());

        // Write first
        let inv = make_invocation(
            "m5w",
            "memory",
            json!({
                "action": "write",
                "id": "read-test",
                "memory_type": "project",
                "name": "Read Test",
                "description": "Test reading",
                "body": "Body content here.",
            }),
        );
        tool.invoke(inv).await.unwrap();

        // Read it back
        let inv = make_invocation(
            "m5r",
            "memory",
            json!({"action": "read", "id": "read-test"}),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "success");
        assert_eq!(result.output["id"], "read-test");
        assert_eq!(result.output["name"], "Read Test");
        assert_eq!(result.output["type"], "project");
        assert_eq!(result.output["body"], "Body content here.");
    }

    #[tokio::test]
    async fn memory_read_nonexistent_returns_err() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let inv = make_invocation(
            "m6",
            "memory",
            json!({"action": "read", "id": "nonexistent"}),
        );
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "nonexistent memory should return Err");
    }

    // ── MemoryTool invoke: list ───────────────────────────────────────────

    #[tokio::test]
    async fn memory_list_empty() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let inv = make_invocation("m7", "memory", json!({"action": "list"}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "success");
        assert_eq!(result.output["count"], 0);
        assert_eq!(result.output["returned"], 0);
    }

    #[tokio::test]
    async fn memory_list_after_write() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());

        // Write
        let inv = make_invocation(
            "m8w",
            "memory",
            json!({
                "action": "write",
                "id": "list-test",
                "memory_type": "user",
                "name": "List Test",
                "description": "desc",
                "body": "body",
            }),
        );
        tool.invoke(inv).await.unwrap();

        // List
        let inv = make_invocation("m8l", "memory", json!({"action": "list"}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["count"], 1);
        assert_eq!(result.output["returned"], 1);
    }

    #[tokio::test]
    async fn memory_list_with_limit() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());

        // Write 3 memories
        for i in 0..3 {
            let inv = make_invocation(
                &format!("m9w{i}"),
                "memory",
                json!({
                    "action": "write",
                    "id": format!("limit-test-{i}"),
                    "memory_type": "user",
                    "name": format!("Item {i}"),
                    "description": "desc",
                    "body": "body",
                }),
            );
            tool.invoke(inv).await.unwrap();
        }

        // List with limit=2
        let inv = make_invocation("m9l", "memory", json!({"action": "list", "limit": 2}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["count"], 3);
        assert_eq!(result.output["returned"], 2);
    }

    // ── MemoryTool invoke: search ─────────────────────────────────────────

    #[tokio::test]
    async fn memory_search_text_matching() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());

        // Write a memory
        let inv = make_invocation(
            "m10w",
            "memory",
            json!({
                "action": "write",
                "id": "search-test",
                "memory_type": "feedback",
                "name": "Search Test Memory",
                "description": "about testing search",
                "body": "The body mentions rust programming.",
            }),
        );
        tool.invoke(inv).await.unwrap();

        // Search for it
        let inv = make_invocation(
            "m10s",
            "memory",
            json!({"action": "search", "query": "rust"}),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "success");
        assert!(result.output["count"].as_u64().unwrap_or(0) >= 1);
    }

    #[tokio::test]
    async fn memory_search_missing_query_returns_err() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let inv = make_invocation("m11", "memory", json!({"action": "search"}));
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "missing query should return Err");
    }

    // ── MemoryTool invoke: update ─────────────────────────────────────────

    #[tokio::test]
    async fn memory_update_name() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());

        // Write
        let inv = make_invocation(
            "m12w",
            "memory",
            json!({
                "action": "write",
                "id": "update-test",
                "memory_type": "user",
                "name": "Original Name",
                "description": "desc",
                "body": "body",
            }),
        );
        tool.invoke(inv).await.unwrap();

        // Update name
        let inv = make_invocation(
            "m12u",
            "memory",
            json!({"action": "update", "id": "update-test", "name": "Updated Name"}),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "success");

        // Verify
        let inv = make_invocation(
            "m12v",
            "memory",
            json!({"action": "read", "id": "update-test"}),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert_eq!(result.output["name"], "Updated Name");
    }

    #[tokio::test]
    async fn memory_update_status() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());

        // Write
        let inv = make_invocation(
            "m13w",
            "memory",
            json!({
                "action": "write",
                "id": "status-test",
                "memory_type": "user",
                "name": "Test",
                "description": "desc",
                "body": "body",
            }),
        );
        tool.invoke(inv).await.unwrap();

        // Update status
        let inv = make_invocation(
            "m13u",
            "memory",
            json!({"action": "update", "id": "status-test", "status": "obsolete"}),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
    }

    #[tokio::test]
    async fn memory_update_missing_id_returns_err() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let inv = make_invocation("m14", "memory", json!({"action": "update", "name": "x"}));
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "missing id should return Err");
    }

    // ── MemoryTool invoke: delete ─────────────────────────────────────────

    #[tokio::test]
    async fn memory_delete_after_write() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());

        // Write
        let inv = make_invocation(
            "m15w",
            "memory",
            json!({
                "action": "write",
                "id": "delete-test",
                "memory_type": "user",
                "name": "Delete Me",
                "description": "desc",
                "body": "body",
            }),
        );
        tool.invoke(inv).await.unwrap();

        // Delete
        let inv = make_invocation(
            "m15d",
            "memory",
            json!({"action": "delete", "id": "delete-test"}),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "success");

        // Read should fail
        let inv = make_invocation(
            "m15v",
            "memory",
            json!({"action": "read", "id": "delete-test"}),
        );
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "deleted memory should not be readable");
    }

    #[tokio::test]
    async fn memory_delete_missing_id_returns_err() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let inv = make_invocation("m16", "memory", json!({"action": "delete"}));
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "missing id should return Err");
    }

    // ── MemoryTool invoke: unsupported action ─────────────────────────────

    #[tokio::test]
    async fn memory_unsupported_action_returns_err() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let inv = make_invocation("m17", "memory", json!({"action": "unsupported"}));
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "unsupported action should return Err");
    }

    #[tokio::test]
    async fn memory_missing_action_returns_err() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let inv = make_invocation("m18", "memory", json!({}));
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "missing action should return Err");
    }

    // ── MemoryTool: db_path / resolve_model_paths ─────────────────────────

    #[test]
    fn memory_tool_db_path_returns_path() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let path = tool.db_path();
        assert!(
            path.to_string_lossy().contains("memories.db"),
            "db_path should contain memories.db: {path:?}"
        );
    }

    #[test]
    fn memory_tool_db_path_is_recomputed_not_cached() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());

        // db_path() now returns an owned PathBuf (recomputed each call) instead
        // of caching via OnceLock. Two calls should return equal paths but as
        // independent values, proving the computation runs fresh each time.
        let path1 = tool.db_path();
        let path2 = tool.db_path();
        assert_eq!(path1, path2, "same config should yield same path");
        // The paths are equal but distinct allocations (not a shared reference).
        assert!(
            !std::ptr::eq(path1.as_path(), path2.as_path()),
            "db_path should return a fresh value, not a cached reference"
        );
    }

    #[test]
    fn memory_tool_resolve_model_paths() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        let (model, tokenizer) = tool.resolve_model_paths();
        assert!(
            model.to_string_lossy().contains(".gguf"),
            "model path should contain .gguf: {model:?}"
        );
        assert!(
            tokenizer.to_string_lossy().contains("tokenizer.json"),
            "tokenizer path should contain tokenizer.json: {tokenizer:?}"
        );
    }

    #[test]
    fn memory_tool_try_generate_embedding_without_model() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_memory_tool(temp.path());
        // Without a real model file, this should return None.
        let result = tool.try_generate_embedding("test text");
        assert!(result.is_none(), "embedding without model should be None");
    }
}
