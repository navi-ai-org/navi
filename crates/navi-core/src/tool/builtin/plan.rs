use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use super::helpers;
use crate::plan_store::{
    MAX_PLANS, MAX_STEPS, Plan, PlanStatus, PlanStep, PlanStore, now_ms, project_plan_file_path,
    read_plan_file, title_from_markdown, write_plan_file,
};
use crate::security::SecurityPolicy;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

/// Atomic counter for generating unique plan IDs even within the same millisecond.
static PLAN_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Built-in tool for creating and managing work plans with checklist tracking.
pub(crate) struct PlanTool {
    policy: SecurityPolicy,
    /// Opened at construction. If both data_dir and temp fallback fail, invoke
    /// returns a structured error instead of panicking.
    store: Result<PlanStore, String>,
}

impl PlanTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        let store = match PlanStore::open_default(policy.data_dir()) {
            Ok(store) => Ok(store),
            Err(err) => {
                // open_default already targets data_dir/plans.sqlite; retrying the
                // same path cannot help. Fall back to a process-scoped temp DB so
                // tool registration never panics on an unwritable data_dir.
                let fallback = std::env::temp_dir()
                    .join(format!("navi-plans-fallback-{}.sqlite", std::process::id()));
                tracing::warn!(
                    error = %err,
                    path = %fallback.display(),
                    "failed to open plan store under data_dir; trying temp fallback"
                );
                PlanStore::open(&fallback).map_err(|fallback_err| {
                    format!(
                        "failed to open plan store under data_dir ({err}) and temp fallback ({fallback_err})"
                    )
                })
            }
        };
        if let Ok(ref store) = store {
            // Best-effort one-time migration of legacy JSON plans.
            let _ = store.migrate_json_dir(&policy.data_dir().join("plans"));
        }
        Self { policy, store }
    }

    fn project_id(&self) -> String {
        project_hash(self.policy.project_root())
    }

    fn store(&self) -> Result<&PlanStore, &str> {
        self.store.as_ref().map_err(|msg| msg.as_str())
    }

    /// Session plan file when set by plan mode; otherwise project-scoped fallback.
    fn plan_file_path(&self) -> PathBuf {
        self.policy
            .plan_file_path()
            .unwrap_or_else(|| project_plan_file_path(self.policy.data_dir(), &self.project_id()))
    }
}

#[async_trait]
impl Tool for PlanTool {
    fn definition(&self) -> ToolDefinition {
        // Markdown design-doc is the source of truth.
        // steps/todos remain optional for progress tracking after approval.
        helpers::definition(
            "plan",
            "Create, draft, and track a work plan. Source of truth is a **markdown design doc**, \
             not a JSON checklist. Not for one-line fixes. Not the same as set_goal.\n\
             \n\
             Preferred (markdown):\n\
             {\"action\":\"write\",\"plan\":\"# Title\\n\\n## Context\\n...\\n\\n## Approach\\n...\\n\\n\
## Files\\n- path — change\\n\\n## Verification\\ncommand\\n\"}\n\
             Then when ready for user approval:\n\
             {\"action\":\"submit\"}  (reads the plan file; do not re-pass the whole body)\n\
             \n\
             Actions:\n\
             - write: save markdown to the plan file (incremental drafting; no review modal).\n\
             - submit | create: finalize for user review (reads plan file if body omitted).\n\
             - update / complete_step / get / list / active: manage after approval.\n\
             \n\
             Plan structure: Context, Approach (recommended only), Files to modify with paths, \
             Verification. Keep it scannable. Avoid JSON step arrays as the primary content.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["write", "submit", "create", "update", "complete_step", "get", "list", "active"],
                        "description": "write = draft markdown to the plan file; submit/create = present for user review; others manage progress."
                    },
                    "plan_id": {
                        "type": "string",
                        "description": "Plan identifier (required for update, complete_step, get)."
                    },
                    "title": {
                        "type": "string",
                        "description": "Short plan title. Optional if markdown starts with # heading."
                    },
                    "description": {
                        "type": "string",
                        "description": "Optional short summary (prefer full markdown in plan)."
                    },
                    "plan": {
                        "type": "string",
                        "description": "Markdown design doc body (preferred). Sections: Context, Approach, Files, Verification."
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
                        "description": "Optional checklist for progress tracking. Prefer markdown body; steps are derived from lists when omitted.",
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
                        "description": "Alias for steps (TodoWrite-compatible).",
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
        let store = match self.store() {
            Ok(store) => store,
            Err(msg) => {
                return Ok(ToolResult {
                    invocation_id: invocation.id,
                    ok: false,
                    output: helpers::tool_error(
                        "plan_store_unavailable",
                        msg,
                        true,
                        Some("Ensure the NAVI data directory is writable and retry."),
                        None,
                    ),
                });
            }
        };

        let plan_file = self.plan_file_path();
        match action.as_str() {
            "write" => action_write(&invocation, store, &project_id, &plan_file),
            "submit" | "create" => {
                action_submit(&invocation, store, &project_id, &plan_file)
            }
            "update" => action_update(&invocation, store, &plan_file),
            "complete_step" => action_complete_step(&invocation, store),
            "get" => action_get(&invocation, store),
            "list" => action_list(&invocation, store, &project_id),
            "active" => action_active(store, &project_id),
            _ => Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: helpers::tool_error(
                    "unknown_plan_action",
                    format!("unknown plan action: {action}"),
                    true,
                    Some("Use write, submit, create, update, complete_step, get, list, or active."),
                    None,
                ),
            }),
        }
    }
}

