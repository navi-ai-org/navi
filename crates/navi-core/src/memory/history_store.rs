use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A thread-safe handle to the SQLite history database.
#[derive(Clone)]
pub struct HistoryStore {
    conn: Arc<Mutex<Connection>>,
    pub db_path: PathBuf,
}

impl std::fmt::Debug for HistoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryStore")
            .field("db_path", &self.db_path)
            .finish()
    }
}

impl HistoryStore {
    /// Opens the SQLite database at `db_path` and runs migrations/schema creation.
    pub fn new(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open database at {:?}", db_path))?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: db_path.to_path_buf(),
        };

        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        conn.execute(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                project_id TEXT,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                metadata_json TEXT
            );",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                role TEXT,
                content TEXT,
                tool_name TEXT,
                tool_input_json TEXT,
                tool_output TEXT,
                token_estimate INTEGER,
                created_at TEXT NOT NULL,
                metadata_json TEXT
            );",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS checkpoints (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                checkpoint_number INTEGER NOT NULL,
                utilization REAL NOT NULL,
                checkpoint_path TEXT NOT NULL,
                created_at TEXT NOT NULL,
                metadata_json TEXT
            );",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS rebuilds (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                previous_cycle INTEGER NOT NULL,
                new_cycle INTEGER NOT NULL,
                injected_context TEXT NOT NULL,
                created_at TEXT NOT NULL,
                metadata_json TEXT
            );",
            [],
        )?;

        Ok(())
    }

    fn get_now_rfc3339(&self) -> String {
        time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
    }

    /// Logs a session start.
    pub fn record_session_start(&self, session_id: &str, project_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = self.get_now_rfc3339();
        conn.execute(
            "INSERT OR IGNORE INTO sessions (id, project_id, started_at, metadata_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![session_id, project_id, now, json!({}).to_string()],
        )?;
        Ok(())
    }

    /// Logs a session end.
    pub fn record_session_end(&self, session_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = self.get_now_rfc3339();
        conn.execute(
            "UPDATE sessions SET ended_at = ?2 WHERE id = ?1",
            params![session_id, now],
        )?;
        Ok(())
    }

    /// Gets count of events for a session and event type.
    pub fn get_event_count(&self, session_id: &str, event_type: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(id) FROM events WHERE session_id = ?1 AND event_type = ?2",
            params![session_id, event_type],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Gets count of checkpoints for a session.
    pub fn get_checkpoint_count(&self, session_id: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(id) FROM checkpoints WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Gets count of rebuilds for a session.
    pub fn get_rebuild_count(&self, session_id: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(id) FROM rebuilds WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Gets the timestamp of the last checkpoint for a session.
    pub fn get_last_checkpoint_time(&self, session_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT created_at FROM checkpoints WHERE session_id = ?1 ORDER BY id DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![session_id], |row| row.get(0))?;
        if let Some(r) = rows.next() {
            Ok(Some(r?))
        } else {
            Ok(None)
        }
    }

    /// Records an event in the session timeline.
    #[allow(clippy::too_many_arguments)]
    pub fn record_event(
        &self,
        session_id: &str,
        event_type: &str,
        role: Option<&str>,
        content: Option<&str>,
        tool_name: Option<&str>,
        tool_input_json: Option<&str>,
        tool_output: Option<&str>,
        token_estimate: Option<i64>,
        metadata: Option<&serde_json::Value>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = self.get_now_rfc3339();

        // Compute sequence
        let sequence: i64 = conn.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM events WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;

        let meta = metadata.cloned().unwrap_or(json!({}));

        let content_redacted = content.map(|s| crate::security::redact_secrets(s));
        let tool_input_redacted = tool_input_json.map(|s| crate::security::redact_secrets(s));
        let tool_output_redacted = tool_output.map(|s| crate::security::redact_secrets(s));

        conn.execute(
            "INSERT INTO events (
                session_id, sequence, event_type, role, content,
                tool_name, tool_input_json, tool_output, token_estimate,
                created_at, metadata_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                session_id,
                sequence,
                event_type,
                role,
                content_redacted.as_deref(),
                tool_name,
                tool_input_redacted.as_deref(),
                tool_output_redacted.as_deref(),
                token_estimate,
                now,
                meta.to_string()
            ],
        )?;

        Ok(())
    }

    /// Records a checkpoint event.
    pub fn record_checkpoint(
        &self,
        session_id: &str,
        checkpoint_number: i64,
        utilization: f64,
        checkpoint_path: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = self.get_now_rfc3339();
        conn.execute(
            "INSERT INTO checkpoints (session_id, checkpoint_number, utilization, checkpoint_path, created_at, metadata_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![session_id, checkpoint_number, utilization, checkpoint_path, now, json!({}).to_string()],
        )?;
        Ok(())
    }

    /// Records a context rebuild event.
    pub fn record_rebuild(
        &self,
        session_id: &str,
        previous_cycle: i64,
        new_cycle: i64,
        injected_context: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = self.get_now_rfc3339();
        let injected_redacted = crate::security::redact_secrets(injected_context);
        conn.execute(
            "INSERT INTO rebuilds (session_id, previous_cycle, new_cycle, injected_context, created_at, metadata_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![session_id, previous_cycle, new_cycle, injected_redacted, now, json!({}).to_string()],
        )?;
        Ok(())
    }

    /// Searches raw history using a SQL LIKE query against event content, tool names, inputs, and outputs.
    pub fn search_history(
        &self,
        query: &str,
        session_id: Option<&str>,
        limit: Option<i64>,
    ) -> Result<Vec<HistoryEvent>> {
        let conn = self.conn.lock().unwrap();
        let limit_val = limit.unwrap_or(50);
        let like_query = format!("%{}%", query);

        let mut sql = "SELECT id, session_id, sequence, event_type, role, content, tool_name, tool_input_json, tool_output, token_estimate, created_at, metadata_json FROM events WHERE (content LIKE ?1 OR tool_name LIKE ?1 OR tool_input_json LIKE ?1 OR tool_output LIKE ?1)".to_string();

        let mut params_vec: Vec<rusqlite::types::Value> = vec![like_query.into()];

        if let Some(sid) = session_id {
            sql.push_str(" AND session_id = ?2");
            params_vec.push(sid.to_string().into());
        }

        sql.push_str(" ORDER BY created_at DESC LIMIT ?");
        params_vec.push(limit_val.into());

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params_vec), |row| {
            Ok(HistoryEvent {
                id: row.get(0)?,
                session_id: row.get(1)?,
                sequence: row.get(2)?,
                event_type: row.get(3)?,
                role: row.get(4)?,
                content: row.get(5)?,
                tool_name: row.get(6)?,
                tool_input_json: row.get(7)?,
                tool_output: row.get(8)?,
                token_estimate: row.get(9)?,
                created_at: row.get(10)?,
                metadata_json: row.get(11)?,
            })
        })?;

        let mut results = Vec::new();
        for r in rows {
            results.push(r?);
        }
        Ok(results)
    }

    /// Retrieves recent events for a given session.
    pub fn get_recent_events(
        &self,
        session_id: &str,
        limit: Option<i64>,
    ) -> Result<Vec<HistoryEvent>> {
        let conn = self.conn.lock().unwrap();
        let limit_val = limit.unwrap_or(50);
        let mut stmt = conn.prepare(
            "SELECT id, session_id, sequence, event_type, role, content, tool_name, tool_input_json, tool_output, token_estimate, created_at, metadata_json 
             FROM events 
             WHERE session_id = ?1 
             ORDER BY sequence DESC 
             LIMIT ?2"
        )?;

        let rows = stmt.query_map(params![session_id, limit_val], |row| {
            Ok(HistoryEvent {
                id: row.get(0)?,
                session_id: row.get(1)?,
                sequence: row.get(2)?,
                event_type: row.get(3)?,
                role: row.get(4)?,
                content: row.get(5)?,
                tool_name: row.get(6)?,
                tool_input_json: row.get(7)?,
                tool_output: row.get(8)?,
                token_estimate: row.get(9)?,
                created_at: row.get(10)?,
                metadata_json: row.get(11)?,
            })
        })?;

        let mut results = Vec::new();
        for r in rows {
            results.push(r?);
        }
        // Reverse so they are chronological
        results.reverse();
        Ok(results)
    }

    /// Retrieves a single event by ID.
    pub fn get_event(&self, event_id: i64) -> Result<Option<HistoryEvent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, sequence, event_type, role, content, tool_name, tool_input_json, tool_output, token_estimate, created_at, metadata_json 
             FROM events 
             WHERE id = ?1"
        )?;
        let mut rows = stmt.query_map(params![event_id], |row| {
            Ok(HistoryEvent {
                id: row.get(0)?,
                session_id: row.get(1)?,
                sequence: row.get(2)?,
                event_type: row.get(3)?,
                role: row.get(4)?,
                content: row.get(5)?,
                tool_name: row.get(6)?,
                tool_input_json: row.get(7)?,
                tool_output: row.get(8)?,
                token_estimate: row.get(9)?,
                created_at: row.get(10)?,
                metadata_json: row.get(11)?,
            })
        })?;

        if let Some(r) = rows.next() {
            Ok(Some(r?))
        } else {
            Ok(None)
        }
    }

    /// Returns a list of all session summaries/IDs logged in history.
    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, started_at, ended_at, metadata_json FROM sessions ORDER BY started_at DESC"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SessionSummary {
                id: row.get(0)?,
                project_id: row.get(1)?,
                started_at: row.get(2)?,
                ended_at: row.get(3)?,
                metadata_json: row.get(4)?,
            })
        })?;
        let mut res = Vec::new();
        for r in rows {
            res.push(r?);
        }
        Ok(res)
    }

    /// Performs diagnostic checks on the database health and structure.
    pub fn doctor_check(&self) -> Result<Vec<String>> {
        let mut logs = Vec::new();
        let conn = self.conn.lock().unwrap();
        logs.push("DB connection is open.".to_string());

        let tables = vec!["sessions", "events", "checkpoints", "rebuilds"];
        for table in tables {
            let exists: Result<bool, _> = conn.query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |_| Ok(true),
            );
            match exists {
                Ok(true) => logs.push(format!("Table '{}' exists.", table)),
                _ => logs.push(format!("ERROR: Table '{}' does not exist.", table)),
            }
        }

        let integrity: Result<String, _> =
            conn.query_row("PRAGMA integrity_check", [], |row| row.get(0));
        match integrity {
            Ok(ref val) if val == "ok" => logs.push("DB integrity check passed (ok).".to_string()),
            Ok(val) => logs.push(format!("ERROR: DB integrity check failed: {}", val)),
            Err(e) => logs.push(format!("ERROR: DB integrity check failed: {}", e)),
        }

        Ok(logs)
    }
}

/// A serialized event record returned from the history store.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryEvent {
    pub id: i64,
    pub session_id: String,
    pub sequence: i64,
    pub event_type: String,
    pub role: Option<String>,
    pub content: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input_json: Option<String>,
    pub tool_output: Option<String>,
    pub token_estimate: Option<i64>,
    pub created_at: String,
    pub metadata_json: String,
}

/// A summary of a session.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub project_id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub metadata_json: String,
}
