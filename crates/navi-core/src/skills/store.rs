//! SQLite-backed skill store (`data_dir/skills.sqlite`).
//!
//! User-authored and agent-created skills live here so Desktop and TUI share
//! one source of truth without scattering SKILL.md under config dirs.

use super::{SkillManifest, SkillSource, SkillWriteRequest, SkillWriteResult, SkillWriteScope};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Local skill database under the NAVI data directory.
pub struct SkillStore {
    conn: Mutex<Connection>,
    path: PathBuf,
}

impl SkillStore {
    /// Opens (or creates) `<data_dir>/skills.sqlite`.
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("failed to create data dir {}", data_dir.display()))?;
        let path = data_dir.join("skills.sqlite");
        let conn = Connection::open(&path)
            .with_context(|| format!("failed to open skill store at {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("set skills.sqlite journal_mode=WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .context("enable skills.sqlite foreign_keys")?;
        let store = Self {
            conn: Mutex::new(conn),
            path,
        };
        store.init_schema()?;
        Ok(store)
    }

    /// In-memory store for tests.
    #[cfg(test)]
    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self {
            conn: Mutex::new(conn),
            path: PathBuf::from(":memory:"),
        };
        store.init_schema()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS skills (
                id            TEXT PRIMARY KEY,
                name          TEXT NOT NULL,
                description   TEXT,
                version       TEXT,
                author        TEXT,
                tags          TEXT NOT NULL DEFAULT '[]',
                requires      TEXT NOT NULL DEFAULT '[]',
                allow_tools   TEXT NOT NULL DEFAULT '[]',
                deny_tools    TEXT NOT NULL DEFAULT '[]',
                instructions  TEXT NOT NULL,
                scope         TEXT NOT NULL DEFAULT 'user',
                harness       INTEGER NOT NULL DEFAULT 0,
                project_key   TEXT,
                created_at    INTEGER NOT NULL,
                updated_at    INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_skills_scope ON skills(scope);
            CREATE INDEX IF NOT EXISTS idx_skills_project ON skills(project_key);
            "#,
        )
        .context("init skills.sqlite schema")?;
        Self::migrate_add_harness(&conn)?;
        Ok(())
    }

    fn migrate_add_harness(conn: &Connection) -> Result<()> {
        let has_col: bool = conn
            .query_row(
                "SELECT 1 FROM pragma_table_info('skills') WHERE name = 'harness'",
                [],
                |_| Ok(true),
            )
            .optional()?
            .is_some();
        if !has_col {
            conn.execute(
                "ALTER TABLE skills ADD COLUMN harness INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .context("add harness column to skills")?;
        }
        Ok(())
    }

    /// Lists all stored skills (user + project rows).
    pub fn list_all(&self) -> Result<Vec<SkillManifest>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT id, name, description, version, author, tags, requires, allow_tools, deny_tools,
                    harness, instructions, scope, project_key
             FROM skills ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |row| row_to_manifest(row, &self.path))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Lists skills for discovery: all `user` scope + project-scoped for `project_key`.
    pub fn list_for_discovery(&self, project_key: Option<&str>) -> Result<Vec<SkillManifest>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT id, name, description, version, author, tags, requires, allow_tools, deny_tools,
                        harness, instructions, scope, project_key
                 FROM skills WHERE scope = 'user' ORDER BY name COLLATE NOCASE",
            )?;
            let rows = stmt.query_map([], |row| row_to_manifest(row, &self.path))?;
            for row in rows {
                out.push(row?);
            }
        }
        if let Some(pk) = project_key.filter(|s| !s.is_empty()) {
            let mut stmt = conn.prepare(
                "SELECT id, name, description, version, author, tags, requires, allow_tools, deny_tools,
                        harness, instructions, scope, project_key
                 FROM skills WHERE scope = 'project' AND project_key = ?1
                 ORDER BY name COLLATE NOCASE",
            )?;
            let rows = stmt.query_map(params![pk], |row| row_to_manifest(row, &self.path))?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    pub fn get(&self, id: &str) -> Result<Option<SkillManifest>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let result = conn
            .query_row(
                "SELECT id, name, description, version, author, tags, requires, allow_tools, deny_tools,
                        harness, instructions, scope, project_key
                 FROM skills WHERE id = ?1",
                params![id],
                |row| row_to_manifest(row, &self.path),
            )
            .optional()
            .context("failed to load skill")?;
        Ok(result)
    }

    pub fn upsert(
        &self,
        request: &SkillWriteRequest,
        project_key: Option<&str>,
    ) -> Result<SkillWriteResult> {
        let id = super::resolve_skill_id(request)?;
        let name = request.name.trim();
        if name.is_empty() {
            return Err(anyhow::anyhow!("skill name is required"));
        }
        let instructions = request.instructions.trim();
        if instructions.is_empty() {
            return Err(anyhow::anyhow!("skill instructions cannot be empty"));
        }

        let scope = match request.scope {
            SkillWriteScope::User => "user",
            SkillWriteScope::Project => "project",
        };
        if scope == "project" && project_key.unwrap_or("").is_empty() {
            return Err(anyhow::anyhow!(
                "project-scoped skill requires an active project"
            ));
        }

        let now = now_secs();
        let tags = serde_json::to_string(&clean_list(&request.tags))?;
        let requires = serde_json::to_string(&clean_list(&request.requires))?;
        let allow_tools = serde_json::to_string(&clean_list(&request.allow_tools))?;
        let deny_tools = serde_json::to_string(&clean_list(&request.deny_tools))?;
        let description = request
            .description
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let version = request
            .version
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let author = request
            .author
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let created = conn
            .query_row("SELECT 1 FROM skills WHERE id = ?1", params![id], |_| {
                Ok(1i32)
            })
            .optional()?
            .is_none();

        conn.execute(
            "INSERT INTO skills (
                id, name, description, version, author, tags, requires, allow_tools, deny_tools,
                instructions, scope, harness, project_key, created_at, updated_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?14)
             ON CONFLICT(id) DO UPDATE SET
                name=excluded.name,
                description=excluded.description,
                version=excluded.version,
                author=excluded.author,
                tags=excluded.tags,
                requires=excluded.requires,
                allow_tools=excluded.allow_tools,
                deny_tools=excluded.deny_tools,
                instructions=excluded.instructions,
                scope=excluded.scope,
                harness=excluded.harness,
                project_key=excluded.project_key,
                updated_at=excluded.updated_at",
            params![
                id,
                name,
                description,
                version,
                author,
                tags,
                requires,
                allow_tools,
                deny_tools,
                instructions,
                scope,
                request.harness as i32,
                project_key.filter(|s| !s.is_empty()),
                now as i64,
            ],
        )?;
        drop(conn);

        let skill = self
            .get(&id)?
            .ok_or_else(|| anyhow::anyhow!("skill `{id}` missing after upsert"))?;
        Ok(SkillWriteResult {
            skill: skill.clone(),
            path: self.path.clone(),
            created,
        })
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let n = conn.execute("DELETE FROM skills WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn clean_list(items: &[String]) -> Vec<String> {
    items
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn parse_json_list(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

fn row_to_manifest(row: &rusqlite::Row<'_>, store_path: &Path) -> rusqlite::Result<SkillManifest> {
    let id: String = row.get(0)?;
    let name: String = row.get(1)?;
    let description: Option<String> = row.get(2)?;
    let version: Option<String> = row.get(3)?;
    let author: Option<String> = row.get(4)?;
    let tags_raw: String = row.get(5)?;
    let requires_raw: String = row.get(6)?;
    let allow_raw: String = row.get(7)?;
    let deny_raw: String = row.get(8)?;
    let harness: i32 = row.get(9)?;
    let instructions: String = row.get(10)?;
    let scope: String = row.get(11)?;
    let _project_key: Option<String> = row.get(12)?;

    Ok(SkillManifest {
        id,
        name,
        description,
        version,
        author,
        tags: parse_json_list(&tags_raw),
        requires: parse_json_list(&requires_raw),
        allow_tools: parse_json_list(&allow_raw),
        deny_tools: parse_json_list(&deny_raw),
        harness: harness != 0,
        path: store_path.to_path_buf(),
        instructions,
        source: SkillSource::Store,
        scope: if scope == "project" {
            SkillWriteScope::Project
        } else {
            SkillWriteScope::User
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_and_list_roundtrip() {
        let store = SkillStore::open_memory().expect("open");
        let result = store
            .upsert(
                &SkillWriteRequest {
                    id: "demo".into(),
                    name: "Demo".into(),
                    description: Some("desc".into()),
                    version: None,
                    author: None,
                    tags: vec!["t".into()],
                    requires: vec![],
                    allow_tools: vec!["read_file".into(), "skill_save".into()],
                    deny_tools: vec![],
                    harness: false,
                    instructions: "Do the demo.".into(),
                    scope: SkillWriteScope::User,
                },
                None,
            )
            .expect("upsert");
        assert!(result.created);
        assert_eq!(result.skill.allow_tools, vec!["read_file", "skill_save"]);
        let listed = store.list_for_discovery(None).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "demo");
    }
}
