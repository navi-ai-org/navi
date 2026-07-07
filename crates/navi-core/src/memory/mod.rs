pub mod auto_dream;
pub mod auto_memory;
pub mod checkpoint_writer;
pub mod embedding;
pub mod extract;
pub mod history_store;
pub mod maintenance;
pub mod memory_store;
pub mod rebuild_context;
pub mod schemas;

#[cfg(test)]
pub mod tests;

pub use auto_dream::AutoDreamState;
pub use auto_memory::{
    AutoMemoryStore, ConsolidationReport, MemoryEntry, MemoryStatus, MemorySummary, MemoryType,
    cosine_similarity, new_entry, sanitize_id,
};
pub use embedding::{
    Embedder, EmbeddingConfig, NoEmbedder, DEFAULT_MODEL_FILE, DEFAULT_MODEL_REPO,
    DEFAULT_TOKENIZER_FILE, DEFAULT_TOKENIZER_REPO, EMBED_DIM,
    create_embedder, embeddings_available,
};
pub use checkpoint_writer::run_checkpoint_writer;
pub use history_store::{HistoryEvent, HistoryStore, SessionSummary};
pub use maintenance::{
    DreamOptions, DreamResult, run_distill_maintenance, run_dream_maintenance,
    run_dream_maintenance_with_options,
};
pub use memory_store::{MemoryStore, write_atomic};
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
    pub fn new(
        project_dir: PathBuf,
        data_dir: PathBuf,
        config: &crate::config::MemoryConfig,
    ) -> Result<Self> {
        let store = MemoryStore::new(
            project_dir,
            data_dir.clone(),
            &config.root,
            &config.global_memory_path,
        );
        store.ensure_initialized()?;

        let resolved_sqlite_path =
            memory_store::resolve_memory_path(&config.history.sqlite_path, &store.memory_root);
        let history = HistoryStore::new(&resolved_sqlite_path)?;

        Ok(Self { store, history })
    }
}
