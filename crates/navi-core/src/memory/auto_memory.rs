use crate::db::{Db, DbConnection, OpenOptions, Row, RowResult, Value, params, params_from_iter};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The four memory types supported by the auto-memory system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryType {
    User,
    Feedback,
    Project,
    Reference,
}

impl MemoryType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Feedback => "feedback",
            Self::Project => "project",
            Self::Reference => "reference",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "user" => Some(Self::User),
            "feedback" => Some(Self::Feedback),
            "project" => Some(Self::Project),
            "reference" => Some(Self::Reference),
            _ => None,
        }
    }
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Status of a memory entry in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    Active,
    NeedsReview,
    Obsolete,
}

impl MemoryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::NeedsReview => "needs_review",
            Self::Obsolete => "obsolete",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "needs_review" => Some(Self::NeedsReview),
            "obsolete" => Some(Self::Obsolete),
            _ => None,
        }
    }
}

/// A memory entry stored in SQLite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub memory_type: MemoryType,
    pub name: String,
    pub description: String,
    pub body: String,
    pub confidence: f64,
    pub status: MemoryStatus,
    pub evidence: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_seen: String,
    pub expires_at: Option<String>,
}

/// A compact summary used for listing and prompt injection.
#[derive(Debug, Clone, Serialize)]
pub struct MemorySummary {
    pub id: String,
    pub memory_type: MemoryType,
    pub name: String,
    pub description: String,
    pub confidence: f64,
    pub status: MemoryStatus,
    pub updated_at: String,
}

/// Result of a dream consolidation pass on the auto-memory store.
#[derive(Debug, Clone, Serialize)]
pub struct ConsolidationReport {
    pub marked_stale: usize,
    pub duplicates_merged: usize,
    pub remaining_active: usize,
}

/// SQLite-backed persistent memory store.
///
/// Source of truth for all auto-memories. Search is text-based (SQL `LIKE`);
/// the project memory index is rendered on demand from this database.
#[derive(Clone)]
pub struct AutoMemoryStore {
    conn: Arc<Mutex<DbConnection>>,
    pub db_path: PathBuf,
}

impl std::fmt::Debug for AutoMemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoMemoryStore")
            .field("db_path", &self.db_path)
            .finish()
    }
}

