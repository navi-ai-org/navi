use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::hash::{Hash, Hasher};
use std::path::Path;

use super::helpers;
use crate::plan_store::{
    MAX_PLANS, MAX_STEPS, Plan, PlanStatus, PlanStep, PlanStore, now_ms,
};
use crate::security::SecurityPolicy;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

/// Atomic counter for generating unique plan IDs even within the same millisecond.
static PLAN_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Built-in tool for creating and managing work plans with checklist tracking.
pub(crate) struct PlanTool {
    policy: SecurityPolicy,
    store: PlanStore,
}

impl PlanTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        let store = PlanStore::open_default(policy.data_dir())
            .unwrap_or_else(|_| {
                // Fallback: in-memory is not available; open a temp path under data_dir.
                PlanStore::open(&policy.data_dir().join("plans.sqlite"))
                    .expect("open plan store")
            });
        // Best-effort one-time migration of legacy JSON plans.
        let _ = store.migrate_json_dir(&policy.data_dir().join("plans"));
        Self { policy, store }
    }

    fn project_id(&self) -> String {
        project_hash(self.policy.project_root())
    }
}

#[async_trait]
impl Tool for PlanTool {
    fn definition(&self) -> ToolDefinition {
        // Schema shape follows Grok/Cursor CreatePlan + agentic post-training norms:
        // models reliably emit markdown plan bodies and/or todos[{id,content}], and often
        // omit nested {description} objects. Multi-action CRUD is kept, but create accepts
        // those familiar shapes so frontier models don't bounce on empty_steps.
        helpers::definition(
            "plan",
            "Create a concise, actionable work plan and track checklist progress.\n\
             \n\
             Prefer create with BOTH a short title and concrete steps (or todos). \
             Creating a plan opens a TUI review modal for approval before execution.\n\
             \n\
             Actions:\n\
             - create: REQUIRED content = steps[] OR todos[] OR plan/body markdown. \
               Title alone is not enough for a good plan.\n\
             - update: change title/description/steps/status of an existing plan_id.\n\
             - complete_step: mark steps[step_index] done (0-based).\n\
             - get / list / active: read plans.\n\
             \n\
             Create example (preferred):\n\
             {\"action\":\"create\",\"title\":\"Add footer meter\",\n\
              \"steps\":[\"Read usage state\",\"Wire footer label\",\"Test meter\"]}\n\
             \n\
             CreatePlan-compatible example:\n\
             {\"action\":\"create\",\"plan\":\"# Title\\n\\nApproach...\\n\",\n\
              \"todos\":[{\"id\":\"wire-ui\",\"content\":\"Wire footer label\"}]}\n\
             \n\
             Do NOT call create with only {\"action\":\"create\",\"title\":\"...\"}.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["create", "update", "complete_step", "get", "list", "active"],
                        "description": "Operation to perform. Use create to finalize a plan for user review."
                    },
                    "plan_id": {
                        "type": "string",
                        "description": "Plan identifier (required for update, complete_step, get)."
                    },
                    "title": {
                        "type": "string",
                        "description": "Short plan title. Optional if plan/body markdown starts with # heading or steps exist."
                    },
                    "description": {
                        "type": "string",
                        "description": "High-level summary (non-checklist prose)."
                    },
                    "plan": {
                        "type": "string",
                        "description": "CreatePlan-style markdown body. First line may be '# Title'. Bullet/numbered lists become steps when steps/todos omitted."
                    },
                    "body": {
                        "type": "string",
                        "description": "Alias for plan (markdown body)."
                    },
                    "body_markdown": {
                        "type": "string",
                        "description": "Alias for plan (markdown body)."
                    },
                    "content": {
                        "type": "string",
                        "description": "Alias for plan (markdown body)."
                    },
                    "overview": {
                        "type": "string",
                        "description": "Optional one-line overview; stored in description if description is empty."
                    },
                    "steps": {
                        "type": "array",
                        "description": "Checklist steps for create/update. Each item may be a string OR an object with description|content|title|text. Prefer 3–12 concrete steps.",
                        "minItems": 1,
                        "items": {
                            "anyOf": [
                                { "type": "string", "minLength": 1 },
                                {
                                    "type": "object",
                                    "properties": {
                                        "description": { "type": "string" },
                                        "content": { "type": "string" },
                                        "title": { "type": "string" },
                                        "text": { "type": "string" },
                                        "id": { "type": "string" },
                                        "completed": { "type": "boolean" },
                                        "notes": { "type": "string" }
                                    },
                                    "additionalProperties": true
                                }
                            ]
                        }
                    },
                    "todos": {
                        "type": "array",
                        "description": "CreatePlan/TodoWrite-compatible alias for steps. Items: string or {id, content|description}.",
                        "minItems": 1,
                        "items": {
                            "anyOf": [
                                { "type": "string", "minLength": 1 },
                                {
                                    "type": "object",
                                    "properties": {
                                        "id": { "type": "string" },
                                        "content": { "type": "string" },
                                        "description": { "type": "string" },
                                        "title": { "type": "string" },
                                        "text": { "type": "string" },
                                        "completed": { "type": "boolean" },
                                        "notes": { "type": "string" }
                                    },
                                    "additionalProperties": true
                                }
                            ]
                        }
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
        let project_id = self.project_id();

        match action.as_str() {
            "create" => action_create(&invocation, &self.store, &project_id),
            "update" => action_update(&invocation, &self.store),
            "complete_step" => action_complete_step(&invocation, &self.store),
            "get" => action_get(&invocation, &self.store),
            "list" => action_list(&invocation, &self.store, &project_id),
            "active" => action_active(&self.store, &project_id),
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

fn action_create(
    invocation: &ToolInvocation,
    store: &PlanStore,
    project_id: &str,
) -> Result<ToolResult> {
    let parsed = parse_create_payload(&invocation.input)?;

    if parsed.steps.is_empty() {
        return Ok(ToolResult {
            invocation_id: invocation.id.clone(),
            ok: false,
            output: helpers::tool_error(
                "empty_steps",
                "a plan must have at least one step",
                true,
                Some(
                    "Provide steps (array of strings or {description}), \
                     todos ([{id,content}]), or a markdown plan/body with a checklist. \
                     Example: {\"action\":\"create\",\"title\":\"…\",\
                     \"steps\":[\"Step one\",\"Step two\"]}",
                ),
                None,
            ),
        });
    }

    let now = now_ms();
    let seq = PLAN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Proposed until the user reviews in the TUI modal.
    let plan = Plan {
        id: format!("plan-{now}-{seq}"),
        title: parsed.title,
        description: parsed.description,
        steps: parsed.steps,
        status: PlanStatus::Proposed,
        created_at: now,
        updated_at: now,
        body_markdown: parsed.body_markdown,
        comments: Vec::new(),
        project_id: project_id.to_string(),
        session_id: String::new(),
    };

    store.upsert(&plan)?;

    Ok(helpers::ok(
        invocation.id.clone(),
        json!({
            "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
            "plan_id": plan.id,
            "title": plan.title,
            "description": plan.description,
            "steps": plan.steps.iter().map(|s| json!({
                "description": s.description,
                "completed": s.completed,
                "notes": s.notes,
            })).collect::<Vec<_>>(),
            "steps_count": plan.steps.len(),
            "status": format!("{}", plan.status),
            "needs_review": true,
            "message": format!(
                "Plan '{}' created with {} steps. Awaiting user review.",
                plan.title,
                plan.steps.len()
            ),
        }),
    ))
}

fn action_update(invocation: &ToolInvocation, store: &PlanStore) -> Result<ToolResult> {
    let plan_id = required_plan_id(invocation)?;
    let mut plan = store
        .get(&plan_id)?
        .ok_or_else(|| anyhow::anyhow!("plan '{plan_id}' not found"))?;

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
        plan.status = PlanStatus::parse(&status)?;
    }

    plan.updated_at = now_ms();
    store.upsert(&plan)?;

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

fn action_complete_step(invocation: &ToolInvocation, store: &PlanStore) -> Result<ToolResult> {
    let plan_id = required_plan_id(invocation)?;
    let step_index = helpers::optional_u64(&invocation.input, "step_index")
        .ok_or_else(|| anyhow::anyhow!("missing required 'step_index'"))?
        as usize;
    let notes = helpers::optional_string(&invocation.input, "step_notes")
        .or_else(|| helpers::optional_string(&invocation.input, "notes"))
        .unwrap_or_default();

    let mut plan = store
        .get(&plan_id)?
        .ok_or_else(|| anyhow::anyhow!("plan '{plan_id}' not found"))?;

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

    let all_done = plan.steps.iter().all(|s| s.completed);
    if all_done && matches!(plan.status, PlanStatus::Active | PlanStatus::Proposed) {
        plan.status = PlanStatus::Completed;
    }

    plan.updated_at = now_ms();
    store.upsert(&plan)?;

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

fn action_get(invocation: &ToolInvocation, store: &PlanStore) -> Result<ToolResult> {
    let plan_id = required_plan_id(invocation)?;
    let plan = store
        .get(&plan_id)?
        .ok_or_else(|| anyhow::anyhow!("plan '{plan_id}' not found"))?;
    Ok(helpers::ok(invocation.id.clone(), plan_to_json(&plan)))
}

fn action_list(
    invocation: &ToolInvocation,
    store: &PlanStore,
    project_id: &str,
) -> Result<ToolResult> {
    let filter = helpers::optional_string(&invocation.input, "filter_status");
    let plans = store.list(project_id, filter.as_deref(), MAX_PLANS)?;

    let summaries: Vec<Value> = plans
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

fn action_active(store: &PlanStore, project_id: &str) -> Result<ToolResult> {
    // Prefer active; fall back to proposed (awaiting user review).
    let plan = store
        .active(project_id)?
        .or_else(|| {
            store
                .list(project_id, Some("proposed"), 1)
                .ok()
                .and_then(|mut v| v.pop())
        });
    match plan {
        Some(plan) => Ok(helpers::ok("active".to_string(), plan_to_json(&plan))),
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

struct ParsedCreate {
    title: String,
    description: String,
    body_markdown: String,
    steps: Vec<PlanStep>,
}

fn required_plan_id(invocation: &ToolInvocation) -> Result<String> {
    helpers::required_string(&invocation.input, "plan_id").map(|s| s.to_string())
}

fn derive_plan_title(description: &str, steps: &[PlanStep], body: &str) -> String {
    if !description.trim().is_empty() {
        let first = description
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("Plan");
        return truncate_title(first);
    }
    if let Some(heading) = markdown_title(body) {
        return heading;
    }
    steps
        .first()
        .map(|step| step.description.trim())
        .filter(|title| !title.is_empty())
        .map(truncate_title)
        .unwrap_or_else(|| "Plan".to_string())
}

fn truncate_title(s: &str) -> String {
    let t = s.trim();
    if t.chars().count() <= 80 {
        t.to_string()
    } else {
        let mut out: String = t.chars().take(79).collect();
        out.push('…');
        out
    }
}

fn markdown_title(body: &str) -> Option<String> {
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix("# ") {
            let title = rest.trim();
            if !title.is_empty() {
                return Some(truncate_title(title));
            }
        }
        // First non-empty non-heading line as weak title fallback.
        return Some(truncate_title(t.trim_start_matches('#').trim()));
    }
    None
}

/// Resolve create payload from the shapes frontier agentic models actually emit:
/// - navi native: steps:[{description}]
/// - string list: steps:["a","b"]
/// - CreatePlan/TodoWrite: todos:[{id,content}]
/// - CreatePlan markdown: plan/body/body_markdown/content
fn parse_create_payload(input: &Value) -> Result<ParsedCreate> {
    let mut description =
        helpers::optional_string(input, "description").unwrap_or_default();
    if description.trim().is_empty() {
        if let Some(overview) = helpers::optional_string(input, "overview") {
            description = overview;
        }
    }

    let body_markdown = first_nonempty_string(input, &["plan", "body", "body_markdown", "content"])
        .unwrap_or_default();

    let mut steps = parse_step_list(input.get("steps"))?;
    if steps.is_empty() {
        steps = parse_step_list(input.get("todos"))?;
    }
    if steps.is_empty() && !body_markdown.trim().is_empty() {
        steps = steps_from_markdown(&body_markdown);
    }

    // Soft recovery for title/description-only creates (common model mistake).
    // Prefer a real checklist next turn; still open review rather than empty_steps loop.
    if steps.is_empty() {
        let title_hint = helpers::optional_string(input, "title").unwrap_or_default();
        let seed = if !description.trim().is_empty() {
            description.trim().to_string()
        } else if !title_hint.trim().is_empty() {
            format!("Implement: {}", title_hint.trim())
        } else {
            String::new()
        };
        if !seed.is_empty() {
            steps.push(PlanStep {
                description: seed,
                completed: false,
                notes: String::new(),
            });
        }
    }

    let title = helpers::optional_string(input, "title")
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| derive_plan_title(&description, &steps, &body_markdown));

    Ok(ParsedCreate {
        title,
        description,
        body_markdown,
        steps,
    })
}

fn first_nonempty_string(input: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = helpers::optional_string(input, key) {
            if !s.trim().is_empty() {
                return Some(s);
            }
        }
    }
    None
}

fn parse_steps(input: &Value) -> Result<Vec<PlanStep>> {
    // Prefer explicit steps, then todos (update path).
    let mut steps = parse_step_list(input.get("steps"))?;
    if steps.is_empty() {
        steps = parse_step_list(input.get("todos"))?;
    }
    Ok(steps)
}

fn parse_step_list(value: Option<&Value>) -> Result<Vec<PlanStep>> {
    let Some(steps_value) = value else {
        return Ok(Vec::new());
    };
    // Single string treated as one step (models occasionally do this).
    if let Some(s) = steps_value.as_str() {
        let t = s.trim();
        if t.is_empty() {
            return Ok(Vec::new());
        }
        return Ok(vec![PlanStep {
            description: t.to_string(),
            completed: false,
            notes: String::new(),
        }]);
    }
    let steps_array = steps_value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("'steps'/'todos' must be an array or string"))?;

    if steps_array.len() > MAX_STEPS {
        return Err(anyhow::anyhow!(
            "too many steps ({}, max {})",
            steps_array.len(),
            MAX_STEPS
        ));
    }

    let mut out = Vec::with_capacity(steps_array.len());
    for s in steps_array {
        if let Some(step) = coerce_step(s)? {
            out.push(step);
        }
    }
    Ok(out)
}

fn coerce_step(value: &Value) -> Result<Option<PlanStep>> {
    if let Some(s) = value.as_str() {
        let t = s.trim();
        if t.is_empty() {
            return Ok(None);
        }
        return Ok(Some(PlanStep {
            description: t.to_string(),
            completed: false,
            notes: String::new(),
        }));
    }
    let Some(obj) = value.as_object() else {
        return Err(anyhow::anyhow!(
            "each step must be a string or object with description/content"
        ));
    };
    let description = ["description", "content", "title", "text", "name", "task"]
        .iter()
        .find_map(|k| obj.get(*k).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // TodoWrite sometimes only has id; skip empty content.
    let description = match description {
        Some(d) => d,
        None => {
            if let Some(id) = obj.get("id").and_then(Value::as_str) {
                let id = id.trim();
                if id.is_empty() {
                    return Ok(None);
                }
                // Prefer readable content; id alone is a weak step label.
                id.replace('-', " ")
            } else {
                return Ok(None);
            }
        }
    };

    let completed = obj
        .get("completed")
        .and_then(Value::as_bool)
        .or_else(|| {
            obj.get("status")
                .and_then(Value::as_str)
                .map(|s| matches!(s.to_ascii_lowercase().as_str(), "completed" | "done"))
        })
        .unwrap_or(false);
    let notes = obj
        .get("notes")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    Ok(Some(PlanStep {
        description,
        completed,
        notes,
    }))
}

/// Extract checklist items from markdown (Grok CreatePlan bodies).
fn steps_from_markdown(body: &str) -> Vec<PlanStep> {
    let mut steps = Vec::new();
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let item = t
            .strip_prefix("- [ ] ")
            .or_else(|| t.strip_prefix("- [x] "))
            .or_else(|| t.strip_prefix("- [X] "))
            .or_else(|| t.strip_prefix("* [ ] "))
            .or_else(|| t.strip_prefix("- "))
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| {
                // "1. foo" / "1) foo"
                let bytes = t.as_bytes();
                let mut i = 0;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i > 0 && i < bytes.len() && (bytes[i] == b'.' || bytes[i] == b')') {
                    Some(t[i + 1..].trim())
                } else {
                    None
                }
            });
        if let Some(text) = item {
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            steps.push(PlanStep {
                description: text.to_string(),
                completed: false,
                notes: String::new(),
            });
            if steps.len() >= MAX_STEPS {
                break;
            }
        }
    }
    // If markdown had no list, use non-heading paragraphs as coarse steps (max 8).
    if steps.is_empty() {
        for para in body.split("\n\n") {
            let t = para
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect::<Vec<_>>()
                .join(" ");
            if t.chars().count() < 8 {
                continue;
            }
            steps.push(PlanStep {
                description: truncate_title(&t),
                completed: false,
                notes: String::new(),
            });
            if steps.len() >= 8 {
                break;
            }
        }
    }
    steps
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
        "needs_review": plan.status == PlanStatus::Proposed,
    })
}

fn project_hash(project_dir: &Path) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    project_dir.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_policy() -> (SecurityPolicy, tempfile::TempDir) {
        let td = tempfile::tempdir().expect("tempdir");
        let project = td.path().join("project");
        let data = td.path().join("data");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&data).unwrap();
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
    async fn create_plan_title_only_soft_recovers() {
        // Models frequently emit only action+title (CreatePlan habit). Soft-recover
        // into a single seed step so the review modal can open instead of empty_steps.
        let (policy, _td) = temp_policy();
        let tool = PlanTool::new(policy);
        let inv = make_invocation(
            "c2",
            json!({
                "action": "create",
                "title": "Simulador gráfico OBR EV3"
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok, "title-only create should soft-recover: {:?}", result.output);
        assert_eq!(result.output["steps_count"], 1);
        assert!(
            result.output["steps"][0]["description"]
                .as_str()
                .unwrap()
                .contains("Simulador")
        );
    }

    #[tokio::test]
    async fn create_plan_accepts_string_steps() {
        let (policy, _td) = temp_policy();
        let tool = PlanTool::new(policy);
        let inv = make_invocation(
            "c2s",
            json!({
                "action": "create",
                "title": "Canvas sim",
                "steps": ["Scaffold HTML", "Draw field", "Wire controls"]
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["steps_count"], 3);
    }

    #[tokio::test]
    async fn create_plan_accepts_todos_createplan_shape() {
        let (policy, _td) = temp_policy();
        let tool = PlanTool::new(policy);
        let inv = make_invocation(
            "c2t",
            json!({
                "action": "create",
                "plan": "# Canvas simulator\n\nUse HTML+Canvas without pygame.\n\n- Build index.html\n- Implement robot draw loop\n- Add keyboard controls",
                "todos": [
                    { "id": "scaffold", "content": "Scaffold HTML/CSS/JS" },
                    { "id": "draw", "content": "Draw OBR field" }
                ]
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok, "{:?}", result.output);
        // todos take precedence over markdown list extraction when present
        assert_eq!(result.output["steps_count"], 2);
        assert_eq!(result.output["title"], "Canvas simulator");
    }

    #[tokio::test]
    async fn create_plan_accepts_markdown_only() {
        let (policy, _td) = temp_policy();
        let tool = PlanTool::new(policy);
        let inv = make_invocation(
            "c2m",
            json!({
                "action": "create",
                "body": "# Fix footer meter\n\n1. Read usage state\n2. Wire label\n3. Smoke test UI"
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok, "{:?}", result.output);
        assert_eq!(result.output["title"], "Fix footer meter");
        assert_eq!(result.output["steps_count"], 3);
    }

    #[tokio::test]
    async fn create_plan_rejects_truly_empty() {
        let (policy, _td) = temp_policy();
        let tool = PlanTool::new(policy);
        let inv = make_invocation("c2e", json!({ "action": "create" }));
        let result = tool.invoke(inv).await.unwrap();
        assert!(!result.ok);
        assert_eq!(result.output["error_code"], "empty_steps");
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
        // Created plans start as proposed until TUI review.
        assert!(
            result.output["plan_status"] == "proposed"
                || result.output["plan_status"] == "active"
        );

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
