//! Filesystem-backed skill store with optional **pools** (folders).
//!
//! Layout:
//! ```text
//! {data_dir}/skills/
//!   <skill-id>/SKILL.md              # root-level skill
//!   <pool-id>/
//!     POOL.md                        # pool metadata
//!     <skill-id>/SKILL.md            # skill inside pool
//! {project}/.navi/skills/            # same shape for project scope
//! ```

use super::{
    ParsedSkillFile, SkillManifest, SkillSource, SkillWriteRequest, SkillWriteResult,
    SkillWriteScope, parse_skill_md, resolve_skill_id, slugify_skill_id,
};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Local skill store rooted at `data_dir` (user) and optionally `project_dir` (project).
pub struct SkillStore {
    data_dir: PathBuf,
    project_dir: Option<PathBuf>,
}

/// A skill pool = folder of related skills (shown as one catalog entry until opened).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPool {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub scope: SkillWriteScope,
    pub path: PathBuf,
    pub skill_count: usize,
}

impl SkillStore {
    /// Opens the filesystem skill store under `<data_dir>/skills/`.
    pub fn open(data_dir: &Path) -> Result<Self> {
        let root = data_dir.join("skills");
        fs::create_dir_all(&root)
            .with_context(|| format!("failed to create skills dir {}", root.display()))?;
        let store = Self {
            data_dir: data_dir.to_path_buf(),
            project_dir: None,
        };
        if let Err(err) = store.migrate_from_sqlite_if_present() {
            tracing::warn!(error = %err, "legacy skills.sqlite migration skipped");
        }
        Ok(store)
    }

    pub fn open_with_project(data_dir: &Path, project_dir: &Path) -> Result<Self> {
        let mut store = Self::open(data_dir)?;
        store.project_dir = Some(project_dir.to_path_buf());
        Ok(store)
    }

    #[cfg(test)]
    pub fn open_memory() -> Result<Self> {
        let dir = tempfile::tempdir().context("tempdir for skill store")?;
        let path = dir.keep();
        Self::open(&path)
    }

    pub fn path(&self) -> PathBuf {
        self.data_dir.join("skills")
    }

    fn user_root(&self) -> PathBuf {
        self.data_dir.join("skills")
    }

    fn project_root(&self) -> Option<PathBuf> {
        self.project_dir
            .as_ref()
            .map(|p| p.join(".navi").join("skills"))
    }

    fn skill_file_path(
        &self,
        id: &str,
        pool: Option<&str>,
        scope: SkillWriteScope,
    ) -> Result<PathBuf> {
        let root = match scope {
            SkillWriteScope::User => self.user_root(),
            SkillWriteScope::Project => self.project_root().ok_or_else(|| {
                anyhow::anyhow!("project-scoped skill requires an active project")
            })?,
        };
        Ok(match pool.filter(|p| !p.is_empty()) {
            Some(pool) => root.join(pool).join(id).join("SKILL.md"),
            None => root.join(id).join("SKILL.md"),
        })
    }

    /// Lists **all** skills (root + every pool member). Used by load_skill / tests.
    pub fn list_all(&self) -> Result<Vec<SkillManifest>> {
        let mut out = list_skills_recursive(&self.user_root(), SkillWriteScope::User, None)?;
        if let Some(root) = self.project_root() {
            out.extend(list_skills_recursive(
                &root,
                SkillWriteScope::Project,
                None,
            )?);
        }
        out.sort_by(|a, b| a.pool.cmp(&b.pool).then_with(|| a.id.cmp(&b.id)));
        out.dedup_by(|a, b| a.id == b.id && a.pool == b.pool);
        Ok(out)
    }

