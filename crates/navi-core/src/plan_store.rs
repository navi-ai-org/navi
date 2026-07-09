//! SQLite-backed plan persistence (reviewable work plans).
//!
//! Replaces the per-plan JSON files under `data_dir/plans/<project>/` with a
//! single SQLite database so plans survive restarts and support line comments.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum plans returned by list.
pub const MAX_PLANS: usize = 20;
/// Maximum steps per plan.
pub const MAX_STEPS: usize = 50;

/// A work plan with checklist steps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub steps: Vec<PlanStep>,
    pub status: PlanStatus,
    pub created_at: u64,
    pub updated_at: u64,
    /// Optional freeform body used for line-oriented review (markdown).
    #[serde(default)]
    pub body_markdown: String,
    /// User line comments from the review modal.
    #[serde(default)]
    pub comments: Vec<PlanLineComment>,
    /// Project scope key (hash of project root).
    #[serde(default)]
    pub project_id: String,
    /// Session that created/last reviewed the plan.
    #[serde(default)]
    pub session_id: String,
}

/// A single step in a plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub description: String,
    pub completed: bool,
    #[serde(default)]
    pub notes: String,
}

/// Plan lifecycle status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PlanStatus {
    Active,
    Completed,
    Abandoned,
    /// Awaiting user review in the TUI modal.
    Proposed,
}

impl std::fmt::Display for PlanStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanStatus::Active => write!(f, "active"),
            PlanStatus::Completed => write!(f, "completed"),
            PlanStatus::Abandoned => write!(f, "abandoned"),
            PlanStatus::Proposed => write!(f, "proposed"),
        }
    }
}

impl PlanStatus {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "active" => Ok(Self::Active),
            "completed" => Ok(Self::Completed),
            "abandoned" => Ok(Self::Abandoned),
            "proposed" => Ok(Self::Proposed),
            _ => Err(anyhow::anyhow!(
                "invalid status '{s}', use active/completed/abandoned/proposed"
            )),
        }
    }
}

/// Inline comment on a line range of the plan preview.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanLineComment {
    /// Inclusive start line (0-based) in the rendered plan view.
    pub start_line: usize,
    /// Inclusive end line (0-based).
    pub end_line: usize,
    pub text: String,
}

/// Thread-safe SQLite plan store.
#[derive(Clone)]
pub struct PlanStore {
    conn: Arc<Mutex<Connection>>,
    pub db_path: PathBuf,
}

impl std::fmt::Debug for PlanStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlanStore")
            .field("db_path", &self.db_path)
            .finish()
    }
}

