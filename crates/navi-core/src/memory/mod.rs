pub mod checkpoint_writer;
pub mod history_store;
pub mod maintenance;
pub mod memory_store;
pub mod rebuild_context;
pub mod schemas;

#[cfg(test)]
pub mod tests;

pub use checkpoint_writer::run_checkpoint_writer;
pub use history_store::{HistoryEvent, HistoryStore, SessionSummary};
pub use maintenance::{run_distill_maintenance, run_dream_maintenance};
pub use memory_store::{MemoryStore, resolve_path, write_atomic};
pub use rebuild_context::build_rebuild_context;
pub use schemas::SessionCheckpoint;

use anyhow::Result;
use std::path::PathBuf;

/// Orchestrates the memory system components (files + SQLite history).
#[derive(Debug, Clone)]
pub struct MemoryManager {
    pub store: MemoryStore,
    pub history: HistoryStore,
}

impl MemoryManager {
    /// Constructs and initializes a new `MemoryManager` from the configuration.
    pub fn new(project_dir: PathBuf, config: &crate::config::MemoryConfig) -> Result<Self> {
        let store = MemoryStore::new(project_dir, &config.root, &config.global_memory_path);
        store.ensure_initialized()?;

        let resolved_sqlite_path =
            memory_store::resolve_path(&config.history.sqlite_path, &store.project_dir);
        let history = HistoryStore::new(&resolved_sqlite_path)?;

        Ok(Self { store, history })
    }
}