impl AutoMemoryStore {
    /// Opens (or creates) the auto-memory SQLite database.
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create auto-memory directory: {:?}", parent))?;
        }

        let db = Db::open(db_path)
            .with_context(|| format!("Failed to open auto-memory database at {:?}", db_path))?;
        let conn = db.connect_with(&OpenOptions::none())?;
        configure_connection(&conn)?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: db_path.to_path_buf(),
        };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories (
                id          TEXT PRIMARY KEY,
                type        TEXT NOT NULL,
                name        TEXT NOT NULL,
                description TEXT NOT NULL,
                body        TEXT NOT NULL,
                confidence  REAL NOT NULL DEFAULT 1.0,
                status      TEXT NOT NULL DEFAULT 'active',
                evidence    TEXT,
                created_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL,
                last_seen   TEXT NOT NULL,
                expires_at  TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_memories_type
                ON memories(type);

            CREATE INDEX IF NOT EXISTS idx_memories_status
                ON memories(status);

            CREATE INDEX IF NOT EXISTS idx_memories_last_seen
                ON memories(last_seen);

            CREATE TABLE IF NOT EXISTS session_checkpoint (
                key         TEXT PRIMARY KEY,
                value       TEXT NOT NULL,
                updated_at  TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS session_notes (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                content     TEXT NOT NULL,
                created_at  TEXT NOT NULL
            );
            ",
        )?;
        Ok(())
    }

    /// Inserts or replaces a memory. Returns the stored entry.
    pub fn upsert(&self, entry: &MemoryEntry) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO memories
                (id, type, name, description, body, confidence, status,
                 evidence, created_at, updated_at, last_seen, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                entry.id.as_str(),
                entry.memory_type.as_str(),
                entry.name.as_str(),
                entry.description.as_str(),
                entry.body.as_str(),
                entry.confidence,
                entry.status.as_str(),
                serde_json::to_string(&entry.evidence).unwrap_or_else(|_| "[]".to_string()),
                entry.created_at.as_str(),
                entry.updated_at.as_str(),
                entry.last_seen.as_str(),
                entry.expires_at.as_deref(),
            ],
        )?;
        Ok(())
    }

    /// Retrieves a single memory by id.
    pub fn get(&self, id: &str) -> Result<Option<MemoryEntry>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;

        conn.query_row_optional(
            "SELECT id, type, name, description, body, confidence, status,
                    evidence, created_at, updated_at, last_seen, expires_at
             FROM memories WHERE id = ?1",
            params![id],
            row_to_entry,
        )
    }

    /// Lists all memories, optionally filtered by status.
    pub fn list(&self, status_filter: Option<MemoryStatus>) -> Result<Vec<MemorySummary>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;

        if let Some(status) = status_filter {
            conn.query_rows(
                "SELECT id, type, name, description, confidence, status, updated_at
                 FROM memories
                 WHERE status = ?1
                 ORDER BY type, name",
                params![status.as_str()],
                row_to_summary,
            )
        } else {
            conn.query_rows(
                "SELECT id, type, name, description, confidence, status, updated_at
                 FROM memories
                 ORDER BY type, name",
                (),
                row_to_summary,
            )
        }
    }

    /// Full-text search across name, description, and body using LIKE.
    pub fn search_text(&self, query: &str, limit: usize) -> Result<Vec<MemorySummary>> {
        let pattern = format!("%{}%", query.to_lowercase());
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;
        conn.query_rows(
            "SELECT id, type, name, description, confidence, status, updated_at
             FROM memories
             WHERE status = 'active'
               AND (LOWER(name) LIKE ?1
                 OR LOWER(description) LIKE ?1
                 OR LOWER(body) LIKE ?1)
             ORDER BY confidence DESC, updated_at DESC
             LIMIT ?2",
            params![pattern, limit as i64],
            row_to_summary,
        )
    }

    /// Updates the status of a memory (e.g. active → obsolete).
    pub fn set_status(&self, id: &str, status: MemoryStatus) -> Result<()> {
        let now = now_iso();
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;
        conn.execute(
            "UPDATE memories SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status.as_str(), now.as_str(), id],
        )?;
        Ok(())
    }

    /// Updates the body and/or description of a memory.
    pub fn update(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
        body: Option<&str>,
    ) -> Result<()> {
        let now = now_iso();
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;

        // Parameter 1 is always `updated_at`; the optional fields follow in
        // order, and the id goes last (used by the WHERE clause).
        let mut sets = vec!["updated_at = ?1".to_string()];
        let mut param_idx = 2usize;
        let mut values: Vec<Value> = vec![now.into()];

        if let Some(n) = name {
            sets.push(format!("name = ?{}", param_idx));
            values.push(n.to_string().into());
            param_idx += 1;
        }
        if let Some(d) = description {
            sets.push(format!("description = ?{}", param_idx));
            values.push(d.to_string().into());
            param_idx += 1;
        }
        if let Some(b) = body {
            sets.push(format!("body = ?{}", param_idx));
            values.push(b.to_string().into());
        }
        values.push(id.to_string().into());

        let sql = format!(
            "UPDATE memories SET {} WHERE id = ?{}",
            sets.join(", "),
            values.len()
        );
        conn.execute(&sql, params_from_iter(values))?;
        Ok(())
    }

    /// Deletes a memory permanently.
    pub fn delete(&self, id: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;
        conn.execute("DELETE FROM memories WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Counts active memories.
    pub fn count_active(&self) -> Result<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE status = 'active'",
            (),
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    // ── Dream consolidation operations ──────────────────────────────────

    /// Marks memories as obsolete if their `last_seen` is older than the given
    /// number of days. Returns the count of memories marked obsolete.
    pub fn mark_stale(&self, stale_days: u32) -> Result<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;
        let now = now_iso();
        // Simple heuristic: if last_seen starts with a date older than stale_days
        // from now, mark as needs_review. We use a simple string comparison since
        // our timestamps are ISO-ish.
        let cutoff = stale_iso_cutoff(stale_days);
        let count = conn.execute(
            "UPDATE memories
             SET status = 'needs_review', updated_at = ?1
             WHERE status = 'active'
               AND last_seen < ?2",
            params![now.as_str(), cutoff],
        )?;
        Ok(count as usize)
    }

    /// Detects and merges duplicate memories. Two memories are considered
    /// duplicates if they have the same `type` and identical `description`
    /// (case-insensitive). The older one is marked obsolete and the newer
    /// one's confidence is bumped. Returns the count of duplicates merged.
    pub fn deduplicate(&self) -> Result<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;

        // Find groups of duplicates: same type + same lower(description)
        let dup_groups: Vec<(String, String)> = conn.query_rows(
            "SELECT id, type, LOWER(description) as desc_lower, MIN(created_at) as oldest
             FROM memories
             WHERE status = 'active'
             GROUP BY type, desc_lower
             HAVING COUNT(*) > 1",
            (),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        let now = now_iso();
        let mut merged = 0;
        for (keep_id, type_str) in &dup_groups {
            // Mark all other active memories with same type+description as obsolete
            let marked = conn.execute(
                "UPDATE memories
                 SET status = 'obsolete', updated_at = ?1
                 WHERE status = 'active'
                   AND type = ?2
                   AND id != ?3
                   AND LOWER(description) = (
                       SELECT LOWER(description) FROM memories WHERE id = ?3
                   )",
                params![now.as_str(), type_str.as_str(), keep_id.as_str()],
            )?;
            merged += marked as usize;
        }

        Ok(merged)
    }

    /// Runs a full consolidation pass: mark stale, deduplicate.
    /// Returns a summary of what changed.
    pub fn consolidate(&self, stale_days: u32) -> Result<ConsolidationReport> {
        let stale_count = self.mark_stale(stale_days)?;
        let dup_count = self.deduplicate()?;
        let active = self.count_active()?;

        Ok(ConsolidationReport {
            marked_stale: stale_count,
            duplicates_merged: dup_count,
            remaining_active: active,
        })
    }

    /// Renders a compact markdown index from all active memories.
    /// Capped at 200 lines / 25KB for system prompt injection.
    pub fn render_index(&self) -> String {
        let memories = self.list(Some(MemoryStatus::Active)).unwrap_or_default();

        let mut content =
            String::from("# Project Memory Index\n\n_Auto-generated by NAVI auto-memory._\n\n");
        for m in &memories {
            let line = format!(
                "- **{}** (`{}`) — {} _(conf: {:.2})_\n",
                m.name, m.memory_type, m.description, m.confidence
            );
            if content.lines().count() >= 200 {
                content.push_str("\n_... additional memories omitted (index at capacity)._\n");
                break;
            }
            if content.len() + line.len() > 25_000 {
                content.push_str("\n_... additional memories omitted (index at capacity)._\n");
                break;
            }
            content.push_str(&line);
        }
        content
    }

    /// Returns a token-budgeted index string for prompt injection.
    pub fn build_prompt_context(&self, token_budget: usize) -> String {
        let index = self.render_index();
        if index.trim().is_empty() {
            return String::new();
        }
        let char_budget = token_budget * 4;
        if index.len() <= char_budget {
            index
        } else {
            let mut idx = char_budget;
            while idx > 0 && !index.is_char_boundary(idx) {
                idx -= 1;
            }
            format!("{}... [truncated]", &index[..idx])
        }
    }

    // ── Full entries listing (for model-based dream consolidation) ─────

    /// Lists all active memories with full body text, for model-based consolidation.
    pub fn list_full_entries(&self) -> Result<Vec<MemoryEntry>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;
        let rows = conn.query_rows(
            "SELECT id, type, name, description, body, confidence, status,
                    evidence, created_at, updated_at, last_seen, expires_at
             FROM memories WHERE status = 'active'
             ORDER BY type, name",
            (),
            row_to_entry,
        )?;
        Ok(rows)
    }

    /// Applies a consolidation action from the model: mark a memory obsolete.
    pub fn mark_obsolete(&self, id: &str) -> Result<()> {
        let now = now_iso();
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;
        conn.execute(
            "UPDATE memories SET status = 'obsolete', updated_at = ?1 WHERE id = ?2",
            params![now.as_str(), id],
        )?;
        Ok(())
    }

    /// Updates a memory's confidence and body (used by model-based consolidation).
    pub fn update_consolidated(
        &self,
        id: &str,
        body: Option<&str>,
        confidence: Option<f64>,
    ) -> Result<()> {
        let now = now_iso();
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;
        if let Some(b) = body {
            conn.execute(
                "UPDATE memories SET body = ?1, updated_at = ?2 WHERE id = ?3",
                params![b, now.as_str(), id],
            )?;
        }
        if let Some(c) = confidence {
            conn.execute(
                "UPDATE memories SET confidence = ?1, updated_at = ?2 WHERE id = ?3",
                params![c, now.as_str(), id],
            )?;
        }
        Ok(())
    }

    // ── Session checkpoint (replaces checkpoint.md) ─────────────────────

    /// Reads the current session checkpoint text. Returns empty string if none.
    pub fn read_checkpoint(&self) -> Result<String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;
        let text: Option<String> = conn.query_row_optional(
            "SELECT value FROM session_checkpoint WHERE key = 'current'",
            (),
            |row| row.get(0),
        )?;
        Ok(text.unwrap_or_default())
    }

    /// Writes the session checkpoint text, replacing any previous content.
    pub fn write_checkpoint(&self, content: &str) -> Result<()> {
        let now = now_iso();
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO session_checkpoint (key, value, updated_at)
             VALUES ('current', ?1, ?2)",
            params![content, now.as_str()],
        )?;
        Ok(())
    }

    // ── Session notes (replaces notes.md) ───────────────────────────────

    /// Appends a note to the session notes table.
    pub fn append_note(&self, content: &str) -> Result<()> {
        let now = now_iso();
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;
        conn.execute(
            "INSERT INTO session_notes (content, created_at) VALUES (?1, ?2)",
            params![content.trim(), now.as_str()],
        )?;
        Ok(())
    }

    /// Reads all session notes, oldest first.
    pub fn read_notes(&self) -> Result<String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;
        let rows: Vec<String> = conn.query_rows(
            "SELECT content FROM session_notes ORDER BY id ASC",
            (),
            |row| row.get(0),
        )?;
        Ok(rows.join("\n"))
    }

    /// Clears all session notes.
    pub fn clear_notes(&self) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("auto-memory lock poisoned: {e}"))?;
        conn.execute("DELETE FROM session_notes", ())?;
        Ok(())
    }

    /// Archives current notes by returning them and then clearing the table.
    pub fn archive_notes(&self) -> Result<String> {
        let notes = self.read_notes()?;
        if !notes.trim().is_empty() {
            self.clear_notes()?;
        }
        Ok(notes)
    }

    // ── Promoted facts (replaces MEMORY.md promote_facts) ───────────────

    /// Appends promoted facts to the project memory index by upserting
    /// a memory entry of type `project` with the given facts.
    pub fn promote_facts(&self, facts: &str) -> Result<()> {
        let trimmed = facts.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        let id = format!("promoted_{}", now_iso().replace(':', "-"));
        let entry = new_entry(
            &id,
            MemoryType::Project,
            "Promoted Facts",
            "Facts promoted from checkpoint",
            trimmed,
        );
        self.upsert(&entry)
    }
}

