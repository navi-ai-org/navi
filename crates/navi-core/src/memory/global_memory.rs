use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// SQLite-backed store for cross-project (global) memories.
///
/// This replaces the legacy `global-memory.md` file. It lives at
/// `{data_dir}/memory/global-memory.db` and shares the same schema as
/// the per-project `AutoMemoryStore`, but without embeddings or session
/// checkpoint/notes tables — it only holds durable global facts.
#[derive(Clone)]
pub struct GlobalMemoryStore {
    conn: Arc<Mutex<Connection>>,
    pub db_path: PathBuf,
}

impl std::fmt::Debug for GlobalMemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlobalMemoryStore")
            .field("db_path", &self.db_path)
            .finish()
    }
}

impl GlobalMemoryStore {
    /// Opens (or creates) the global memory database.
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open global memory database at {:?}", db_path))?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: db_path.to_path_buf(),
        };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS global_memories (
                id          TEXT PRIMARY KEY,
                type        TEXT NOT NULL,
                name        TEXT NOT NULL,
                description TEXT NOT NULL,
                body        TEXT NOT NULL,
                confidence  REAL NOT NULL DEFAULT 1.0,
                status      TEXT NOT NULL DEFAULT 'active',
                created_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL,
                last_seen   TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_global_memories_status
                ON global_memories(status);
            ",
        )?;
        Ok(())
    }

    /// Reads all active global memories and renders them as a markdown string.
    pub fn read_index(&self) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT name, type, description, confidence
             FROM global_memories
             WHERE status = 'active'
             ORDER BY type, name",
        )?;
        let rows: Vec<(String, String, String, f64)> = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        if rows.is_empty() {
            return Ok(String::new());
        }

        let mut content = String::from("# Global Memory Index\n\n");
        for (name, mem_type, description, confidence) in &rows {
            content.push_str(&format!(
                "- **{}** (`{}`) — {} _(conf: {:.2})_\n",
                name, mem_type, description, confidence
            ));
        }
        Ok(content)
    }

    /// Replaces the entire global memory set with the given markdown text.
    /// This is used by the dream maintenance to apply consolidated output.
    /// Parses the markdown into individual entries.
    pub fn write_from_markdown(&self, markdown: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        // Clear existing active memories
        conn.execute("DELETE FROM global_memories", [])?;

        let now = crate::memory::auto_memory::now_iso();
        let mut id_counter = 0u32;
        for line in markdown.lines() {
            let trimmed = line.trim();
            // Parse lines like "- **Name** (`type`) — description"
            if let Some(rest) = trimmed.strip_prefix("- **") {
                if let Some(end_name) = rest.find("**") {
                    let name = &rest[..end_name];
                    let after_name = &rest[end_name + 2..];
                    if let Some(start_type) = after_name.find("(`") {
                        if let Some(end_type) = after_name[start_type + 2..].find("`)") {
                            let mem_type = &after_name[start_type + 2..start_type + 2 + end_type];
                            let description = after_name[start_type + 2 + end_type + 2..]
                                .trim_start_matches('—')
                                .trim();

                            id_counter += 1;
                            let id = format!("global_{}", id_counter);
                            conn.execute(
                                "INSERT OR REPLACE INTO global_memories
                                    (id, type, name, description, body, confidence, status,
                                     created_at, updated_at, last_seen)
                                 VALUES (?1, ?2, ?3, ?4, ?5, 1.0, 'active', ?6, ?6, ?6)",
                                params![
                                    id,
                                    mem_type,
                                    name,
                                    description,
                                    description,
                                    now,
                                ],
                            )?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Counts active global memories.
    pub fn count_active(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM global_memories WHERE status = 'active'",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Marks memories as obsolete if their `last_seen` is older than the given
    /// number of days.
    pub fn mark_stale(&self, stale_days: u32) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let now = crate::memory::auto_memory::now_iso();
        let cutoff = crate::memory::auto_memory::stale_iso_cutoff(stale_days);
        let count = conn.execute(
            "UPDATE global_memories
             SET status = 'needs_review', updated_at = ?1
             WHERE status = 'active'
               AND last_seen < ?2",
            params![now, cutoff],
        )?;
        Ok(count)
    }
}
