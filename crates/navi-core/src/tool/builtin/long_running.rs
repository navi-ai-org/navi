use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use super::helpers;
use crate::security::SecurityPolicy;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

const FEATURE_LIST: &str = "feature_list.json";
const PROGRESS_FILE: &str = "navi-progress.txt";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FeatureList {
    schema_version: u32,
    goal: String,
    features: Vec<FeatureEntry>,
    created_at: u64,
    updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FeatureEntry {
    id: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    verification_steps: Vec<String>,
    passes: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VerificationResult {
    command: String,
    ok: bool,
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

pub(crate) struct InitSessionTool {
    policy: SecurityPolicy,
}

impl InitSessionTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }
}

#[async_trait]
impl Tool for InitSessionTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "init_session",
            "Initialize a long-running NAVI sprint. Creates `.navi/feature_list.json` and `.navi/navi-progress.txt` with machine-readable feature status. Use before long-running implementation work.",
            ToolKind::Write,
            json!({
                "type": "object",
                "properties": {
                    "goal": {
                        "type": "string",
                        "description": "Overall sprint goal."
                    },
                    "features": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 50,
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string", "description": "Optional stable feature id. Defaults to feature-N." },
                                "title": { "type": "string" },
                                "description": { "type": "string" },
                                "verification_steps": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Shell commands that must pass before this feature may be marked done."
                                }
                            },
                            "required": ["title", "verification_steps"],
                            "additionalProperties": false
                        }
                    },
                    "overwrite": {
                        "type": "boolean",
                        "description": "When true, replace existing long-running sprint artifacts."
                    }
                },
                "required": ["goal", "features"],
                "additionalProperties": false
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let goal = helpers::required_string(&invocation.input, "goal")?
            .trim()
            .to_string();
        let overwrite = helpers::optional_bool(&invocation.input, "overwrite").unwrap_or(false);
        let feature_values = invocation
            .input
            .get("features")
            .and_then(Value::as_array)
            .context("missing required array `features`")?;
        if feature_values.is_empty() {
            bail!("features must not be empty");
        }

        let navi_dir = navi_dir(self.policy.project_root());
        let feature_path = navi_dir.join(FEATURE_LIST);
        let progress_path = navi_dir.join(PROGRESS_FILE);
        if feature_path.exists() && !overwrite {
            return Ok(helpers::ok(
                invocation.id,
                helpers::tool_error(
                    "session_already_initialized",
                    "Long-running sprint artifacts already exist. Pass overwrite=true to replace them.",
                    true,
                    Some(
                        "Read `.navi/feature_list.json` and continue the next feature, or call init_session with overwrite=true.",
                    ),
                    None,
                ),
            ));
        }

        fs::create_dir_all(&navi_dir).context("create .navi directory")?;
        let now = now_secs();
        let mut features = Vec::with_capacity(feature_values.len());
        for (index, value) in feature_values.iter().enumerate() {
            let title = value
                .get("title")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .with_context(|| format!("feature {index} missing non-empty title"))?
                .trim()
                .to_string();
            let id = value
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(sanitize_feature_id)
                .unwrap_or_else(|| format!("feature-{}", index + 1));
            let verification_steps = value
                .get("verification_steps")
                .and_then(Value::as_array)
                .context("feature verification_steps must be an array")?
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|step| !step.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            if verification_steps.is_empty() {
                bail!("feature `{id}` must include at least one verification step");
            }
            features.push(FeatureEntry {
                id,
                title,
                description: value
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                verification_steps,
                passes: false,
                notes: None,
                completed_at: None,
            });
        }

        let list = FeatureList {
            schema_version: 1,
            goal,
            features,
            created_at: now,
            updated_at: now,
        };
        write_json(&feature_path, &list)?;
        write_progress(&progress_path, &list)?;

        Ok(helpers::ok(
            invocation.id,
            helpers::versioned(json!({
                "status": "initialized",
                "feature_list": project_relative(self.policy.project_root(), &feature_path),
                "progress": project_relative(self.policy.project_root(), &progress_path),
                "features_total": list.features.len(),
                "next_feature": list.features.first().map(|feature| feature.id.clone()),
            })),
        ))
    }
}

pub(crate) struct MarkFeatureDoneTool {
    policy: SecurityPolicy,
}

impl MarkFeatureDoneTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }
}

