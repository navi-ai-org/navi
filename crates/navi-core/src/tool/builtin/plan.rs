use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::helpers;
use crate::security::SecurityPolicy;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

/// Maximum number of plans to list.
const MAX_PLANS: usize = 20;
/// Maximum steps per plan.
const MAX_STEPS: usize = 50;

/// A work plan with checklist steps, persisted to NAVI data directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    /// Unique plan identifier (e.g. `"plan-1719612345000"`).
    pub id: String,
    /// Human-readable plan title.
    pub title: String,
    /// Optional high-level description of the goal.
    #[serde(default)]
    pub description: String,
    /// Ordered checklist steps.
    pub steps: Vec<PlanStep>,
    /// Overall plan status.
    pub status: PlanStatus,
    /// Unix timestamp (milliseconds) when the plan was created.
    pub created_at: u64,
    /// Unix timestamp (milliseconds) when the plan was last updated.
    pub updated_at: u64,
}

/// A single step in a plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// Step description.
    pub description: String,
    /// Whether this step is completed.
    pub completed: bool,
    /// Optional notes added when completing or reviewing the step.
    #[serde(default)]
    pub notes: String,
}

/// Plan lifecycle status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PlanStatus {
    /// Plan is actively being worked on.
    Active,
    /// All steps are completed.
    Completed,
    /// Plan was abandoned.
    Abandoned,
}

impl std::fmt::Display for PlanStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanStatus::Active => write!(f, "active"),
            PlanStatus::Completed => write!(f, "completed"),
            PlanStatus::Abandoned => write!(f, "abandoned"),
        }
    }
}

/// Atomic counter for generating unique plan IDs even within the same millisecond.
static PLAN_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Built-in tool for creating and managing work plans with checklist tracking.
pub(crate) struct PlanTool {
    policy: SecurityPolicy,
}

impl PlanTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }

    fn plans_dir(&self) -> PathBuf {
        let project_hash = project_hash(self.policy.project_root());
        self.policy.data_dir().join("plans").join(project_hash)
    }
}

#[async_trait]
impl Tool for PlanTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "plan",
            "Create and manage work plans with checklist steps. Use this for multi-step tasks to track progress. Actions:\n\
             - create: Create a new plan with title, description, and steps. If title is omitted, it is derived from description.\n\
             - update: Add, remove, or reorder steps. Set status.\n\
             - complete_step: Mark a step as completed (by index, 0-based). Add optional notes.\n\
             - get: Retrieve the full plan by id.\n\
             - list: List all plans (optionally filtered by status).\n\
             - active: Get the currently active plan (most recent with status=active).\n\
             Plans are persisted and survive across turns and compaction.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["create", "update", "complete_step", "get", "list", "active"],
                        "description": "Operation to perform on the plan."
                    },
                    "plan_id": {
                        "type": "string",
                        "description": "Plan identifier (required for update, complete_step, get)."
                    },
                    "title": {
                        "type": "string",
                        "description": "Plan title for create/update. Optional for create; defaults to description or first step."
                    },
                    "description": {
                        "type": "string",
                        "description": "High-level plan description (for create/update)."
                    },
                    "steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "description": { "type": "string" },
                                "completed": { "type": "boolean" },
                                "notes": { "type": "string" }
                            },
                            "required": ["description"],
                            "additionalProperties": false
                        },
                        "description": "Checklist steps (for create, or to replace all steps in update)."
                    },
                    "step_index": {
                        "type": "integer",
                        "description": "0-based step index (for complete_step)."
                    },
                    "step_notes": {
                        "type": "string",
                        "description": "Notes to attach when completing a step."
                    },
                    "notes": {
                        "type": "string",
                        "description": "Alias for step_notes, accepted for compatibility."
                    },
                    "status": {
                        "type": "string",
                        "enum": ["active", "completed", "abandoned"],
                        "description": "Set plan status (for update)."
                    },
                    "filter_status": {
                        "type": "string",
                        "enum": ["active", "completed", "abandoned"],
                        "description": "Filter plans by status (for list)."
                    }
                },
                "required": ["action"],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let action = helpers::required_string(&invocation.input, "action")?.to_string();
        let plans_dir = self.plans_dir();

        match action.as_str() {
            "create" => action_create(&invocation, &plans_dir),
            "update" => action_update(&invocation, &plans_dir),
            "complete_step" => action_complete_step(&invocation, &plans_dir),
            "get" => action_get(&invocation, &plans_dir),
            "list" => action_list(&invocation, &plans_dir),
            "active" => action_active(&plans_dir),
            _ => Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: helpers::tool_error(
                    "unknown_plan_action",
                    format!("unknown plan action: {action}"),
                    true,
                    Some("Use create, update, complete_step, get, list, or active."),
                    None,
                ),
            }),
        }
    }
}