    /// Root-level skills only (not inside a pool). For prompt catalog.
    pub fn list_root_skills(&self) -> Result<Vec<SkillManifest>> {
        let mut out = list_root_skills_in(&self.user_root(), SkillWriteScope::User)?;
        if let Some(root) = self.project_root() {
            out.extend(list_root_skills_in(&root, SkillWriteScope::Project)?);
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out.dedup_by(|a, b| a.id == b.id);
        Ok(out)
    }

    /// All pools (folders) under user + project roots.
    pub fn list_pools(&self) -> Result<Vec<SkillPool>> {
        let mut out = list_pools_in(&self.user_root(), SkillWriteScope::User)?;
        if let Some(root) = self.project_root() {
            out.extend(list_pools_in(&root, SkillWriteScope::Project)?);
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out.dedup_by(|a, b| a.id == b.id);
        Ok(out)
    }

    /// Skills inside one pool (metadata + instructions for load).
    pub fn list_pool_skills(&self, pool_id: &str) -> Result<Vec<SkillManifest>> {
        let pool = slugify_skill_id(pool_id);
        let mut out = Vec::new();
        let user_pool = self.user_root().join(&pool);
        if user_pool.is_dir() {
            out.extend(list_skills_in_pool_dir(
                &user_pool,
                &pool,
                SkillWriteScope::User,
            )?);
        }
        if let Some(root) = self.project_root() {
            let p = root.join(&pool);
            if p.is_dir() {
                out.extend(list_skills_in_pool_dir(
                    &p,
                    &pool,
                    SkillWriteScope::Project,
                )?);
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out.dedup_by(|a, b| a.id == b.id);
        Ok(out)
    }

    pub fn list_for_discovery(&self, _project_key: Option<&str>) -> Result<Vec<SkillManifest>> {
        self.list_all()
    }

    pub fn get(&self, id: &str) -> Result<Option<SkillManifest>> {
        self.get_in_pool(id, None)
    }

    /// Resolve skill by id, optionally restricted to a pool. Also tries `pool/id` form.
    pub fn get_in_pool(&self, id: &str, pool: Option<&str>) -> Result<Option<SkillManifest>> {
        let (pool_hint, skill_id) = split_pool_skill_ref(id, pool);
        let skill_id = slugify_skill_id(&skill_id);
        let pool_hint = pool_hint.map(|p| slugify_skill_id(&p));

        // Prefer project over user.
        for scope in [SkillWriteScope::Project, SkillWriteScope::User] {
            if matches!(scope, SkillWriteScope::Project) && self.project_dir.is_none() {
                continue;
            }
            if let Some(ref p) = pool_hint {
                let path = self.skill_file_path(&skill_id, Some(p), scope)?;
                if path.is_file() {
                    return Ok(Some(load_skill_md(&path, scope, Some(p))?));
                }
            } else {
                // Root first, then search all pools.
                let root_path = self.skill_file_path(&skill_id, None, scope)?;
                if root_path.is_file() {
                    return Ok(Some(load_skill_md(&root_path, scope, None)?));
                }
                let root = match scope {
                    SkillWriteScope::User => self.user_root(),
                    SkillWriteScope::Project => self.project_root().unwrap(),
                };
                if let Some(found) = find_skill_in_pools(&root, &skill_id, scope)? {
                    return Ok(Some(found));
                }
            }
        }
        Ok(None)
    }

    pub fn upsert(
        &self,
        request: &SkillWriteRequest,
        _project_key: Option<&str>,
    ) -> Result<SkillWriteResult> {
        let id = resolve_skill_id(request)?;
        let name = request.name.trim();
        if name.is_empty() {
            return Err(anyhow::anyhow!("skill name is required"));
        }
        let instructions = request.instructions.trim();
        if instructions.is_empty() {
            return Err(anyhow::anyhow!("skill instructions cannot be empty"));
        }
        if matches!(request.scope, SkillWriteScope::Project) && self.project_dir.is_none() {
            return Err(anyhow::anyhow!(
                "project-scoped skill requires an active project"
            ));
        }

        let pool = request
            .pool
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(slugify_skill_id);

        if let Some(ref p) = pool {
            ensure_pool_meta(
                match request.scope {
                    SkillWriteScope::User => self.user_root().join(p),
                    SkillWriteScope::Project => self
                        .project_root()
                        .ok_or_else(|| {
                            anyhow::anyhow!("project-scoped skill requires an active project")
                        })?
                        .join(p),
                },
                p,
            )?;
        }

        let path = self.skill_file_path(&id, pool.as_deref(), request.scope)?;
        let created = !path.is_file();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create skill dir {}", parent.display()))?;
        }

        let body = render_skill_md(request, &id);
        fs::write(&path, body)
            .with_context(|| format!("failed to write skill file {}", path.display()))?;

        let skill = load_skill_md(&path, request.scope, pool.as_deref())?;
        Ok(SkillWriteResult {
            skill,
            path,
            created,
        })
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        let (pool_hint, skill_id) = split_pool_skill_ref(id, None);
        let skill_id = slugify_skill_id(&skill_id);
        let mut deleted = false;

        if let Some(skill) = self.get_in_pool(&skill_id, pool_hint.as_deref())?
            && let Some(parent) = skill.path.parent()
            && parent.is_dir()
        {
            fs::remove_dir_all(parent)
                .with_context(|| format!("failed to delete skill dir {}", parent.display()))?;
            deleted = true;
        }
        Ok(deleted)
    }

    fn migrate_from_sqlite_if_present(&self) -> Result<()> {
        let sqlite_path = self.data_dir.join("skills.sqlite");
        if !sqlite_path.is_file() {
            return Ok(());
        }
        let conn = rusqlite::Connection::open(&sqlite_path)
            .with_context(|| format!("open legacy {}", sqlite_path.display()))?;
        let mut stmt = match conn.prepare(
            "SELECT id, name, description, version, author, tags, requires, allow_tools, deny_tools,
                    instructions, scope,
                    COALESCE(harness, 0)
             FROM skills",
        ) {
            Ok(s) => s,
            Err(_) => conn.prepare(
                "SELECT id, name, description, version, author, tags, requires, allow_tools, deny_tools,
                        instructions, scope, 0
                 FROM skills",
            )?,
        };
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5).unwrap_or_else(|_| "[]".into()),
                row.get::<_, String>(6).unwrap_or_else(|_| "[]".into()),
                row.get::<_, String>(7).unwrap_or_else(|_| "[]".into()),
                row.get::<_, String>(8).unwrap_or_else(|_| "[]".into()),
                row.get::<_, String>(9)?,
                row.get::<_, String>(10).unwrap_or_else(|_| "user".into()),
                row.get::<_, i32>(11).unwrap_or(0),
            ))
        })?;

        for row in rows {
            let (
                id,
                name,
                description,
                version,
                author,
                tags_raw,
                requires_raw,
                allow_raw,
                deny_raw,
                instructions,
                _scope_raw,
                harness,
            ) = row?;
            let path = self.skill_file_path(&id, None, SkillWriteScope::User)?;
            if path.is_file() {
                continue;
            }
            let request = SkillWriteRequest {
                id: id.clone(),
                name,
                description,
                version,
                author,
                tags: serde_json::from_str(&tags_raw).unwrap_or_default(),
                requires: serde_json::from_str(&requires_raw).unwrap_or_default(),
                allow_tools: serde_json::from_str(&allow_raw).unwrap_or_default(),
                deny_tools: serde_json::from_str(&deny_raw).unwrap_or_default(),
                harness: harness != 0,
                pool: None,
                instructions,
                scope: SkillWriteScope::User,
            };
            let body = render_skill_md(&request, &id);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, body)?;
            tracing::info!(id = %id, path = %path.display(), "migrated skill from skills.sqlite");
        }
        Ok(())
    }
}

