//! Tools for listing / saving / deleting skills on the filesystem skill store.
//! Used by the built-in `navi-create-skill` skill and UIs via the engine API.
//!
//! Skill **pools** behave like folders:
//! - `skill_list` with no `pool` → root skills + pool folders (catalog)
//! - `skill_list` with `pool` → skills inside that pool (metadata only)
//! - `load_skill` / `skill_get` → full body by id or `pool/id`
//! - `skill_save` with `pool` → write into that pool folder

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use super::helpers;
use crate::config::{NaviConfig, SkillsConfig};
use crate::harness_pack::materialize_after_save;
use crate::skills::{
    SkillManifest, SkillPool, SkillStore, SkillWriteRequest, SkillWriteScope, delete_skill,
    discover_catalog_entries, load_skill_by_id, write_skill,
};
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

fn shared_config(config: &Arc<RwLock<NaviConfig>>) -> NaviConfig {
    config.read().unwrap_or_else(|e| e.into_inner()).clone()
}

fn parse_string_list(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect(),
        serde_json::Value::String(s) => s
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_scope(value: Option<&serde_json::Value>) -> SkillWriteScope {
    match value.and_then(|v| v.as_str()).unwrap_or("user") {
        "project" => SkillWriteScope::Project,
        _ => SkillWriteScope::User,
    }
}

fn parse_bool(value: Option<&serde_json::Value>) -> bool {
    match value {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::String(s)) => {
            matches!(
                s.trim().to_ascii_lowercase().as_str(),
                "true" | "yes" | "1" | "on"
            )
        }
        Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        _ => false,
    }
}

fn parse_optional_pool(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn skill_meta_json(s: &SkillManifest) -> serde_json::Value {
    json!({
        "id": s.id,
        "name": s.name,
        "description": s.description,
        "pool": s.pool,
        "allow_tools": s.allow_tools,
        "deny_tools": s.deny_tools,
        "tags": s.tags,
        "harness": s.harness,
        "source": format!("{:?}", s.source).to_lowercase(),
        "scope": format!("{:?}", s.scope).to_lowercase(),
    })
}

fn pool_meta_json(p: &SkillPool) -> serde_json::Value {
    json!({
        "kind": "pool",
        "id": p.id,
        "name": p.name,
        "description": p.description,
        "skill_count": p.skill_count,
        "scope": format!("{:?}", p.scope).to_lowercase(),
    })
}

// ── skill_list ────────────────────────────────────────────────────────────

pub(crate) struct SkillListTool {
    project_dir: PathBuf,
    data_dir: PathBuf,
    config: Arc<RwLock<NaviConfig>>,
}

impl SkillListTool {
    pub(crate) fn new(
        project_dir: PathBuf,
        data_dir: PathBuf,
        config: Arc<RwLock<NaviConfig>>,
    ) -> Self {
        Self {
            project_dir,
            data_dir,
            config,
        }
    }
}

#[async_trait]
impl Tool for SkillListTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "skill_list",
            "Browse the skill catalog like a filesystem. Without `pool`: list root skills and skill pools (folders) — metadata only, not pool members. With `pool` (e.g. `navi`): open that pool and list its skills (metadata only). Use `load_skill` / `skill_get` for full instructions.",
            ToolKind::Read,
            helpers::json_schema(
                &[(
                    "pool",
                    "Optional pool id (folder). When set, lists skills inside that pool only.",
                )],
                &[],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let config = shared_config(&self.config);
        if !config.skills.enabled {
            return Ok(helpers::ok(
                invocation.id,
                json!({ "skills": [], "pools": [], "count": 0 }),
            ));
        }

        let pool = parse_optional_pool(invocation.input.get("pool"));

        if let Some(pool_id) = pool {
            // Open pool folder: members + builtins that declare this pool.
            let store = SkillStore::open_with_project(&self.data_dir, &self.project_dir)?;
            let mut skills = store.list_pool_skills(&pool_id)?;
            for builtin in crate::skills::builtin_skills() {
                if builtin.pool.as_deref() == Some(pool_id.as_str())
                    && !skills.iter().any(|s| s.id == builtin.id)
                {
                    skills.push(builtin);
                }
            }
            skills.sort_by(|a, b| a.id.cmp(&b.id));
            let items: Vec<_> = skills.iter().map(skill_meta_json).collect();
            let count = items.len();
            return Ok(helpers::ok(
                invocation.id,
                json!({
                    "pool": pool_id,
                    "kind": "pool_listing",
                    "skills": items,
                    "count": count,
                }),
            ));
        }

        // Top-level catalog: root skills + pools (not nested members).
        let catalog = discover_catalog_entries(&config.skills, &self.project_dir, &self.data_dir)?;
        let skills: Vec<_> = catalog.root_skills.iter().map(skill_meta_json).collect();
        let pools: Vec<_> = catalog.pools.iter().map(pool_meta_json).collect();
        let count = skills.len() + pools.len();
        Ok(helpers::ok(
            invocation.id,
            json!({
                "kind": "catalog",
                "skills": skills,
                "pools": pools,
                "count": count,
            }),
        ))
    }
}