// ── Actions ────────────────────────────────────────────────────────────────

fn action_create(invocation: &ToolInvocation, plans_dir: &Path) -> Result<ToolResult> {
    let description =
        helpers::optional_string(&invocation.input, "description").unwrap_or_default();
    let steps = parse_steps(&invocation.input)?;
    let title = helpers::optional_string(&invocation.input, "title")
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| derive_plan_title(&description, &steps));

    if steps.is_empty() {
        return Ok(ToolResult {
            invocation_id: invocation.id.clone(),
            ok: false,
            output: helpers::tool_error(
                "empty_steps",
                "a plan must have at least one step",
                true,
                Some("Provide a 'steps' array with at least one step object."),
                None,
            ),
        });
    }

    let now = current_unix_millis();
    let seq = PLAN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let plan = Plan {
        id: format!("plan-{now}-{seq}"),
        title,
        description,
        steps,
        status: PlanStatus::Active,
        created_at: now,
        updated_at: now,
    };

    save_plan(&plan, plans_dir)?;

    Ok(helpers::ok(
        invocation.id.clone(),
        json!({
            "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
            "plan_id": plan.id,
            "title": plan.title,
            "steps_count": plan.steps.len(),
            "status": format!("{}", plan.status),
            "message": format!("Plan '{}' created with {} steps.", plan.title, plan.steps.len()),
        }),
    ))
}

fn action_update(invocation: &ToolInvocation, plans_dir: &Path) -> Result<ToolResult> {
    let plan_id = required_plan_id(invocation)?;
    let mut plan = load_plan(&plan_id, plans_dir)?;

    if let Some(title) = helpers::optional_string(&invocation.input, "title") {
        plan.title = title;
    }
    if let Some(desc) = helpers::optional_string(&invocation.input, "description") {
        plan.description = desc;
    }
    if invocation.input.get("steps").is_some() {
        let steps = parse_steps(&invocation.input)?;
        plan.steps = steps;
    }
    if let Some(status) = helpers::optional_string(&invocation.input, "status") {
        plan.status = parse_status(&status)?;
    }

    plan.updated_at = current_unix_millis();
    save_plan(&plan, plans_dir)?;

    Ok(helpers::ok(
        invocation.id.clone(),
        json!({
            "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
            "plan_id": plan.id,
            "title": plan.title,
            "steps_count": plan.steps.len(),
            "completed_steps": plan.steps.iter().filter(|s| s.completed).count(),
            "status": format!("{}", plan.status),
        }),
    ))
}

fn action_complete_step(invocation: &ToolInvocation, plans_dir: &Path) -> Result<ToolResult> {
    let plan_id = required_plan_id(invocation)?;
    let step_index = helpers::optional_u64(&invocation.input, "step_index")
        .ok_or_else(|| anyhow::anyhow!("missing required 'step_index'"))?
        as usize;
    let notes = helpers::optional_string(&invocation.input, "step_notes")
        .or_else(|| helpers::optional_string(&invocation.input, "notes"))
        .unwrap_or_default();

    let mut plan = load_plan(&plan_id, plans_dir)?;

    if step_index >= plan.steps.len() {
        return Ok(ToolResult {
            invocation_id: invocation.id.clone(),
            ok: false,
            output: helpers::tool_error(
                "step_index_out_of_range",
                format!(
                    "step_index {} out of range (plan has {} steps)",
                    step_index,
                    plan.steps.len()
                ),
                true,
                Some("Use a 0-based index."),
                None,
            ),
        });
    }

    plan.steps[step_index].completed = true;
    plan.steps[step_index].notes = notes;

    // Auto-complete the plan if all steps are done.
    let all_done = plan.steps.iter().all(|s| s.completed);
    if all_done && plan.status == PlanStatus::Active {
        plan.status = PlanStatus::Completed;
    }

    plan.updated_at = current_unix_millis();
    save_plan(&plan, plans_dir)?;

    let completed_count = plan.steps.iter().filter(|s| s.completed).count();
    let total = plan.steps.len();

    Ok(helpers::ok(
        invocation.id.clone(),
        json!({
            "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
            "plan_id": plan.id,
            "step_index": step_index,
            "step_description": plan.steps[step_index].description,
            "completed_steps": completed_count,
            "total_steps": total,
            "plan_status": format!("{}", plan.status),
            "all_complete": all_done,
        }),
    ))
}