// ── Actions ────────────────────────────────────────────────────────────────

/// Draft markdown to the plan file without opening review.
fn action_write(
    invocation: &ToolInvocation,
    store: &PlanStore,
    project_id: &str,
    plan_file: &Path,
) -> Result<ToolResult> {
    let mut body = first_nonempty_string(
        &invocation.input,
        &["plan", "body", "body_markdown", "content"],
    )
    .unwrap_or_default();
    if body.trim().is_empty() {
        return Ok(ToolResult {
            invocation_id: invocation.id.clone(),
            ok: false,
            output: helpers::tool_error(
                "empty_plan",
                "plan markdown body is required for write",
                true,
                Some(
                    "Pass plan=\"# Title\\n\\n## Context\\n...\\n## Approach\\n...\\n\
                     ## Files\\n...\\n## Verification\\n...\"",
                ),
                None,
            ),
        });
    }

    // Ensure a top-level heading if the model omitted one.
    if markdown_title(&body).is_none() {
        if let Some(title) = helpers::optional_string(&invocation.input, "title") {
            if !title.trim().is_empty() {
                body = format!("# {}\n\n{}", title.trim(), body.trim_start());
            }
        }
    }

    write_plan_file(plan_file, &body)?;

    let title = helpers::optional_string(&invocation.input, "title")
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| title_from_markdown(&body));
    let steps = {
        let mut s = steps_from_markdown(&body);
        if s.is_empty() {
            s.push(PlanStep {
                description: "Implement the approved plan".into(),
                completed: false,
                notes: String::new(),
            });
        }
        s
    };

    // Keep a single in-progress draft row (latest proposed for this project).
    let now = now_ms();
    let existing = store
        .list(project_id, Some("proposed"), 1)?
        .into_iter()
        .next();
    let plan = if let Some(mut plan) = existing {
        plan.title = title;
        plan.body_markdown = body.clone();
        plan.steps = steps;
        plan.updated_at = now;
        plan.status = PlanStatus::Proposed;
        plan
    } else {
        let seq = PLAN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Plan {
            id: format!("plan-{now}-{seq}"),
            title,
            description: helpers::optional_string(&invocation.input, "description")
                .unwrap_or_default(),
            steps,
            status: PlanStatus::Proposed,
            created_at: now,
            updated_at: now,
            body_markdown: body.clone(),
            comments: Vec::new(),
            project_id: project_id.to_string(),
            session_id: String::new(),
        }
    };
    store.upsert(&plan)?;

    Ok(helpers::ok(
        invocation.id.clone(),
        json!({
            "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
            "plan_id": plan.id,
            "title": plan.title,
            "plan_file_path": plan_file.display().to_string(),
            "body_chars": body.chars().count(),
            "needs_review": false,
            "message": format!(
                "Plan markdown written to {}. Continue editing the file or call plan(action='submit') when ready for review.",
                plan_file.display()
            ),
        }),
    ))
}