fn split_pool_skill_ref(id: &str, pool: Option<&str>) -> (Option<String>, String) {
    if let Some(p) = pool.filter(|s| !s.trim().is_empty()) {
        return (Some(p.to_string()), id.to_string());
    }
    if let Some((p, s)) = id.split_once('/')
        && !p.is_empty()
        && !s.is_empty()
    {
        return (Some(p.to_string()), s.to_string());
    }
    (None, id.to_string())
}

fn ensure_pool_meta(pool_dir: PathBuf, pool_id: &str) -> Result<()> {
    fs::create_dir_all(&pool_dir)
        .with_context(|| format!("failed to create pool dir {}", pool_dir.display()))?;
    let meta = pool_dir.join("POOL.md");
    if !meta.is_file() {
        let body = format!(
            "---\nname: {}\nid: {}\ndescription: Skill pool\n---\n\n# {}\n\nSkill pool folder.\n",
            pool_id, pool_id, pool_id
        );
        fs::write(&meta, body)?;
    }
    Ok(())
}

fn list_root_skills_in(root: &Path, scope: SkillWriteScope) -> Result<Vec<SkillManifest>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(root)
        .with_context(|| format!("failed to read skills dir {}", root.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            if path.extension().and_then(|e| e.to_str()) == Some("md")
                && !path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .eq_ignore_ascii_case("POOL.md")
                && let Ok(skill) = load_skill_md(&path, scope, None)
            {
                out.push(skill);
            }
            continue;
        }
        // Pool directory: has POOL.md or child skill dirs without SKILL.md at this level
        if path.join("POOL.md").is_file() {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        if skill_md.is_file()
            && let Ok(skill) = load_skill_md(&skill_md, scope, None)
        {
            out.push(skill);
        }
    }
    Ok(out)
}