fn action_get(invocation: &ToolInvocation, plans_dir: &Path) -> Result<ToolResult> {
    let plan_id = required_plan_id(invocation)?;
    let plan = load_plan(&plan_id, plans_dir)?;
    Ok(helpers::ok(invocation.id.clone(), plan_to_json(&plan)))
}

fn action_list(invocation: &ToolInvocation, plans_dir: &Path) -> Result<ToolResult> {
    let filter = helpers::optional_string(&invocation.input, "filter_status");
    let plans = load_all_plans(plans_dir)?;

    let filtered: Vec<&Plan> = plans
        .iter()
        .filter(|p| {
            filter
                .as_deref()
                .map(|f| format!("{}", p.status) == f)
                .unwrap_or(true)
        })
        .take(MAX_PLANS)
        .collect();

    let summaries: Vec<Value> = filtered
        .iter()
        .map(|p| {
            json!({
                "plan_id": p.id,
                "title": p.title,
                "status": format!("{}", p.status),
                "steps_total": p.steps.len(),
                "steps_completed": p.steps.iter().filter(|s| s.completed).count(),
                "updated_at": p.updated_at,
            })
        })
        .collect();

    Ok(helpers::ok(
        invocation.id.clone(),
        json!({
            "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
            "plans": summaries,
            "total": summaries.len(),
        }),
    ))
}

fn action_active(plans_dir: &Path) -> Result<ToolResult> {
    let plans = load_all_plans(plans_dir)?;
    let active = plans.iter().find(|p| p.status == PlanStatus::Active);

    match active {
        Some(plan) => Ok(helpers::ok("active".to_string(), plan_to_json(plan))),
        None => Ok(helpers::ok(
            "active".to_string(),
            json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "active_plan": null,
                "message": "No active plan found. Use plan(action='create') to start one.",
            }),
        )),
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn required_plan_id(invocation: &ToolInvocation) -> Result<String> {
    helpers::required_string(&invocation.input, "plan_id").map(|s| s.to_string())
}

fn derive_plan_title(description: &str, steps: &[PlanStep]) -> String {
    if !description.trim().is_empty() {
        return description.trim().to_string();
    }
    steps
        .first()
        .map(|step| step.description.trim())
        .filter(|title| !title.is_empty())
        .unwrap_or("Plan")
        .to_string()
}

fn parse_steps(input: &Value) -> Result<Vec<PlanStep>> {
    let Some(steps_value) = input.get("steps") else {
        return Ok(Vec::new());
    };
    let steps_array = steps_value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("'steps' must be an array"))?;

    if steps_array.len() > MAX_STEPS {
        return Err(anyhow::anyhow!(
            "too many steps ({}, max {})",
            steps_array.len(),
            MAX_STEPS
        ));
    }

    steps_array
        .iter()
        .map(|s| {
            let description = s
                .get("description")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("each step must have a 'description'"))?
                .to_string();
            let completed = s.get("completed").and_then(Value::as_bool).unwrap_or(false);
            let notes = s
                .get("notes")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Ok(PlanStep {
                description,
                completed,
                notes,
            })
        })
        .collect()
}