/// Configures a SQLite connection for concurrent access:
/// - WAL mode: allows multiple readers + 1 writer simultaneously
/// - busy_timeout: wait up to 5s on lock instead of failing immediately
/// - synchronous: NORMAL (durable enough for a WAL database)
pub fn configure_connection(conn: &DbConnection) -> Result<()> {
    conn.pragma_update("journal_mode", "WAL")?;
    conn.busy_timeout(Duration::from_millis(5000))?;
    conn.pragma_update("synchronous", "NORMAL")?;
    Ok(())
}

/// Creates a new MemoryEntry with sensible defaults.
pub fn new_entry(
    id: &str,
    memory_type: MemoryType,
    name: &str,
    description: &str,
    body: &str,
) -> MemoryEntry {
    let now = now_iso();
    MemoryEntry {
        id: id.to_string(),
        memory_type,
        name: name.to_string(),
        description: description.to_string(),
        body: body.to_string(),
        confidence: 1.0,
        status: MemoryStatus::Active,
        evidence: Vec::new(),
        created_at: now.clone(),
        updated_at: now.clone(),
        last_seen: now,
        expires_at: None,
    }
}

/// Sanitizes an id for use as a SQLite primary key / filename.
pub fn sanitize_id(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    iso_from_unix(secs)
}

