//! Tools for listing / saving / deleting skills in the SQLite skill store.
//! Used by the built-in `navi-create-skill` skill and UIs via the engine API.

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use super::helpers;
use crate::config::NaviConfig;
use crate::skills::{
    SkillWriteRequest, SkillWriteScope, delete_skill, discover_configured_skills, write_skill,
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
            "List NAVI skills from the skill database and built-ins (id, name, description, allow_tools, source). Does not return full instructions.",
            ToolKind::Read,
            helpers::json_schema(&[], &[]),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let config = shared_config(&self.config);
        let skills = discover_configured_skills(&config.skills, &self.project_dir, &self.data_dir)?;
        let items: Vec<_> = skills
            .into_iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "name": s.name,
                    "description": s.description,
                    "allow_tools": s.allow_tools,
                    "deny_tools": s.deny_tools,
                    "tags": s.tags,
                    "source": format!("{:?}", s.source).to_lowercase(),
                    "scope": format!("{:?}", s.scope).to_lowercase(),
                })
            })
            .collect();
        Ok(helpers::ok(
            invocation.id,
            json!({ "skills": items, "count": items.len() }),
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
            "Load one skill including full instructions and tool policy by id or name.",
            ToolKind::Read,
            helpers::json_schema(
                &[
                    ("id", "Skill id."),
                    ("name", "Skill name."),
                    ("skill", "Skill id or name."),
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

        let config = shared_config(&self.config);
        let skills = discover_configured_skills(&config.skills, &self.project_dir, &self.data_dir)?;
        let skill = skills
            .into_iter()
            .find(|s| s.id == requested || s.name == requested)
            .ok_or_else(|| anyhow::anyhow!("skill `{requested}` not found"))?;

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
                "allow_tools": skill.allow_tools,
                "deny_tools": skill.deny_tools,
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
            "Create or update a skill in the NAVI skill database (SQLite under the data dir). Include allow_tools so the skill runs with a tight tool set.",
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
                        "scope",
                        "`user` (default, shared Desktop+TUI) or `project`.",
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
            instructions,
            scope: parse_scope(invocation.input.get("scope")),
        };

        let result = write_skill(&request, &self.project_dir, &self.data_dir)?;
        Ok(helpers::ok(
            invocation.id,
            json!({
                "created": result.created,
                "id": result.skill.id,
                "name": result.skill.name,
                "allow_tools": result.skill.allow_tools,
                "path": result.path.display().to_string(),
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
            "Delete a user skill from the NAVI skill database. Cannot delete built-in skills. Confirm with the user first.",
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