fn parse_status(s: &str) -> Result<PlanStatus> {
    match s {
        "active" => Ok(PlanStatus::Active),
        "completed" => Ok(PlanStatus::Completed),
        "abandoned" => Ok(PlanStatus::Abandoned),
        _ => Err(anyhow::anyhow!(
            "invalid status '{}', use active/completed/abandoned",
            s
        )),
    }
}

fn save_plan(plan: &Plan, plans_dir: &Path) -> Result<()> {
    fs::create_dir_all(plans_dir)
        .with_context(|| format!("failed to create {}", plans_dir.display()))?;
    let path = plans_dir.join(format!("{}.json", plan.id));
    let data = serde_json::to_vec_pretty(plan)?;
    fs::write(&path, data).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn load_plan(plan_id: &str, plans_dir: &Path) -> Result<Plan> {
    let path = plans_dir.join(format!("{plan_id}.json"));
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read plan '{plan_id}' at {}", path.display()))?;
    serde_json::from_str(&content).with_context(|| format!("failed to parse plan '{plan_id}'"))
}

fn load_all_plans(plans_dir: &Path) -> Result<Vec<Plan>> {
    let mut plans = Vec::new();
    if !plans_dir.exists() {
        return Ok(plans);
    }
    for entry in fs::read_dir(plans_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            if let Ok(content) = fs::read_to_string(&path)
                && let Ok(plan) = serde_json::from_str::<Plan>(&content)
            {
                plans.push(plan);
            }
        }
    }
    plans.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(plans)
}

fn plan_to_json(plan: &Plan) -> Value {
    let steps_json: Vec<Value> = plan
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            json!({
                "index": i,
                "description": s.description,
                "completed": s.completed,
                "notes": s.notes,
            })
        })
        .collect();

    json!({
        "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
        "plan_id": plan.id,
        "title": plan.title,
        "description": plan.description,
        "status": format!("{}", plan.status),
        "steps": steps_json,
        "steps_total": plan.steps.len(),
        "steps_completed": plan.steps.iter().filter(|s| s.completed).count(),
        "created_at": plan.created_at,
        "updated_at": plan.updated_at,
    })
}

