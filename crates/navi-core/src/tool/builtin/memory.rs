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
use crate::memory::embedding::{get_cached_embedder, embeddings_available};
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
        let manager = MemoryManager::new(
            self.project_root.clone(),
            loaded_config.data_dir.clone(),
            &loaded_config.config.memory,
        )?;

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
    /// Cached db_path — computed once on first use.
    db_path_cache: std::sync::OnceLock<PathBuf>,
}

impl MemoryTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self {
            project_root,
            db_path_cache: std::sync::OnceLock::new(),
        }
    }

    fn db_path(&self) -> &PathBuf {
        self.db_path_cache.get_or_init(|| {
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
        })
    }

    fn open_store(&self) -> Result<AutoMemoryStore> {
        AutoMemoryStore::open(self.db_path())
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
                tracing::debug!("Embedding generation failed: {}, falling back to text search", e);
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
                let id = sanitize_id(&raw_id);
                let memory_type_str = helpers::required_string(&invocation.input, "memory_type")?;
                let memory_type = MemoryType::from_str(&memory_type_str)
                    .context(format!("Invalid memory_type: {memory_type_str}"))?;
                let name = helpers::required_string(&invocation.input, "name")?;
                let description = helpers::required_string(&invocation.input, "description")?;
                let body = helpers::required_string(&invocation.input, "body")?;

                let entry = new_entry(&id, memory_type, &name, &description, &body);
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
                let id = sanitize_id(&helpers::required_string(&invocation.input, "id")?);
                let entry = store.get(&id)?
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
                let limit = invocation.input.get("limit").and_then(|v| v.as_i64()).unwrap_or(50) as usize;
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
                let limit = invocation.input.get("limit").and_then(|v| v.as_i64()).unwrap_or(20) as usize;

                // Try semantic search first (embeddings), fall back to text matching
                let search_results: Vec<(String, String, String, crate::memory::MemoryType, f64, String)> =
                    if let Some(query_emb) = self.try_generate_embedding(&query) {
                        let semantic = store.search_semantic(&query_emb, 0.3, limit)?;
                        if !semantic.is_empty() {
                            semantic
                                .into_iter()
                                .map(|(m, score)| {
                                    (m.id, m.name, m.description, m.memory_type, m.confidence, format!("semantic:{:.3}", score))
                                })
                                .collect()
                        } else {
                            // Semantic returned nothing — fall back to text
                            let text_results = store.search_text(&query, limit)?;
                            text_results
                                .into_iter()
                                .map(|m| {
                                    (m.id, m.name, m.description, m.memory_type, m.confidence, "text_match".to_string())
                                })
                                .collect()
                        }
                    } else {
                        // No embeddings available — text search only
                        let text_results = store.search_text(&query, limit)?;
                        text_results
                            .into_iter()
                            .map(|m| {
                                (m.id, m.name, m.description, m.memory_type, m.confidence, "text_match".to_string())
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
                let id = sanitize_id(&helpers::required_string(&invocation.input, "id")?);

                if let Some(status_str) = invocation.input.get("status").and_then(|v| v.as_str()) {
                    if let Some(status) = MemoryStatus::from_str(status_str) {
                        store.set_status(&id, status)?;
                    }
                }

                let name = invocation.input.get("name").and_then(|v| v.as_str());
                let description = invocation.input.get("description").and_then(|v| v.as_str());
                let body = invocation.input.get("body").and_then(|v| v.as_str());

                store.update(&id, name, description, body)?;

                // Regenerate embedding if body/description/name changed
                let content_changed = name.is_some() || description.is_some() || body.is_some();
                let re_embedded = if content_changed {
                    if let Some(entry) = store.get(&id)? {
                        let embed_text = format!(
                            "{}\n{}\n{}",
                            entry.name, entry.description, entry.body
                        );
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
                let id = sanitize_id(&helpers::required_string(&invocation.input, "id")?);
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