fn list_pools_in(root: &Path, scope: SkillWriteScope) -> Result<Vec<SkillPool>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(root)
        .with_context(|| format!("failed to read skills dir {}", root.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Pool if: POOL.md exists OR (no SKILL.md and has subdirs with SKILL.md)
        let is_pool = path.join("POOL.md").is_file()
            || (!path.join("SKILL.md").is_file() && dir_has_nested_skills(&path));
        if !is_pool {
            continue;
        }
        let id = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("pool")
            .to_string();
        let (name, description) = read_pool_meta(&path, &id);
        let skills = list_skills_in_pool_dir(&path, &id, scope)?;
        out.push(SkillPool {
            id: slugify_skill_id(&id),
            name,
            description,
            scope,
            path: path.clone(),
            skill_count: skills.len(),
        });
    }
    Ok(out)
}

fn dir_has_nested_skills(path: &Path) -> bool {
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() && p.join("SKILL.md").is_file() {
            return true;
        }
    }
    false
}

fn read_pool_meta(path: &Path, fallback_id: &str) -> (String, Option<String>) {
    let meta = path.join("POOL.md");
    if meta.is_file()
        && let Ok(raw) = fs::read_to_string(&meta)
    {
        let parsed = parse_skill_md(&raw, fallback_id);
        let name = if parsed.name.trim().is_empty() {
            fallback_id.to_string()
        } else {
            parsed.name
        };
        return (name, parsed.description);
    }
    (
        fallback_id.to_string(),
        Some(format!("Skill pool `{fallback_id}`")),
    )
}

fn list_skills_in_pool_dir(
    pool_dir: &Path,
    pool_id: &str,
    scope: SkillWriteScope,
) -> Result<Vec<SkillManifest>> {
    let mut out = Vec::new();
    if !pool_dir.is_dir() {
        return Ok(out);
    }
    for entry in fs::read_dir(pool_dir)
        .with_context(|| format!("failed to read pool dir {}", pool_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.eq_ignore_ascii_case("POOL.md"))
            .unwrap_or(false)
        {
            continue;
        }
        if path.is_dir() {
            let skill_md = path.join("SKILL.md");
            if skill_md.is_file()
                && let Ok(skill) = load_skill_md(&skill_md, scope, Some(pool_id))
            {
                out.push(skill);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("md")
            && let Ok(skill) = load_skill_md(&path, scope, Some(pool_id))
        {
            out.push(skill);
        }
    }
    Ok(out)
}

fn list_skills_recursive(
    root: &Path,
    scope: SkillWriteScope,
    pool: Option<&str>,
) -> Result<Vec<SkillManifest>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    // Root skills
    out.extend(list_root_skills_in(root, scope)?);
    // Pool members
    for pool_meta in list_pools_in(root, scope)? {
        out.extend(list_skills_in_pool_dir(
            &pool_meta.path,
            &pool_meta.id,
            scope,
        )?);
    }
    // If we're already inside a pool path (pool arg set), list that dir only
    if let Some(p) = pool {
        out.extend(list_skills_in_pool_dir(root, p, scope)?);
    }
    Ok(out)
}

fn find_skill_in_pools(
    root: &Path,
    skill_id: &str,
    scope: SkillWriteScope,
) -> Result<Option<SkillManifest>> {
    for pool in list_pools_in(root, scope)? {
        let path = pool.path.join(skill_id).join("SKILL.md");
        if path.is_file() {
            return Ok(Some(load_skill_md(&path, scope, Some(&pool.id))?));
        }
    }
    Ok(None)
}

fn load_skill_md(path: &Path, scope: SkillWriteScope, pool: Option<&str>) -> Result<SkillManifest> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read skill file {}", path.display()))?;
    let fallback = path.file_stem().and_then(|s| s.to_str()).unwrap_or("skill");
    let fallback = if fallback.eq_ignore_ascii_case("skill") {
        path.parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or(fallback)
    } else {
        fallback
    };
    let parsed = parse_skill_md(&raw, fallback);
    Ok(parsed_to_manifest(parsed, path, scope, pool))
}

