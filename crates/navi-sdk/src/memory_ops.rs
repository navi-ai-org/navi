//! Extended memory operations on [`NaviEngine`] (status, doctor, init, history, dream, distill).
//!
//! CRUD lives in `engine.rs`; this module covers CLI-parity maintenance APIs for desktop.

use navi_core::memory::{
    DreamOptions, HistoryEvent, MemoryManager, build_rebuild_context, run_checkpoint_writer,
    run_distill_maintenance, run_dream_maintenance_with_options,
};
use navi_core::{ModelMessage, ModelRole, effective_context_window};
use serde::{Deserialize, Serialize};

use crate::engine::NaviEngine;
use crate::tooling::build_provider_for_project_config;
use crate::types::NaviError;

type Result<T> = std::result::Result<T, NaviError>;

/// Serializable memory system status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStatusReport {
    pub memory_root: String,
    pub auto_memory_db: String,
    pub global_memory_db: String,
    pub history_db: String,
    pub enabled: bool,
    pub active_memories: usize,
    pub last_session_id: Option<String>,
    pub rebuild_count: i64,
    pub checkpoint_count: i64,
    pub last_checkpoint_time: Option<String>,
    pub message_event_count: i64,
    pub lines: Vec<String>,
}

/// Doctor diagnostics for the memory subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryDoctorReport {
    pub ok: bool,
    pub lines: Vec<String>,
}

/// Result of `memory_init`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryInitReport {
    pub memory_root: String,
    pub auto_memory_db: String,
    pub active_memories: usize,
    pub lines: Vec<String>,
}

/// Result of dream maintenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryDreamReport {
    pub output_dir: String,
    pub project_memory_path: String,
    pub global_memory_path: String,
    pub report_path: String,
    pub applied: bool,
    pub marked_stale: Option<usize>,
    pub duplicates_merged: Option<usize>,
    pub remaining_active: Option<usize>,
}

impl NaviEngine {
    fn memory_manager(&self) -> Result<MemoryManager> {
        let loaded = self.loaded_config();
        MemoryManager::new(
            self.inner.project_dir.clone(),
            loaded.data_dir.clone(),
            &loaded.config.memory,
        )
        .map_err(|e| NaviError::Config(format!("open memory manager: {e}")))
    }

    /// Status of the auto-memory / history system for this project.
    pub fn memory_status(&self) -> Result<MemoryStatusReport> {
        let loaded = self.loaded_config();
        let manager = self.memory_manager()?;
        let mut lines = Vec::new();
        let active = manager.auto_memory.count_active().unwrap_or(0);
        lines.push(format!(
            "Memory root: {}",
            manager.store.memory_root.display()
        ));
        lines.push(format!("Active memories: {active}"));

        let sessions = manager
            .history
            .list_sessions()
            .map_err(|e| NaviError::Config(format!("list memory history sessions: {e}")))?;
        let (
            last_session_id,
            rebuild_count,
            checkpoint_count,
            last_checkpoint_time,
            message_event_count,
        ) = if let Some(session) = sessions.first() {
            let rebuild = manager.history.get_rebuild_count(&session.id).unwrap_or(0);
            let checkpoint = manager
                .history
                .get_checkpoint_count(&session.id)
                .unwrap_or(0);
            let last_cp = manager
                .history
                .get_last_checkpoint_time(&session.id)
                .ok()
                .flatten();
            let events = manager
                .history
                .get_event_count(&session.id, "message")
                .unwrap_or(0);
            (
                Some(session.id.clone()),
                rebuild,
                checkpoint,
                last_cp,
                events,
            )
        } else {
            (None, 0, 0, None, 0)
        };

        Ok(MemoryStatusReport {
            memory_root: manager.store.memory_root.display().to_string(),
            auto_memory_db: manager.auto_memory.db_path.display().to_string(),
            global_memory_db: manager.global_memory.db_path.display().to_string(),
            history_db: manager.history.db_path.display().to_string(),
            enabled: loaded.config.memory.enabled,
            active_memories: active,
            last_session_id,
            rebuild_count,
            checkpoint_count,
            last_checkpoint_time,
            message_event_count,
            lines,
        })
    }

