use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use super::helpers;
use crate::config::NaviConfig;
use crate::skills::load_skill_by_id;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

pub(crate) struct SkillTool {
    project_dir: PathBuf,
    data_dir: PathBuf,
    config: Arc<RwLock<NaviConfig>>,
}

impl SkillTool {
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
impl Tool for SkillTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "load_skill",
            "Load full skill instructions by id, name, or `pool/id`. Prefer after `skill_list` (catalog or pool listing). Optional `pool` scopes the lookup when the skill lives in a folder.",
            ToolKind::Read,
            helpers::json_schema(
                &[
                    ("id", "Skill id from the catalog or a pool listing (or `pool/id`)."),
                    ("name", "Skill name."),
                    ("skill", "Skill id or name."),
                    ("pool", "Optional pool id when the skill is inside a folder."),
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
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("load_skill requires `id`, `skill`, or `name`"))?;

        let pool = invocation
            .input
            .get("pool")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let lookup = match pool {
            Some(p) if !requested.contains('/') => format!("{p}/{requested}"),
            _ => requested.to_string(),
        };

        let config = self
            .config
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let skill = load_skill_by_id(
            &config.skills,
            &self.project_dir,
            &self.data_dir,
            &lookup,
        )?;

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
                "instructions": skill.instructions,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SkillsConfig;
    use crate::skills::{SkillWriteRequest, SkillWriteScope, write_skill};

    #[tokio::test]
    async fn load_skill_returns_instructions_for_requested_skill() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        write_skill(
            &SkillWriteRequest {
                id: "reviewer".into(),
                name: "Reviewer".into(),
                description: Some("Reviews code".into()),
                version: None,
                author: None,
                tags: vec![],
                requires: vec![],
                allow_tools: vec![],
                deny_tools: vec![],
                harness: false,
                pool: None,
                instructions: "# Reviewer\nReview carefully.".into(),
                scope: SkillWriteScope::User,
            },
            tempdir.path(),
            tempdir.path(),
        )
        .expect("write skill");

        let mut config = NaviConfig::default();
        config.skills = SkillsConfig {
            enabled: true,
            active: Vec::new(),
        };
        let tool = SkillTool::new(
            tempdir.path().to_path_buf(),
            tempdir.path().to_path_buf(),
            Arc::new(RwLock::new(config)),
        );

        let result = tool
            .invoke(ToolInvocation {
                id: "call-1".to_string(),
                tool_name: "load_skill".to_string(),
                input: json!({ "id": "reviewer" }),
            })
            .await
            .expect("invoke");

        assert!(result.ok);
        assert_eq!(result.output["id"], "reviewer");
        assert_eq!(result.output["name"], "Reviewer");
        let body = result.output["instructions"].as_str().unwrap_or("");
        assert!(
            body.contains("Review carefully"),
            "unexpected instructions: {body:?}"
        );
    }

    #[tokio::test]
    async fn load_skill_opens_pool_member_with_pool_arg() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        write_skill(
            &SkillWriteRequest {
                id: "create-skill".into(),
                name: "Create Skill".into(),
                description: Some("Author".into()),
                version: None,
                author: None,
                tags: vec![],
                requires: vec![],
                allow_tools: vec!["skill_save".into()],
                deny_tools: vec![],
                harness: false,
                pool: Some("navi".into()),
                instructions: "# Author\nSave carefully.".into(),
                scope: SkillWriteScope::User,
            },
            tempdir.path(),
            tempdir.path(),
        )
        .expect("write");

        let mut config = NaviConfig::default();
        config.skills.enabled = true;
        let tool = SkillTool::new(
            tempdir.path().to_path_buf(),
            tempdir.path().to_path_buf(),
            Arc::new(RwLock::new(config)),
        );
        let result = tool
            .invoke(ToolInvocation {
                id: "c1".into(),
                tool_name: "load_skill".into(),
                input: json!({ "id": "create-skill", "pool": "navi" }),
            })
            .await
            .expect("invoke");
        assert!(result.ok);
        assert_eq!(result.output["pool"], "navi");
        assert!(
            result.output["instructions"]
                .as_str()
                .unwrap()
                .contains("Save carefully")
        );
    }
}