impl PlanStore {
    /// Open (or create) the plans database at `db_path`.
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create plan store dir {}", parent.display()))?;
        }
        let conn = Connection::open(db_path)
            .with_context(|| format!("open plans db {}", db_path.display()))?;
        crate::memory::auto_memory::configure_connection(&conn)?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: db_path.to_path_buf(),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Default path: `{data_dir}/plans.sqlite`.
    pub fn open_default(data_dir: &Path) -> Result<Self> {
        Self::open(&data_dir.join("plans.sqlite"))
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS plans (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                session_id TEXT NOT NULL DEFAULT '',
                title TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                body_markdown TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL,
                steps_json TEXT NOT NULL,
                comments_json TEXT NOT NULL DEFAULT '[]',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_plans_project_status
                ON plans(project_id, status, updated_at DESC);
            "#,
        )?;
        Ok(())
    }

    /// Import legacy JSON plan files from `data_dir/plans/<project_hash>/*.json`.
    pub fn migrate_json_dir(&self, plans_root: &Path) -> Result<usize> {
        if !plans_root.exists() {
            return Ok(0);
        }
        let mut imported = 0usize;
        for project_entry in fs::read_dir(plans_root)? {
            let project_entry = project_entry?;
            if !project_entry.file_type()?.is_dir() {
                continue;
            }
            let project_id = project_entry.file_name().to_string_lossy().to_string();
            for entry in fs::read_dir(project_entry.path())? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let Ok(content) = fs::read_to_string(&path) else {
                    continue;
                };
                let Ok(mut plan) = serde_json::from_str::<Plan>(&content) else {
                    continue;
                };
                if plan.project_id.is_empty() {
                    plan.project_id = project_id.clone();
                }
                if self.get(&plan.id)?.is_none() {
                    self.upsert(&plan)?;
                    imported += 1;
                }
            }
        }
        Ok(imported)
    }

    pub fn upsert(&self, plan: &Plan) -> Result<()> {
        let steps_json = serde_json::to_string(&plan.steps)?;
        let comments_json = serde_json::to_string(&plan.comments)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO plans (
                id, project_id, session_id, title, description, body_markdown,
                status, steps_json, comments_json, created_at, updated_at
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
            ON CONFLICT(id) DO UPDATE SET
                project_id=excluded.project_id,
                session_id=excluded.session_id,
                title=excluded.title,
                description=excluded.description,
                body_markdown=excluded.body_markdown,
                status=excluded.status,
                steps_json=excluded.steps_json,
                comments_json=excluded.comments_json,
                updated_at=excluded.updated_at
            "#,
            params![
                plan.id,
                plan.project_id,
                plan.session_id,
                plan.title,
                plan.description,
                plan.body_markdown,
                plan.status.to_string(),
                steps_json,
                comments_json,
                plan.created_at as i64,
                plan.updated_at as i64,
            ],
        )?;
        Ok(())
    }

    pub fn get(&self, plan_id: &str) -> Result<Option<Plan>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, project_id, session_id, title, description, body_markdown,
                   status, steps_json, comments_json, created_at, updated_at
            FROM plans WHERE id = ?1
            "#,
        )?;
        let plan = stmt.query_row(params![plan_id], row_to_plan).optional()?;
        Ok(plan)
    }

    pub fn list(
        &self,
        project_id: &str,
        filter_status: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Plan>> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.min(MAX_PLANS) as i64;
        let mut plans = Vec::new();
        if let Some(status) = filter_status {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, project_id, session_id, title, description, body_markdown,
                       status, steps_json, comments_json, created_at, updated_at
                FROM plans
                WHERE project_id = ?1 AND status = ?2
                ORDER BY updated_at DESC
                LIMIT ?3
                "#,
            )?;
            let rows = stmt.query_map(params![project_id, status, limit], row_to_plan)?;
            for row in rows {
                plans.push(row?);
            }
        } else {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, project_id, session_id, title, description, body_markdown,
                       status, steps_json, comments_json, created_at, updated_at
                FROM plans
                WHERE project_id = ?1
                ORDER BY updated_at DESC
                LIMIT ?2
                "#,
            )?;
            let rows = stmt.query_map(params![project_id, limit], row_to_plan)?;
            for row in rows {
                plans.push(row?);
            }
        }
        Ok(plans)
    }

    pub fn active(&self, project_id: &str) -> Result<Option<Plan>> {
        let mut plans = self.list(project_id, Some("active"), 1)?;
        Ok(plans.pop())
    }

    pub fn set_status(&self, plan_id: &str, status: PlanStatus) -> Result<()> {
        let mut plan = self
            .get(plan_id)?
            .ok_or_else(|| anyhow::anyhow!("plan '{plan_id}' not found"))?;
        plan.status = status;
        plan.updated_at = now_ms();
        self.upsert(&plan)
    }

    pub fn save_comments(&self, plan_id: &str, comments: Vec<PlanLineComment>) -> Result<()> {
        let mut plan = self
            .get(plan_id)?
            .ok_or_else(|| anyhow::anyhow!("plan '{plan_id}' not found"))?;
        plan.comments = comments;
        plan.updated_at = now_ms();
        self.upsert(&plan)
    }
}