/// Returns a cutoff timestamp string for memories older than `days`.
pub fn stale_iso_cutoff(days: u32) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cutoff_secs = now_secs.saturating_sub((days as u64) * 86400);
    iso_from_unix(cutoff_secs)
}

/// Converts a Unix timestamp (seconds since epoch) to an ISO 8601 string.
/// Uses a simple civil-from-days algorithm (Howard Hinnant) — no external crate needed.
fn iso_from_unix(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let remainder = secs % 86400;
    let hour = (remainder / 3600) as u32;
    let min = ((remainder % 3600) / 60) as u32;
    let sec = (remainder % 60) as u32;

    // Civil from days (epoch 1970-01-01 = day 0)
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, m, d, hour, min, sec
    )
}

fn row_to_entry(row: &Row) -> RowResult<MemoryEntry> {
    let evidence_str: String = row.get(7).unwrap_or_else(|_| "[]".to_string());
    let evidence: Vec<String> = serde_json::from_str(&evidence_str).unwrap_or_default();
    Ok(MemoryEntry {
        id: row.get(0)?,
        memory_type: MemoryType::from_str(&row.get::<String>(1)?).unwrap_or(MemoryType::User),
        name: row.get(2)?,
        description: row.get(3)?,
        body: row.get(4)?,
        confidence: row.get(5)?,
        status: MemoryStatus::from_str(&row.get::<String>(6)?).unwrap_or(MemoryStatus::Active),
        evidence,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        last_seen: row.get(10)?,
        expires_at: row.get(11).ok(),
    })
}