fn project_hash(project_dir: &Path) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    project_dir.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_policy() -> (SecurityPolicy, tempfile::TempDir) {
        let td = tempfile::tempdir().expect("tempdir");
        let project = td.path().join("project");
        let data = td.path().join("data");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&data).unwrap();
        let policy =
            SecurityPolicy::new(project, data, crate::config::SecurityConfig::default()).unwrap();
        (policy, td)
    }

    fn make_invocation(id: &str, input: Value) -> ToolInvocation {
        ToolInvocation {
            id: id.to_string(),
            tool_name: "plan".to_string(),
            input,
        }
    }

    #[tokio::test]
    async fn create_plan_with_steps() {
        let (policy, _td) = temp_policy();
        let tool = PlanTool::new(policy);
        let inv = make_invocation(
            "c1",
            json!({
                "action": "create",
                "title": "Refactor auth module",
                "description": "Break monolith into microservices",
                "steps": [
                    { "description": "Read current auth.rs" },
                    { "description": "Identify boundaries" },
                    { "description": "Extract service layer" },
                    { "description": "Write tests" }
                ]
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["steps_count"], 4);
    }

    #[tokio::test]
    async fn create_plan_derives_title_when_omitted() {
        let (policy, _td) = temp_policy();
        let tool = PlanTool::new(policy);
        let inv = make_invocation(
            "c1",
            json!({
                "action": "create",
                "description": "Add extractor support",
                "steps": [{ "description": "Explore code" }]
            }),
        );

        let result = tool.invoke(inv).await.unwrap();

        assert!(result.ok);
        assert_eq!(result.output["title"], "Add extractor support");
    }

    #[tokio::test]
    async fn create_plan_requires_steps() {
        let (policy, _td) = temp_policy();
        let tool = PlanTool::new(policy);
        let inv = make_invocation(
            "c2",
            json!({
                "action": "create",
                "title": "Empty plan"
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(!result.ok);
    }

    #[tokio::test]
    async fn complete_step_and_auto_complete_plan() {
        let (policy, _td) = temp_policy();
        let tool = PlanTool::new(policy);

        // Create
        let inv = make_invocation(
            "c1",
            json!({
                "action": "create",
                "title": "Small plan",
                "steps": [
                    { "description": "Step A" },
                    { "description": "Step B" }
                ]
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        let plan_id = result.output["plan_id"].as_str().unwrap().to_string();

        // Complete step 0
        let inv = make_invocation(
            "c2",
            json!({
                "action": "complete_step",
                "plan_id": plan_id,
                "step_index": 0,
                "step_notes": "done"
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["completed_steps"], 1);
        assert_eq!(result.output["plan_status"], "active");

        // Complete step 1 → plan auto-completes
        let inv = make_invocation(
            "c3",
            json!({
                "action": "complete_step",
                "plan_id": plan_id,
                "step_index": 1,
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["completed_steps"], 2);
        assert_eq!(result.output["plan_status"], "completed");
        assert_eq!(result.output["all_complete"], true);
    }

    #[tokio::test]
    async fn complete_step_accepts_notes_alias() {
        let (policy, _td) = temp_policy();
        let tool = PlanTool::new(policy);
        let create = make_invocation(
            "c1",
            json!({
                "action": "create",
                "title": "Small plan",
                "steps": [{ "description": "Step A" }]
            }),
        );
        let plan_id = tool.invoke(create).await.unwrap().output["plan_id"]
            .as_str()
            .unwrap()
            .to_string();

        let complete = make_invocation(
            "c2",
            json!({
                "action": "complete_step",
                "plan_id": plan_id,
                "step_index": 0,
                "notes": "done via alias"
            }),
        );
        let result = tool.invoke(complete).await.unwrap();

        assert!(result.ok);
    }

    #[tokio::test]
    async fn list_and_get_plans() {
        let (policy, _td) = temp_policy();
        let tool = PlanTool::new(policy);

        // Create two plans
        let inv = make_invocation(
            "c1",
            json!({
                "action": "create",
                "title": "Plan A",
                "steps": [{ "description": "Do thing" }]
            }),
        );
        tool.invoke(inv).await.unwrap();

        let inv = make_invocation(
            "c2",
            json!({
                "action": "create",
                "title": "Plan B",
                "steps": [{ "description": "Do other thing" }]
            }),
        );
        tool.invoke(inv).await.unwrap();

        // List
        let inv = make_invocation("c3", json!({ "action": "list" }));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["total"], 2);
    }

    #[tokio::test]
    async fn active_plan_returns_most_recent() {
        let (policy, _td) = temp_policy();
        let tool = PlanTool::new(policy);

        let inv = make_invocation(
            "c1",
            json!({
                "action": "create",
                "title": "My Plan",
                "steps": [{ "description": "Do thing" }]
            }),
        );
        tool.invoke(inv).await.unwrap();

        let inv = make_invocation("c2", json!({ "action": "active" }));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["title"], "My Plan");
    }

    #[tokio::test]
    async fn step_index_out_of_range() {
        let (policy, _td) = temp_policy();
        let tool = PlanTool::new(policy);

        let inv = make_invocation(
            "c1",
            json!({
                "action": "create",
                "title": "Small plan",
                "steps": [{ "description": "One step" }]
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        let plan_id = result.output["plan_id"].as_str().unwrap().to_string();

        let inv = make_invocation(
            "c2",
            json!({
                "action": "complete_step",
                "plan_id": plan_id,
                "step_index": 5
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(!result.ok);
    }

    #[tokio::test]
    async fn update_plan_adds_description() {
        let (policy, _td) = temp_policy();
        let tool = PlanTool::new(policy);

        let inv = make_invocation(
            "c1",
            json!({
                "action": "create",
                "title": "Plan",
                "steps": [{ "description": "Step 1" }]
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        let plan_id = result.output["plan_id"].as_str().unwrap().to_string();

        let inv = make_invocation(
            "c2",
            json!({
                "action": "update",
                "plan_id": plan_id,
                "description": "Added description after thought"
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);

        // Verify
        let inv = make_invocation("c3", json!({ "action": "get", "plan_id": plan_id }));
        let result = tool.invoke(inv).await.unwrap();
        assert_eq!(
            result.output["description"],
            "Added description after thought"
        );
    }
}