fn row_to_plan(row: &rusqlite::Row<'_>) -> rusqlite::Result<Plan> {
    let steps_json: String = row.get(7)?;
    let comments_json: String = row.get(8)?;
    let steps: Vec<PlanStep> = serde_json::from_str(&steps_json).unwrap_or_default();
    let comments: Vec<PlanLineComment> = serde_json::from_str(&comments_json).unwrap_or_default();
    let status_str: String = row.get(6)?;
    let status = PlanStatus::parse(&status_str).unwrap_or(PlanStatus::Active);
    Ok(Plan {
        id: row.get(0)?,
        project_id: row.get(1)?,
        session_id: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        body_markdown: row.get(5)?,
        status,
        steps,
        comments,
        created_at: row.get::<_, i64>(9)? as u64,
        updated_at: row.get::<_, i64>(10)? as u64,
    })
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Build display lines for the review modal (stable line indices for comments).
pub fn plan_view_lines(plan: &Plan) -> Vec<String> {
    let mut lines = Vec::new();
    if !plan.title.is_empty() {
        lines.push(plan.title.clone());
        lines.push(String::new());
    }
    if !plan.description.trim().is_empty() {
        for para in plan.description.lines() {
            lines.push(para.to_string());
        }
        lines.push(String::new());
    }
    if !plan.body_markdown.trim().is_empty() {
        for line in plan.body_markdown.lines() {
            lines.push(line.to_string());
        }
        if !plan.steps.is_empty() {
            lines.push(String::new());
        }
    }
    for (i, step) in plan.steps.iter().enumerate() {
        let mark = if step.completed { "✓" } else { "•" };
        lines.push(format!("{mark} {}. {}", i + 1, step.description));
        if !step.notes.trim().is_empty() {
            lines.push(format!("    ↳ {}", step.notes.trim()));
        }
    }
    if lines.is_empty() {
        lines.push("(empty plan)".to_string());
    }
    lines
}

/// Format review feedback for the agent.
pub fn format_plan_feedback(
    plan: &Plan,
    comments: &[PlanLineComment],
    freeform: &str,
    decision: &str,
) -> String {
    let view = plan_view_lines(plan);
    let mut out = String::new();
    out.push_str(&format!("## Plan review feedback (plan_id={})\n", plan.id));
    out.push_str(&format!("### Decision: {decision}\n"));
    if !comments.is_empty() {
        out.push_str("### Line comments\n");
        for c in comments {
            let start = c.start_line.min(view.len().saturating_sub(1));
            let end = c.end_line.min(view.len().saturating_sub(1)).max(start);
            let snippet: Vec<&str> = view[start..=end]
                .iter()
                .map(|s| s.as_str())
                .filter(|s| !s.is_empty())
                .collect();
            let range = if start == end {
                format!("L{}", start + 1)
            } else {
                format!("L{}–{}", start + 1, end + 1)
            };
            out.push_str(&format!("- **{range}**"));
            if !snippet.is_empty() {
                out.push_str(&format!(" (`{}`)", snippet.join(" / ")));
            }
            out.push_str(&format!(": {}\n", c.text.trim()));
        }
    }
    let free = freeform.trim();
    if !free.is_empty() {
        out.push_str("### Freeform notes\n");
        out.push_str(free);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn upsert_get_list_roundtrip() {
        let dir = tempdir().unwrap();
        let store = PlanStore::open(&dir.path().join("plans.sqlite")).unwrap();
        let plan = Plan {
            id: "plan-1".into(),
            title: "Ship feature".into(),
            description: "Do the thing".into(),
            steps: vec![PlanStep {
                description: "Write code".into(),
                completed: false,
                notes: String::new(),
            }],
            status: PlanStatus::Active,
            created_at: 1,
            updated_at: 2,
            body_markdown: String::new(),
            comments: Vec::new(),
            project_id: "proj".into(),
            session_id: "sess".into(),
        };
        store.upsert(&plan).unwrap();
        let loaded = store.get("plan-1").unwrap().expect("plan");
        assert_eq!(loaded.title, "Ship feature");
        assert_eq!(loaded.steps.len(), 1);
        let list = store.list("proj", Some("active"), 10).unwrap();
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn view_lines_and_feedback() {
        let plan = Plan {
            id: "p".into(),
            title: "T".into(),
            description: "D".into(),
            steps: vec![PlanStep {
                description: "step one".into(),
                completed: false,
                notes: String::new(),
            }],
            status: PlanStatus::Proposed,
            created_at: 0,
            updated_at: 0,
            body_markdown: String::new(),
            comments: Vec::new(),
            project_id: String::new(),
            session_id: String::new(),
        };
        let lines = plan_view_lines(&plan);
        assert!(lines.iter().any(|l| l.contains("step one")));
        let fb = format_plan_feedback(
            &plan,
            &[PlanLineComment {
                start_line: 0,
                end_line: 0,
                text: "rename".into(),
            }],
            "more detail",
            "request_changes",
        );
        assert!(fb.contains("rename"));
        assert!(fb.contains("request_changes"));
    }
}