// ── skill_get ─────────────────────────────────────────────────────────────

pub(crate) struct SkillGetTool {
    project_dir: PathBuf,
    data_dir: PathBuf,
    config: Arc<RwLock<NaviConfig>>,
}

impl SkillGetTool {
    pub(crate) fn new(
        project_dir: PathBuf,
        data_dir: PathBuf,
        config: Arc<RwLock<NaviConfig>>,
    ) -> Self {
        Self {
            project_dir,
            data_dir,
            config,
        }
    }
}

#[async_trait]
impl Tool for SkillGetTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "skill_get",
            "Load one skill including full instructions and tool policy by id, name, or `pool/id`. Optional `pool` scopes the lookup.",
            ToolKind::Read,
            helpers::json_schema(
                &[
                    ("id", "Skill id (or `pool/id`)."),
                    ("name", "Skill name."),
                    ("skill", "Skill id or name."),
                    ("pool", "Optional pool id when the skill lives in a folder."),
                ],
                &[],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let requested = invocation
            .input
            .get("id")
            .or_else(|| invocation.input.get("skill"))
            .or_else(|| invocation.input.get("name"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("skill_get requires `id`, `skill`, or `name`"))?;

        let pool = parse_optional_pool(invocation.input.get("pool"));
        let config = shared_config(&self.config);
        let lookup = match pool {
            Some(ref p) if !requested.contains('/') => format!("{p}/{requested}"),
            _ => requested.to_string(),
        };
        let skill = load_skill_by_id(&config.skills, &self.project_dir, &self.data_dir, &lookup)?;

        Ok(helpers::ok(
            invocation.id,
            json!({
                "id": skill.id,
                "name": skill.name,
                "description": skill.description,
                "version": skill.version,
                "author": skill.author,
                "tags": skill.tags,
                "requires": skill.requires,
                "pool": skill.pool,
                "allow_tools": skill.allow_tools,
                "deny_tools": skill.deny_tools,
                "harness": skill.harness,
                "instructions": skill.instructions,
                "source": format!("{:?}", skill.source).to_lowercase(),
                "scope": format!("{:?}", skill.scope).to_lowercase(),
            }),
        ))
    }
}

// ── skill_save ────────────────────────────────────────────────────────────

pub(crate) struct SkillSaveTool {
    project_dir: PathBuf,
    data_dir: PathBuf,
}

impl SkillSaveTool {
    pub(crate) fn new(project_dir: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            project_dir,
            data_dir,
        }
    }
}