fn parsed_to_manifest(
    parsed: ParsedSkillFile,
    path: &Path,
    scope: SkillWriteScope,
    pool: Option<&str>,
) -> SkillManifest {
    let id = parsed
        .id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .filter(|s| !s.eq_ignore_ascii_case("skills"))
                .unwrap_or("skill")
                .to_string()
        });
    let pool = pool
        .map(|p| p.to_string())
        .or(parsed.pool)
        .filter(|p| !p.is_empty());
    SkillManifest {
        id: slugify_skill_id(&id),
        name: if parsed.name.trim().is_empty() {
            id
        } else {
            parsed.name
        },
        description: parsed.description,
        version: parsed.version,
        author: parsed.author,
        tags: parsed.tags,
        requires: parsed.requires,
        allow_tools: parsed.allow_tools,
        deny_tools: parsed.deny_tools,
        harness: parsed.harness,
        pool,
        path: path.to_path_buf(),
        instructions: parsed.instructions,
        source: SkillSource::Store,
        scope,
    }
}

/// Render a skill request to SKILL.md with YAML frontmatter.
pub fn render_skill_md(request: &SkillWriteRequest, id: &str) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("name: {}\n", yaml_quote(&request.name)));
    out.push_str(&format!("id: {}\n", yaml_quote(id)));
    if let Some(desc) = request
        .description
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        out.push_str(&format!("description: {}\n", yaml_quote(desc)));
    }
    if let Some(version) = request
        .version
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        out.push_str(&format!("version: {}\n", yaml_quote(version)));
    }
    if let Some(author) = request
        .author
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        out.push_str(&format!("author: {}\n", yaml_quote(author)));
    }
    if let Some(pool) = request
        .pool
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        out.push_str(&format!("pool: {}\n", yaml_quote(pool)));
    }
    if !request.tags.is_empty() {
        out.push_str(&format!("tags: {}\n", yaml_list(&request.tags)));
    }
    if !request.requires.is_empty() {
        out.push_str(&format!("requires: {}\n", yaml_list(&request.requires)));
    }
    if !request.allow_tools.is_empty() {
        out.push_str(&format!(
            "allow_tools: {}\n",
            yaml_list(&request.allow_tools)
        ));
    }
    if !request.deny_tools.is_empty() {
        out.push_str(&format!("deny_tools: {}\n", yaml_list(&request.deny_tools)));
    }
    if request.harness {
        out.push_str("harness: true\n");
    }
    out.push_str("---\n\n");
    out.push_str(request.instructions.trim());
    out.push('\n');
    out
}

fn yaml_quote(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".into();
    }
    if value.contains(':')
        || value.contains('#')
        || value.contains('"')
        || value.contains('\'')
        || value.starts_with(' ')
        || value.ends_with(' ')
        || value.contains('\n')
    {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

fn yaml_list(items: &[String]) -> String {
    let cleaned: Vec<String> = items
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    format!(
        "[{}]",
        cleaned
            .iter()
            .map(|s| yaml_quote(s))
            .collect::<Vec<_>>()
            .join(", ")
    )
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
                    pool: None,
                    instructions: "Do the demo.".into(),
                    scope: SkillWriteScope::User,
                },
                None,
            )
            .expect("upsert");
        assert!(result.created);
        assert_eq!(result.skill.allow_tools, vec!["read_file", "skill_save"]);
        assert!(result.path.ends_with("SKILL.md"));
        let listed = store.list_for_discovery(None).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "demo");
    }

    #[test]
    fn pool_hides_members_from_root_list() {
        let store = SkillStore::open_memory().expect("open");
        store
            .upsert(
                &SkillWriteRequest {
                    id: "create-skill".into(),
                    name: "Create Skill".into(),
                    description: Some("Author skills".into()),
                    version: None,
                    author: None,
                    tags: vec!["navi".into()],
                    requires: vec![],
                    allow_tools: vec!["skill_save".into()],
                    deny_tools: vec![],
                    harness: false,
                    pool: Some("navi".into()),
                    instructions: "Create carefully.".into(),
                    scope: SkillWriteScope::User,
                },
                None,
            )
            .expect("upsert");
        let root = store.list_root_skills().expect("root");
        assert!(root.is_empty(), "pool members must not be root: {root:?}");
        let pools = store.list_pools().expect("pools");
        assert_eq!(pools.len(), 1);
        assert_eq!(pools[0].id, "navi");
        assert_eq!(pools[0].skill_count, 1);
        let members = store.list_pool_skills("navi").expect("members");
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].id, "create-skill");
        assert_eq!(members[0].pool.as_deref(), Some("navi"));
        let loaded = store
            .get_in_pool("create-skill", Some("navi"))
            .expect("get")
            .expect("present");
        assert!(loaded.instructions.contains("Create carefully"));
    }
}