    /// Validate memory paths, SQLite accessibility, and config.
    pub fn memory_doctor(&self) -> Result<MemoryDoctorReport> {
        let loaded = self.loaded_config();
        let manager = self.memory_manager()?;
        let mut lines = Vec::new();
        let mut ok = true;

        lines.push(format!(
            "Memory config enabled: {}",
            loaded.config.memory.enabled
        ));

        let root = &manager.store.memory_root;
        if root.exists() {
            lines.push(format!("[OK] Memory root exists: {}", root.display()));
        } else {
            lines.push(format!(
                "[WARN] Memory root missing (will be created): {}",
                root.display()
            ));
        }

        match manager.auto_memory.count_active() {
            Ok(n) => lines.push(format!("[OK] Auto-memory DB readable ({n} active)")),
            Err(e) => {
                ok = false;
                lines.push(format!("[FAIL] Auto-memory DB: {e}"));
            }
        }

        match manager.history.doctor_check() {
            Ok(checks) => {
                for c in checks {
                    lines.push(c);
                }
            }
            Err(e) => {
                ok = false;
                lines.push(format!("[FAIL] History doctor: {e}"));
            }
        }

        lines.push(if ok {
            "Doctor: OK".into()
        } else {
            "Doctor: issues found".into()
        });

        Ok(MemoryDoctorReport { ok, lines })
    }

    /// Ensure memory directories/DB exist.
    pub async fn memory_init(&self) -> Result<MemoryInitReport> {
        let manager = self.memory_manager()?;
        let mut lines = Vec::new();
        let memory_root = manager.store.memory_root.clone();

        if !memory_root.exists() {
            std::fs::create_dir_all(&memory_root)
                .map_err(|e| NaviError::Config(format!("create memory root: {e}")))?;
            lines.push(format!("Created memory root: {}", memory_root.display()));
        } else {
            lines.push(format!("Memory root exists: {}", memory_root.display()));
        }

        let active = manager.auto_memory.count_active().unwrap_or(0);
        lines.push(format!(
            "Auto-memory DB: {} ({active} active)",
            manager.auto_memory.db_path.display()
        ));

        Ok(MemoryInitReport {
            memory_root: memory_root.display().to_string(),
            auto_memory_db: manager.auto_memory.db_path.display().to_string(),
            active_memories: active,
            lines,
        })
    }

    /// Search raw session history events.
    pub fn memory_history_search(
        &self,
        query: &str,
        session_id: Option<&str>,
        limit: Option<i64>,
    ) -> Result<Vec<HistoryEvent>> {
        let manager = self.memory_manager()?;
        manager
            .history
            .search_history(query, session_id, limit)
            .map_err(|e| NaviError::Config(format!("search memory history: {e}")))
    }

    /// Run model-based dream consolidation.
    pub async fn memory_dream(
        &self,
        apply: bool,
        sessions: usize,
        instructions: Option<String>,
    ) -> Result<MemoryDreamReport> {
        let loaded = self.loaded_config();
        let manager = self.memory_manager()?;
        let provider = build_provider_for_project_config(&loaded, &self.inner.project_dir)?;
        let model_name = loaded.config.model.name.clone();
        let result = run_dream_maintenance_with_options(
            &manager.auto_memory,
            &manager.global_memory,
            &manager.history,
            provider.as_ref(),
            &model_name,
            DreamOptions {
                session_limit: sessions.max(1),
                instructions,
                apply,
            },
        )
        .await
        .map_err(|e| NaviError::Config(format!("memory dream maintenance failed: {e}")))?;

        let (marked_stale, duplicates_merged, remaining_active) =
            if let Some(ref am) = result.auto_memory_report {
                (
                    Some(am.marked_stale),
                    Some(am.duplicates_merged),
                    Some(am.remaining_active),
                )
            } else {
                (None, None, None)
            };

        Ok(MemoryDreamReport {
            output_dir: result.output_dir.display().to_string(),
            project_memory_path: result.project_memory_path.display().to_string(),
            global_memory_path: result.global_memory_path.display().to_string(),
            report_path: result.report_path.display().to_string(),
            applied: result.applied,
            marked_stale,
            duplicates_merged,
            remaining_active,
        })
    }