#[async_trait]
impl Tool for SkillSaveTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "skill_save",
            "Create or update a skill as markdown on disk. Root: `{data_dir}/skills/<id>/SKILL.md`. Inside a pool: `{data_dir}/skills/<pool>/<id>/SKILL.md` (same under `.navi/skills` for project). Set `pool` to place the skill in a folder (creates POOL.md if needed).",
            ToolKind::Write,
            helpers::json_schema(
                &[
                    ("name", "Human-readable skill name (required)."),
                    (
                        "instructions",
                        "Markdown instructions when the skill is active (required).",
                    ),
                    ("id", "Optional stable id; derived from name if omitted."),
                    ("description", "Short one-line summary."),
                    ("version", "Optional version string."),
                    ("author", "Optional author."),
                    ("tags", "Array or comma-separated tags."),
                    (
                        "requires",
                        "Array or comma-separated skill ids this depends on.",
                    ),
                    (
                        "allow_tools",
                        "Array of tool names available while this skill is active (recommended).",
                    ),
                    ("deny_tools", "Optional tool names to hide while active."),
                    (
                        "pool",
                        "Optional skill pool (folder) id, e.g. `navi`. Empty = root-level skill.",
                    ),
                    (
                        "scope",
                        "`user` (default, shared Desktop+TUI) or `project`.",
                    ),
                    (
                        "harness",
                        "When true, materialize a harness pack (loop.toml/graph.toml) after saving.",
                    ),
                ],
                &["name", "instructions"],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let name = invocation
            .input
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("skill_save requires `name`"))?
            .to_string();
        let instructions = invocation
            .input
            .get("instructions")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("skill_save requires non-empty `instructions`"))?
            .to_string();

        let request = SkillWriteRequest {
            id: invocation
                .input
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            name,
            description: invocation
                .input
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            version: invocation
                .input
                .get("version")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            author: invocation
                .input
                .get("author")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            tags: invocation
                .input
                .get("tags")
                .map(parse_string_list)
                .unwrap_or_default(),
            requires: invocation
                .input
                .get("requires")
                .map(parse_string_list)
                .unwrap_or_default(),
            allow_tools: invocation
                .input
                .get("allow_tools")
                .map(parse_string_list)
                .unwrap_or_default(),
            deny_tools: invocation
                .input
                .get("deny_tools")
                .map(parse_string_list)
                .unwrap_or_default(),
            harness: parse_bool(invocation.input.get("harness")),
            pool: parse_optional_pool(invocation.input.get("pool")),
            instructions,
            scope: parse_scope(invocation.input.get("scope")),
        };

        let result = write_skill(&request, &self.project_dir, &self.data_dir)?;

        // Best-effort harness materialize when the skill declares itself a harness
        // or declares required skills that form a workflow graph.
        let mut harness_pack: Option<std::path::PathBuf> = None;
        if result.skill.harness || !result.skill.requires.is_empty() {
            let mut required = Vec::new();
            if !result.skill.requires.is_empty() {
                let mut cfg = SkillsConfig::default();
                cfg.enabled = true;
                for req_id in &result.skill.requires {
                    if let Ok(req) =
                        load_skill_by_id(&cfg, &self.project_dir, &self.data_dir, req_id)
                    {
                        required.push(req);
                    }
                }
            }
            harness_pack = materialize_after_save(&self.data_dir, &result, &required)
                .ok()
                .flatten();
        }

        Ok(helpers::ok(
            invocation.id,
            json!({
                "created": result.created,
                "id": result.skill.id,
                "name": result.skill.name,
                "pool": result.skill.pool,
                "allow_tools": result.skill.allow_tools,
                "harness": result.skill.harness,
                "path": result.path.display().to_string(),
                "harness_pack": harness_pack.map(|p| p.display().to_string()),
                "source": "store",
            }),
        ))
    }
}

// ── skill_delete ──────────────────────────────────────────────────────────

pub(crate) struct SkillDeleteTool {
    project_dir: PathBuf,
    data_dir: PathBuf,
}