#[async_trait]
impl Tool for MarkFeatureDoneTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "mark_feature_done",
            "Run a feature's declared verification commands and mark it passes=true only if every command succeeds. The verification_steps input must exactly match the feature entry in `.navi/feature_list.json`.",
            ToolKind::Command,
            json!({
                "type": "object",
                "properties": {
                    "feature_id": {
                        "type": "string",
                        "description": "Feature id from `.navi/feature_list.json`."
                    },
                    "verification_steps": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Exact commands copied from the feature's verification_steps. They are shown for approval and must match the stored contract."
                    },
                    "notes": {
                        "type": "string",
                        "description": "Optional completion notes to persist in feature_list.json and navi-progress.txt."
                    }
                },
                "required": ["feature_id", "verification_steps"],
                "additionalProperties": false
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let feature_id = helpers::required_string(&invocation.input, "feature_id")?.to_string();
        let provided_steps = invocation
            .input
            .get("verification_steps")
            .and_then(Value::as_array)
            .context("missing required array `verification_steps`")?
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|step| !step.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        let notes = helpers::optional_string(&invocation.input, "notes");

        let navi_dir = navi_dir(self.policy.project_root());
        let feature_path = navi_dir.join(FEATURE_LIST);
        let progress_path = navi_dir.join(PROGRESS_FILE);
        let mut list = read_feature_list(&feature_path)?;
        let Some(index) = list
            .features
            .iter()
            .position(|feature| feature.id == feature_id)
        else {
            return Ok(helpers::ok(
                invocation.id,
                helpers::tool_error(
                    "feature_not_found",
                    format!("No feature with id `{feature_id}` exists in .navi/feature_list.json"),
                    true,
                    Some(
                        "Call init_session first, or list the feature ids from `.navi/feature_list.json`.",
                    ),
                    None,
                ),
            ));
        };
        if list.features[index].verification_steps != provided_steps {
            return Ok(helpers::ok(
                invocation.id,
                helpers::tool_error(
                    "verification_contract_mismatch",
                    "verification_steps must exactly match the stored feature contract",
                    true,
                    Some(
                        "Copy the exact verification_steps array from `.navi/feature_list.json` into mark_feature_done.",
                    ),
                    None,
                ),
            ));
        }

        let mut results = Vec::new();
        for command in &provided_steps {
            let result = run_verification(self.policy.project_root(), command)?;
            let ok = result.ok;
            results.push(result);
            if !ok {
                return Ok(helpers::ok(
                    invocation.id,
                    helpers::versioned(json!({
                        "status": "verification_failed",
                        "feature_id": feature_id,
                        "passes": false,
                        "verification_results": results,
                    })),
                ));
            }
        }

        let now = now_secs();
        list.features[index].passes = true;
        list.features[index].completed_at = Some(now);
        list.features[index].notes = notes.filter(|value| !value.trim().is_empty());
        list.updated_at = now;
        write_json(&feature_path, &list)?;
        write_progress(&progress_path, &list)?;

        let next_feature = list
            .features
            .iter()
            .find(|feature| !feature.passes)
            .map(|feature| feature.id.clone());
        Ok(helpers::ok(
            invocation.id,
            helpers::versioned(json!({
                "status": "feature_completed",
                "feature_id": feature_id,
                "passes": true,
                "next_feature": next_feature,
                "verification_results": results,
            })),
        ))
    }
}

fn navi_dir(project_root: &Path) -> PathBuf {
    project_root.join(".navi")
}

fn read_feature_list(path: &Path) -> Result<FeatureList> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("read feature list at {}", path.display()))?;
    serde_json::from_str(&content).context("parse .navi/feature_list.json")
}

fn write_json(path: &Path, value: &FeatureList) -> Result<()> {
    let content = serde_json::to_string_pretty(value)?;
    fs::write(path, format!("{content}\n")).with_context(|| format!("write {}", path.display()))
}

fn write_progress(path: &Path, list: &FeatureList) -> Result<()> {
    let mut content = format!("# NAVI Long-Running Progress\n\nGoal: {}\n\n", list.goal);
    for feature in &list.features {
        let status = if feature.passes { "done" } else { "pending" };
        content.push_str(&format!(
            "- [{}] `{}` — {}\n",
            status, feature.id, feature.title
        ));
        if !feature.description.is_empty() {
            content.push_str(&format!("  Description: {}\n", feature.description));
        }
        if !feature.verification_steps.is_empty() {
            content.push_str("  Verification:\n");
            for step in &feature.verification_steps {
                content.push_str(&format!("  - `{step}`\n"));
            }
        }
        if let Some(notes) = &feature.notes {
            content.push_str(&format!("  Notes: {notes}\n"));
        }
    }
    fs::write(path, content).with_context(|| format!("write {}", path.display()))
}

fn run_verification(project_root: &Path, command: &str) -> Result<VerificationResult> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(project_root)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("run verification command `{command}`"))?;
    Ok(VerificationResult {
        command: command.to_string(),
        ok: output.status.success(),
        status: output.status.code(),
        stdout: truncate_utf8(output.stdout),
        stderr: truncate_utf8(output.stderr),
    })
}

fn truncate_utf8(bytes: Vec<u8>) -> String {
    let text = String::from_utf8_lossy(&bytes).to_string();
    helpers::truncate_string(text, 16 * 1024)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn sanitize_feature_id(value: &str) -> String {
    let mut out = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    out.trim_matches('-').to_string()
}

fn project_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_feature_ids() {
        assert_eq!(
            sanitize_feature_id("Feature: Auth Flow!"),
            "feature-auth-flow"
        );
    }
}
