use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use super::helpers;
use crate::config::NaviConfig;
use crate::skills::{SkillManifest, discover_configured_skills};
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

    fn discover(&self) -> Result<Vec<SkillManifest>> {
        let config = self
            .config
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        discover_configured_skills(&config.skills, &self.project_dir, &self.data_dir)
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "load_skill",
            "Load the instruction text for one available NAVI skill by id or name. Use this only after deciding the skill is relevant from the Available Skills catalog.",
            ToolKind::Read,
            helpers::json_schema(
                &[
                    ("id", "Skill id from the Available Skills catalog."),
                    ("name", "Skill name from the Available Skills catalog."),
                    (
                        "skill",
                        "Skill id or name from the Available Skills catalog.",
                    ),
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

        let skills = self.discover()?;
        let skill = skills
            .iter()
            .find(|skill| skill.id == requested || skill.name == requested)
            .ok_or_else(|| {
                let available: Vec<&str> = skills.iter().map(|skill| skill.id.as_str()).collect();
                anyhow::anyhow!(
                    "skill `{}` was not found. Available skill ids: {}",
                    requested,
                    available.join(", ")
                )
            })?;

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
                "instructions": skill.instructions,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SkillsConfig;

    #[tokio::test]
    async fn load_skill_returns_instructions_for_requested_skill() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let skill_dir = tempdir.path().join(".navi").join("skills").join("reviewer");
        std::fs::create_dir_all(&skill_dir).expect("create skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: Reviewer\ndescription: Reviews code\n---\n# Reviewer\nReview carefully.",
        )
        .expect("write skill");

        let mut config = NaviConfig::default();
        config.skills = SkillsConfig {
            enabled: true,
            dirs: Vec::new(),
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
        assert_eq!(
            result.output["instructions"],
            "# Reviewer\nReview carefully."
        );
    }
}