fn row_to_summary(row: &Row) -> RowResult<MemorySummary> {
    Ok(MemorySummary {
        id: row.get(0)?,
        memory_type: MemoryType::from_str(&row.get::<String>(1)?).unwrap_or(MemoryType::User),
        name: row.get(2)?,
        description: row.get(3)?,
        confidence: row.get(4)?,
        status: MemoryStatus::from_str(&row.get::<String>(5)?).unwrap_or(MemoryStatus::Active),
        updated_at: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> (AutoMemoryStore, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = AutoMemoryStore::open(&tmp.path().join("memories.db")).expect("open");
        (store, tmp)
    }

    #[test]
    fn test_memory_type_roundtrip() {
        for t in [
            MemoryType::User,
            MemoryType::Feedback,
            MemoryType::Project,
            MemoryType::Reference,
        ] {
            assert_eq!(MemoryType::from_str(t.as_str()), Some(t));
        }
    }

    #[test]
    fn test_upsert_and_get() {
        let (store, _tmp) = test_store();
        let entry = new_entry(
            "redis_tests",
            MemoryType::Feedback,
            "Redis for Tests",
            "Tests need Redis running",
            "Run Redis locally before pnpm test",
        );
        store.upsert(&entry).expect("upsert");

        let got = store.get("redis_tests").expect("get").expect("found");
        assert_eq!(got.name, "Redis for Tests");
        assert_eq!(got.memory_type, MemoryType::Feedback);
        assert_eq!(got.status, MemoryStatus::Active);
        assert_eq!(got.confidence, 1.0);
    }

    #[test]
    fn test_list_and_filter() {
        let (store, _tmp) = test_store();
        store
            .upsert(&new_entry(
                "m1",
                MemoryType::User,
                "Prefs",
                "Dark mode",
                "body1",
            ))
            .expect("upsert");
        store
            .upsert(&new_entry(
                "m2",
                MemoryType::Project,
                "Deadline",
                "Release July",
                "body2",
            ))
            .expect("upsert");

        let all = store.list(None).expect("list");
        assert_eq!(all.len(), 2);

        let active = store.list(Some(MemoryStatus::Active)).expect("list active");
        assert_eq!(active.len(), 2);

        store
            .set_status("m1", MemoryStatus::Obsolete)
            .expect("status");
        let active = store
            .list(Some(MemoryStatus::Active))
            .expect("list active after");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "m2");
    }

    #[test]
    fn test_search_text() {
        let (store, _tmp) = test_store();
        store
            .upsert(&new_entry(
                "redis",
                MemoryType::Feedback,
                "Redis Tests",
                "Need Redis for tests",
                "Always start Redis before running the test suite",
            ))
            .expect("upsert");
        store
            .upsert(&new_entry(
                "deadline",
                MemoryType::Project,
                "Release Date",
                "Ship by July 20",
                "The release is scheduled for July 20",
            ))
            .expect("upsert");

        let results = store.search_text("redis", 10).expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "redis");

        let results = store.search_text("july", 10).expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "deadline");
    }

    #[test]
    fn test_update() {
        let (store, _tmp) = test_store();
        store
            .upsert(&new_entry(
                "m1",
                MemoryType::User,
                "Old Name",
                "Old desc",
                "Old body",
            ))
            .expect("upsert");

        store
            .update("m1", Some("New Name"), Some("New desc"), Some("New body"))
            .expect("update");

        let got = store.get("m1").expect("get").expect("found");
        assert_eq!(got.name, "New Name");
        assert_eq!(got.description, "New desc");
        assert_eq!(got.body, "New body");
    }

    #[test]
    fn test_delete() {
        let (store, _tmp) = test_store();
        store
            .upsert(&new_entry("m1", MemoryType::User, "Test", "Desc", "Body"))
            .expect("upsert");
        assert!(store.get("m1").expect("get").is_some());
        store.delete("m1").expect("delete");
        assert!(store.get("m1").expect("get").is_none());
    }

    #[test]
    fn test_render_index() {
        let (store, _tmp) = test_store();
        store
            .upsert(&new_entry(
                "m1",
                MemoryType::User,
                "Prefs",
                "Dark mode",
                "body",
            ))
            .expect("upsert");
        store
            .upsert(&new_entry(
                "m2",
                MemoryType::Project,
                "Deadline",
                "Ship July",
                "body",
            ))
            .expect("upsert");

        let index = store.render_index();
        assert!(index.contains("Prefs"));
        assert!(index.contains("Deadline"));
        assert!(index.contains("user"));
        assert!(index.contains("project"));
    }

    #[test]
    fn test_count_active() {
        let (store, _tmp) = test_store();
        store
            .upsert(&new_entry("m1", MemoryType::User, "A", "B", "C"))
            .expect("upsert");
        store
            .upsert(&new_entry("m2", MemoryType::User, "D", "E", "F"))
            .expect("upsert");
        assert_eq!(store.count_active().expect("count"), 2);

        store
            .set_status("m1", MemoryStatus::Obsolete)
            .expect("status");
        assert_eq!(store.count_active().expect("count after"), 1);
    }

    #[test]
    fn test_sanitize_id() {
        assert_eq!(sanitize_id("my memory"), "my_memory");
        assert_eq!(sanitize_id("test-123"), "test-123");
        assert_eq!(sanitize_id("../evil"), ".._evil");
    }

    #[test]
    fn test_now_iso_is_real_date() {
        let ts = now_iso();
        // Should start with 20 (year 2000s) not 1970
        assert!(ts.starts_with("20"), "now_iso returned: {}", ts);
        // Should have format YYYY-MM-DDTHH:MM:SSZ (20 chars)
        assert_eq!(ts.len(), 20, "now_iso length: {} for {}", ts.len(), ts);
    }

    #[test]
    fn test_iso_from_unix_epoch() {
        // Unix epoch = 1970-01-01T00:00:00Z
        assert_eq!(iso_from_unix(0), "1970-01-01T00:00:00Z");
        // 2026-01-01 00:00:00 UTC = 1767225600
        assert_eq!(iso_from_unix(1767225600), "2026-01-01T00:00:00Z");
    }

    #[test]
    fn test_stale_iso_cutoff() {
        let cutoff = stale_iso_cutoff(30);
        // Should be a valid ISO date starting with 20
        assert!(cutoff.starts_with("20"), "stale cutoff: {}", cutoff);
        assert_eq!(cutoff.len(), 20);
    }
}