/// Present plan for user review. Reads the plan file when body is omitted.
fn action_submit(
    invocation: &ToolInvocation,
    store: &PlanStore,
    project_id: &str,
    plan_file: &Path,
) -> Result<ToolResult> {
    let mut parsed = parse_create_payload(&invocation.input)?;

    // Submit reads the plan file when body is omitted (plan already on disk).
    if parsed.body_markdown.trim().is_empty() {
        if let Some(from_disk) = read_plan_file(plan_file) {
            parsed.body_markdown = from_disk;
            if parsed.title == "Plan" || parsed.title.is_empty() {
                parsed.title = title_from_markdown(&parsed.body_markdown);
            }
            if parsed.steps.is_empty() {
                parsed.steps = steps_from_markdown(&parsed.body_markdown);
            }
        }
    }

    // If the model wrote via write_file but store has an active draft, merge.
    if parsed.body_markdown.trim().is_empty() {
        if let Some(active) = store.active(project_id)? {
            if !active.body_markdown.trim().is_empty() {
                parsed.body_markdown = active.body_markdown;
                if parsed.title.is_empty() || parsed.title == "Plan" {
                    parsed.title = active.title;
                }
                if parsed.steps.is_empty() {
                    parsed.steps = active.steps;
                }
            }
        }
    }

    if parsed.body_markdown.trim().is_empty() && parsed.steps.is_empty() {
        let hint = format!(
            "Write a markdown plan first: plan(action='write', plan='...') or \
             write_file(path='{}', content='...'), then plan(action='submit').",
            plan_file.display()
        );
        return Ok(ToolResult {
            invocation_id: invocation.id.clone(),
            ok: false,
            output: helpers::tool_error(
                "empty_plan",
                "no plan markdown found",
                true,
                Some(hint.as_str()),
                None,
            ),
        });
    }

    if parsed.steps.is_empty() {
        // Markdown design docs often have no checklist; keep one execution step.
        parsed.steps.push(PlanStep {
            description: "Implement the approved plan".into(),
            completed: false,
            notes: String::new(),
        });
    }

    // Persist markdown to the plan file so review/approve share one artifact.
    if !parsed.body_markdown.trim().is_empty() {
        let _ = write_plan_file(plan_file, &parsed.body_markdown);
    }

    let now = now_ms();
    let seq = PLAN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
            "body_markdown": plan.body_markdown,
            "plan_file_path": plan_file.display().to_string(),
            "steps": plan.steps.iter().map(|s| json!({
                "description": s.description,
                "completed": s.completed,
                "notes": s.notes,
            })).collect::<Vec<_>>(),
            "steps_count": plan.steps.len(),
            "status": format!("{}", plan.status),
            "needs_review": true,
            "message": format!(
                "Plan '{}' ready for user review ({}).",
                plan.title,
                plan_file.display()
            ),
        }),
    ))
}

fn action_update(
    invocation: &ToolInvocation,
    store: &PlanStore,
    plan_file: &Path,
) -> Result<ToolResult> {
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
    if let Some(body) =
        first_nonempty_string(&invocation.input, &["plan", "body", "body_markdown", "content"])
    {
        plan.body_markdown = body;
        let _ = write_plan_file(plan_file, &plan.body_markdown);
        if plan.steps.is_empty() {
            plan.steps = steps_from_markdown(&plan.body_markdown);
        }
    }
    if invocation.input.get("steps").is_some() || invocation.input.get("todos").is_some() {
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
            "plan_file_path": plan_file.display().to_string(),
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
    let plan = store.active(project_id)?.or_else(|| {
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
    let mut description = helpers::optional_string(input, "description").unwrap_or_default();
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

/// Extract checklist items from markdown (CreatePlan bodies).
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
        "body_markdown": plan.body_markdown,
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
        assert!(
            result.ok,
            "title-only create should soft-recover: {:?}",
            result.output
        );
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
        assert_eq!(result.output["error_code"], "empty_plan");
    }

    #[tokio::test]
    async fn write_then_submit_reads_plan_file() {
        let (policy, _td) = temp_policy();
        let tool = PlanTool::new(policy);
        let md = "# Range picker\n\n## Context\nNeed episode ranges.\n\n## Approach\nAdd De/Até inputs.\n\n## Files\n- `src/ui.tsx` — inputs\n\n## Verification\ncargo test\n";
        let write = make_invocation(
            "w1",
            json!({
                "action": "write",
                "plan": md,
            }),
        );
        let result = tool.invoke(write).await.unwrap();
        assert!(result.ok, "{:?}", result.output);
        assert_eq!(result.output["needs_review"], false);
        assert!(result.output["plan_file_path"].as_str().unwrap().ends_with(".md"));

        // submit without body — should read the file
        let submit = make_invocation("s1", json!({ "action": "submit" }));
        let result = tool.invoke(submit).await.unwrap();
        assert!(result.ok, "{:?}", result.output);
        assert_eq!(result.output["needs_review"], true);
        assert_eq!(result.output["title"], "Range picker");
        let body = result.output["body_markdown"].as_str().unwrap();
        assert!(body.contains("## Context"));
        assert!(body.contains("src/ui.tsx"));
    }

    #[tokio::test]
    async fn create_markdown_design_doc_without_list_steps() {
        let (policy, _td) = temp_policy();
        let tool = PlanTool::new(policy);
        let inv = make_invocation(
            "c-md",
            json!({
                "action": "create",
                "plan": "# Auth rewrite\n\n## Context\nSessions are fragile.\n\n## Approach\nUse JWT in httpOnly cookies.\n\n## Files\n- `auth.rs` — token mint\n\n## Verification\ncargo test -p navi-core\n"
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok, "{:?}", result.output);
        assert_eq!(result.output["title"], "Auth rewrite");
        assert!(
            result.output["body_markdown"]
                .as_str()
                .unwrap()
                .contains("JWT")
        );
        // Derived checklist or synthetic execution step
        assert!(result.output["steps_count"].as_u64().unwrap() >= 1);
        assert_eq!(result.output["needs_review"], true);
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
            result.output["plan_status"] == "proposed" || result.output["plan_status"] == "active"
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