impl SkillDeleteTool {
    pub(crate) fn new(project_dir: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            project_dir,
            data_dir,
        }
    }
}

#[async_trait]
impl Tool for SkillDeleteTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "skill_delete",
            "Delete a user skill from the filesystem skill store. Cannot delete built-in skills. Confirm with the user first.",
            ToolKind::Write,
            helpers::json_schema(
                &[("id", "Skill id to delete."), ("skill", "Skill id alias.")],
                &[],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let id = invocation
            .input
            .get("id")
            .or_else(|| invocation.input.get("skill"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("skill_delete requires `id`"))?;

        let deleted = delete_skill(id, &self.project_dir, &self.data_dir)?;
        Ok(helpers::ok(
            invocation.id,
            json!({ "deleted": deleted, "id": id }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{SkillWriteRequest, SkillWriteScope, write_skill};

    fn tool_config(temp: &std::path::Path) -> (SkillListTool, SkillSaveTool, PathBuf) {
        let mut config = NaviConfig::default();
        config.skills.enabled = true;
        let project = temp.to_path_buf();
        let data = temp.to_path_buf();
        let list = SkillListTool::new(project.clone(), data.clone(), Arc::new(RwLock::new(config)));
        let save = SkillSaveTool::new(project, data.clone());
        (list, save, data)
    }

    #[tokio::test]
    async fn skill_list_catalog_shows_pools_not_nested_members() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        write_skill(
            &SkillWriteRequest {
                id: "create-skill".into(),
                name: "Create".into(),
                description: Some("d".into()),
                version: None,
                author: None,
                tags: vec![],
                requires: vec![],
                allow_tools: vec![],
                deny_tools: vec![],
                harness: false,
                pool: Some("navi".into()),
                instructions: "body".into(),
                scope: SkillWriteScope::User,
            },
            tempdir.path(),
            tempdir.path(),
        )
        .expect("write");

        let (list, _, _) = tool_config(tempdir.path());
        let result = list
            .invoke(ToolInvocation {
                id: "1".into(),
                tool_name: "skill_list".into(),
                input: json!({}),
            })
            .await
            .expect("list");
        assert!(result.ok);
        assert_eq!(result.output["kind"], "catalog");
        let pools = result.output["pools"].as_array().expect("pools");
        assert!(
            pools.iter().any(|p| p["id"] == "navi"),
            "expected navi pool: {pools:?}"
        );
        let skills = result.output["skills"].as_array().expect("skills");
        assert!(
            !skills.iter().any(|s| s["id"] == "create-skill"),
            "nested member must not appear at root: {skills:?}"
        );
    }

    #[tokio::test]
    async fn skill_list_with_pool_opens_folder() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        write_skill(
            &SkillWriteRequest {
                id: "create-skill".into(),
                name: "Create".into(),
                description: Some("d".into()),
                version: None,
                author: None,
                tags: vec![],
                requires: vec![],
                allow_tools: vec![],
                deny_tools: vec![],
                harness: false,
                pool: Some("navi".into()),
                instructions: "body".into(),
                scope: SkillWriteScope::User,
            },
            tempdir.path(),
            tempdir.path(),
        )
        .expect("write");

        let (list, _, _) = tool_config(tempdir.path());
        let result = list
            .invoke(ToolInvocation {
                id: "1".into(),
                tool_name: "skill_list".into(),
                input: json!({ "pool": "navi" }),
            })
            .await
            .expect("list pool");
        assert!(result.ok);
        assert_eq!(result.output["kind"], "pool_listing");
        assert_eq!(result.output["pool"], "navi");
        let skills = result.output["skills"].as_array().expect("skills");
        assert!(
            skills.iter().any(|s| s["id"] == "create-skill"),
            "pool open must list create-skill: {skills:?}"
        );
        // Builtin create skill may also appear under navi.
        assert!(
            skills
                .iter()
                .any(|s| s["id"] == crate::skills::CREATE_SKILL_ID)
                || skills.iter().any(|s| s["id"] == "create-skill")
        );
    }

    #[tokio::test]
    async fn skill_save_writes_into_pool() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let (_, save, _) = tool_config(tempdir.path());
        let result = save
            .invoke(ToolInvocation {
                id: "1".into(),
                tool_name: "skill_save".into(),
                input: json!({
                    "name": "Pool Member",
                    "instructions": "Do pool work.",
                    "pool": "navi",
                    "allow_tools": ["read_file"]
                }),
            })
            .await
            .expect("save");
        assert!(result.ok);
        assert_eq!(result.output["pool"], "navi");
        assert_eq!(result.output["id"], "pool-member");
        let path = result.output["path"].as_str().expect("path");
        assert!(path.contains("navi"), "path should include pool: {path}");
        assert!(std::path::Path::new(path).is_file());
    }

    #[tokio::test]
    async fn skill_list_pool_navi_includes_builtin_create_skill() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let (list, _, _) = tool_config(tempdir.path());
        let result = list
            .invoke(ToolInvocation {
                id: "1".into(),
                tool_name: "skill_list".into(),
                input: json!({ "pool": "navi" }),
            })
            .await
            .expect("list navi pool");
        assert!(result.ok);
        assert_eq!(result.output["kind"], "pool_listing");
        let skills = result.output["skills"].as_array().expect("skills");
        assert!(
            skills
                .iter()
                .any(|s| s["id"] == crate::skills::CREATE_SKILL_ID),
            "builtin create-skill must appear in pool navi: {skills:?}"
        );
        assert!(
            skills
                .iter()
                .any(|s| s["id"] == crate::skills::HARNESS_AUTHOR_ID),
            "builtin harness-author must appear in pool navi: {skills:?}"
        );
    }

    #[tokio::test]
    async fn skill_save_harness_materializes_pack_under_data_dir() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let data = tempdir.path().to_path_buf();
        let save = SkillSaveTool::new(tempdir.path().to_path_buf(), data.clone());
        let result = save
            .invoke(ToolInvocation {
                id: "1".into(),
                tool_name: "skill_save".into(),
                input: json!({
                    "name": "Design Loop",
                    "id": "design-loop",
                    "instructions": "Run design steps.",
                    "harness": true,
                    "allow_tools": ["read_file", "search"]
                }),
            })
            .await
            .expect("save harness");
        assert!(result.ok, "skill_save failed: {:?}", result.output);
        assert_eq!(result.output["harness"], true);
        let pack = result.output["harness_pack"]
            .as_str()
            .expect("harness_pack path");
        assert!(
            pack.contains("harnesses"),
            "pack should be under harnesses/: {pack}"
        );
        assert!(
            std::path::Path::new(pack).is_dir() || std::path::Path::new(pack).exists(),
            "pack path missing: {pack}"
        );
        // Soft apply only when skill is treated as active — pack on disk alone
        // must not lock when active list is empty.
        let idle = crate::harness_pack::apply_harness_for_skills(&data, &[]);
        assert!(
            idle.allow_tools.is_none(),
            "empty active list must not lock after materialize: {:?}",
            idle.allow_tools
        );
        let skill = crate::skills::load_skill_by_id(
            &SkillsConfig {
                enabled: true,
                active: vec![],
            },
            tempdir.path(),
            &data,
            "design-loop",
        )
        .expect("load saved harness skill");
        assert!(skill.harness, "saved skill must retain harness flag");
        // Session-active harness with pack → soft allowlist from entry and/or skill.
        let active =
            crate::harness_pack::apply_harness_for_skills(&data, std::slice::from_ref(&skill));
        assert!(
            !active.packs.is_empty(),
            "session-active harness skill must load materialized pack"
        );
        assert!(
            active.allow_tools.is_some(),
            "session-active harness must soft-lock tools via pack entry or harness allow_tools: {:?}",
            active.allow_tools
        );
    }
}