    /// Run process distillation maintenance.
    pub async fn memory_distill(&self) -> Result<()> {
        let loaded = self.loaded_config();
        let manager = self.memory_manager()?;
        let provider = build_provider_for_project_config(&loaded, &self.inner.project_dir)?;
        let model_name = loaded.config.model.name.clone();
        run_distill_maintenance(
            &manager.auto_memory,
            &manager.history,
            provider.as_ref(),
            &model_name,
        )
        .await
        .map_err(|e| NaviError::Config(format!("memory distill maintenance failed: {e}")))
    }

    /// Manually run checkpoint writer for the latest (or temporary) session.
    pub async fn memory_checkpoint(&self) -> Result<String> {
        let loaded = self.loaded_config();
        let manager = self.memory_manager()?;
        let sessions = manager
            .history
            .list_sessions()
            .map_err(|e| NaviError::Config(format!("list memory history sessions: {e}")))?;
        let session_id = if let Some(s) = sessions.first() {
            s.id.clone()
        } else {
            format!("manual-{}", navi_core::session::current_unix_timestamp())
        };
        let events = manager
            .history
            .get_recent_events(&session_id, None)
            .map_err(|e| {
                NaviError::Config(format!("load recent history for '{session_id}': {e}"))
            })?;
        let messages = history_to_model_messages(&events);
        let provider = build_provider_for_project_config(&loaded, &self.inner.project_dir)?;
        let model_name = loaded.config.model.name.clone();
        run_checkpoint_writer(
            &session_id,
            &messages,
            &manager.auto_memory,
            provider.as_ref(),
            &model_name,
        )
        .await
        .map_err(|e| {
            NaviError::Config(format!(
                "memory checkpoint writer for '{session_id}' failed: {e}"
            ))
        })?;
        Ok(session_id)
    }

    /// Preview rebuild context that would be injected (debug / desktop inspector).
    pub fn memory_rebuild_preview(&self) -> Result<String> {
        let loaded = self.loaded_config();
        let manager = self.memory_manager()?;
        let sessions = manager
            .history
            .list_sessions()
            .map_err(|e| NaviError::Config(format!("list memory history sessions: {e}")))?;
        let session_id = sessions.first().map(|s| s.id.clone()).ok_or_else(|| {
            NaviError::Config("no session in history — cannot build rebuild preview".into())
        })?;
        let events = manager
            .history
            .get_recent_events(&session_id, None)
            .map_err(|e| {
                NaviError::Config(format!("load recent history for '{session_id}': {e}"))
            })?;
        let messages = history_to_model_messages(&events);
        let context_window = effective_context_window(&loaded.config);
        Ok(build_rebuild_context(
            &messages,
            &manager.auto_memory,
            &manager.global_memory,
            context_window,
            loaded.config.memory.injected_context_token_budget,
        ))
    }
}

fn history_to_model_messages(events: &[HistoryEvent]) -> Vec<ModelMessage> {
    events
        .iter()
        .filter(|e| e.event_type == "message")
        .map(|e| {
            let role = match e.role.as_deref() {
                Some("system") => ModelRole::System,
                Some("assistant") => ModelRole::Assistant,
                Some("tool") => ModelRole::Tool,
                _ => ModelRole::User,
            };
            ModelMessage {
                role,
                content: e.content.clone().unwrap_or_default(),
                content_parts: Vec::new(),
                tool_call_id: None,
                tool_name: e.tool_name.clone(),
                tool_calls: Vec::new(),
                created_at: None,
                thinking_content: None,
            }
        })
        .collect()
}
