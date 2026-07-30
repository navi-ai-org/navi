//! ATIF v1.7 (Agent Trajectory Interchange Format) exporter.
//!
//! Folds a persisted [`SessionSnapshot`] event log into a single ATIF v1.7
//! trajectory document suitable for external SFT / RL / analysis pipelines.
//!
//! The event log stays the ground truth; this module is a read-only projection.
//! Lifecycle / approval / plan / goal / compaction-detail events with no ATIF
//! equivalent are dropped (counted in `trajectory.notes`), mirroring the
//! "observability detail with no ATIF equivalent" convention used by other
//! producers. Secret redaction reuses [`crate::security::redact_snapshot_events`]
//! so every export path shares one scrubbing policy.
//!
//! Spec: Harbor RFC 0001 — `ATIF-v1.7`.
//!
//! Mapping summary (`AgentEvent` → ATIF `Step`):
//! - `UserTaskSubmitted` → `source: "user"` step (multimodal → `ContentPart[]`).
//! - `ModelOutput` → opens a new `source: "agent"` step (`message`, `reasoning_content`).
//! - `ToolRequested` → appends a `tool_calls[]` entry to the current agent step
//!   (opens one with empty `message` if none exists).
//! - `ToolCompleted` → appends an `observation.results[]` entry, correlated to
//!   the producing step by `invocation_id` → `tool_call_id`.
//! - `UsageReported` → sets `metrics` on the most recent agent step.
//! - `AutoCompactCompleted` / `Error` / `HarnessStopped` → `source: "system"` step.
//! - Everything else (approvals, questions, plan, goal, dream, recap,
//!   repetition, patch, subagent transcript, notifications, mode change,
//!   micro-compact, stream-resuming, update-available) → dropped.

use crate::event::AgentEvent;
use crate::model::ContentPart;
use crate::security::redact_snapshot_events;
use crate::session::{SessionSnapshot, SessionUsageSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// ATIF schema version produced by this module.
pub const ATIF_VERSION: &str = "ATIF-v1.7";

/// Options controlling ATIF trajectory export from a session snapshot.
#[derive(Debug, Clone)]
pub struct AtifExportOptions<'a> {
    /// Agent system version string (e.g. the NAVI product version).
    pub agent_version: &'a str,
    /// Default model name for the trajectory (step-level overrides win).
    pub model_name: &'a str,
    /// When true, secret-like event content is scrubbed before folding.
    pub redact_secrets: bool,
}

impl<'a> Default for AtifExportOptions<'a> {
    fn default() -> Self {
        Self {
            agent_version: env!("CARGO_PKG_VERSION"),
            model_name: "",
            redact_secrets: true,
        }
    }
}

/// A complete ATIF v1.7 trajectory document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trajectory {
    pub schema_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub agent: Agent,
    pub steps: Vec<Step>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_metrics: Option<FinalMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
}

/// Agent system identification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
}

/// Aggregate trajectory metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FinalMetrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_prompt_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_completion_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cached_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_steps: Option<u64>,
}

/// A single ATIF step (system / user / agent turn).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub step_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    pub source: StepSource,
    /// `message` is required by ATIF; may be an empty string or a multimodal
    /// `ContentPart` array.
    pub message: Message,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation: Option<Observation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
}

/// Step originator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepSource {
    System,
    User,
    Agent,
}

/// `message` / observation `content`: a plain string or a multimodal part array.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Text(String),
    Parts(Vec<AtifContentPart>),
}

impl Message {
    fn text<S: Into<String>>(s: S) -> Self {
        Self::Text(s.into())
    }
    fn empty() -> Self {
        Self::Text(String::new())
    }
}

/// ATIF `ContentPart` (text or image only — audio/video have no ATIF home).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AtifContentPart {
    Text { text: String },
    Image { media_type: String, data: String },
}

/// A tool call within an agent step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool_call_id: String,
    pub function_name: String,
    pub arguments: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
}

/// Environment feedback for a step.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Observation {
    pub results: Vec<ObservationResult>,
}

/// A single observation result, linked back to its tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservationResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
}

/// Per-step LLM operational metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
}

/// Builds an ATIF v1.7 [`Trajectory`] from a persisted session snapshot.
///
/// Applies secret redaction to the event log when `opts.redact_secrets` is set,
/// sharing the same scrubbing policy as [`crate::session::SessionStore`].
pub fn build_trajectory(snapshot: &SessionSnapshot, opts: &AtifExportOptions<'_>) -> Trajectory {
    let events: Vec<AgentEvent> = if opts.redact_secrets {
        redact_snapshot_events(&snapshot.events)
    } else {
        snapshot.events.clone()
    };

    let mut folder = Folder::new();
    folder.fold(&events);
    folder.finalize();

    let final_metrics = build_final_metrics(&folder, snapshot.usage.as_ref());
    let notes = if folder.dropped > 0 {
        Some(format!(
            "Exported from a NAVI session snapshot. Dropped {} event(s) with no ATIF v1.7 \
             equivalent (lifecycle/approval/plan/goal/subagent transcript). Subagent trajectories \
             are not embedded (transient UI transcript items only).",
            folder.dropped
        ))
    } else {
        None
    };

    Trajectory {
        schema_version: ATIF_VERSION.to_string(),
        session_id: Some(snapshot.id.as_str().to_string()),
        agent: Agent {
            name: "navi".to_string(),
            version: opts.agent_version.to_string(),
            model_name: (!opts.model_name.is_empty()).then(|| opts.model_name.to_string()),
            extra: Some(serde_json::json!({
                "title": snapshot.title,
                "project": snapshot.project.display().to_string(),
            })),
        },
        steps: folder.steps,
        notes,
        final_metrics,
        extra: Some(serde_json::json!({
            "snapshot_version": snapshot.version,
            "created_at": snapshot.created_at,
            "updated_at": snapshot.updated_at,
        })),
    }
}

/// Serializes a trajectory as pretty-printed ATIF JSON.
pub fn to_json(trajectory: &Trajectory) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(trajectory)?)
}

struct Folder {
    steps: Vec<Step>,
    /// Index of the currently-open agent step (its `tool_calls`/`observation`
    /// are still being accumulated).
    current: Option<usize>,
    /// Index of the most recent agent step, used to attach `UsageReported`
    /// metrics when no step is open.
    last_agent: Option<usize>,
    /// `tool_call_id` → index of the step that issued the call, so late-arriving
    /// `ToolCompleted` results land in the correct step.
    call_to_step: std::collections::HashMap<String, usize>,
    next_id: u64,
    dropped: u64,
}

impl Folder {
    fn new() -> Self {
        Self {
            steps: Vec::new(),
            current: None,
            last_agent: None,
            call_to_step: std::collections::HashMap::new(),
            next_id: 1,
            dropped: 0,
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn finalize_current(&mut self) {
        self.current = None;
    }

    /// Opens a new agent step with the given message + reasoning, closing any
    /// currently-open one first. Returns its index.
    fn open_agent(&mut self, message: Message, reasoning: Option<String>) -> usize {
        self.finalize_current();
        let id = self.alloc_id();
        let idx = self.steps.len();
        self.steps.push(Step {
            step_id: id,
            timestamp: None,
            source: StepSource::Agent,
            message,
            model_name: None,
            reasoning_content: reasoning,
            tool_calls: Vec::new(),
            observation: None,
            metrics: None,
            extra: None,
        });
        self.current = Some(idx);
        self.last_agent = Some(idx);
        idx
    }

    /// Ensures an agent step is open (creating one with an empty message if
    /// needed) and returns its index.
    fn ensure_agent_open(&mut self) -> usize {
        if let Some(idx) = self.current {
            return idx;
        }
        self.open_agent(Message::empty(), None)
    }

    fn push_user(&mut self, message: Message, timestamp: Option<String>) {
        self.finalize_current();
        let id = self.alloc_id();
        self.steps.push(Step {
            step_id: id,
            timestamp,
            source: StepSource::User,
            message,
            model_name: None,
            reasoning_content: None,
            tool_calls: Vec::new(),
            observation: None,
            metrics: None,
            extra: None,
        });
    }

    fn push_system(&mut self, message: Message, extra: Option<Value>) {
        self.finalize_current();
        let id = self.alloc_id();
        self.steps.push(Step {
            step_id: id,
            timestamp: None,
            source: StepSource::System,
            message,
            model_name: None,
            reasoning_content: None,
            tool_calls: Vec::new(),
            observation: None,
            metrics: None,
            extra,
        });
    }

    fn fold(&mut self, events: &[AgentEvent]) {
        for event in events {
            match event {
                AgentEvent::UserTaskSubmitted {
                    text,
                    content_parts,
                    submitted_at,
                } => {
                    let ts = submitted_at.and_then(format_unix_secs);
                    let msg = user_message(text, content_parts);
                    self.push_user(msg, ts);
                }
                AgentEvent::ModelOutput { text, thinking } => {
                    self.open_agent(Message::text(text.clone()), thinking.clone());
                }
                AgentEvent::ToolRequested(invocation) => {
                    let idx = self.ensure_agent_open();
                    self.call_to_step.insert(invocation.id.clone(), idx);
                    let args = coerce_arguments(&invocation.input);
                    if let Some(step) = self.steps.get_mut(idx) {
                        step.tool_calls.push(ToolCall {
                            tool_call_id: invocation.id.clone(),
                            function_name: invocation.tool_name.clone(),
                            arguments: args,
                            extra: None,
                        });
                    }
                }
                AgentEvent::ToolCompleted(result) => {
                    // When the result has a matching `ToolRequested`, route it
                    // to that step and keep the `source_call_id` correlation.
                    // Orphan results (no matching request, e.g. a result whose
                    // request was dropped or came from a subagent boundary) land
                    // in the current/new agent step with `source_call_id: None`
                    // so the ATIF invariant "source_call_id ⇒ matching tool_call"
                    // always holds.
                    let (target, has_match) = match self.call_to_step.get(&result.invocation_id) {
                        Some(&idx) => (Some(idx), true),
                        None => (self.current, false),
                    };
                    let idx = match target {
                        Some(idx) => idx,
                        None => self.open_agent(Message::empty(), None),
                    };
                    if let Some(step) = self.steps.get_mut(idx) {
                        let obs = step.observation.get_or_insert_with(Observation::default);
                        obs.results.push(ObservationResult {
                            source_call_id: has_match.then(|| result.invocation_id.clone()),
                            content: Some(Message::text(tool_result_text(&result.output))),
                            extra: Some(serde_json::json!({ "ok": result.ok })),
                        });
                    }
                }
                AgentEvent::UsageReported {
                    input_tokens,
                    output_tokens,
                    cache_creation_tokens,
                    cache_read_tokens,
                } => {
                    let idx = self.current.or(self.last_agent);
                    if let Some(idx) = idx {
                        if let Some(step) = self.steps.get_mut(idx) {
                            step.metrics = Some(Metrics {
                                prompt_tokens: Some(*input_tokens),
                                completion_tokens: Some(*output_tokens),
                                cached_tokens: Some(*cache_read_tokens),
                                extra: Some(serde_json::json!({
                                    "cache_creation_tokens": cache_creation_tokens,
                                })),
                            });
                        }
                    }
                }
                AgentEvent::AutoCompactCompleted {
                    tokens_saved,
                    summary,
                    kept_recent_messages,
                } => {
                    let extra = serde_json::json!({
                        "context_management": true,
                        "tokens_saved": tokens_saved,
                        "kept_recent_messages": kept_recent_messages,
                    });
                    self.push_system(
                        Message::text(if summary.trim().is_empty() {
                            "auto-compaction applied".to_string()
                        } else {
                            format!("auto-compaction summary: {summary}")
                        }),
                        Some(extra),
                    );
                }
                AgentEvent::Error { message } => {
                    self.push_system(
                        Message::text(message.clone()),
                        Some(serde_json::json!({ "error": true })),
                    );
                }
                AgentEvent::HarnessStopped {
                    reason,
                    message,
                    tool_name,
                } => {
                    let mut extra = serde_json::Map::new();
                    extra.insert("stop_reason".to_string(), Value::String(reason.clone()));
                    if let Some(name) = tool_name {
                        extra.insert("tool_name".to_string(), Value::String(name.clone()));
                    }
                    self.push_system(Message::text(message.clone()), Some(Value::Object(extra)));
                }
                // No ATIF v1.7 equivalent — dropped.
                AgentEvent::ModelDelta { .. }
                | AgentEvent::ModelThinkingDelta { .. }
                | AgentEvent::ToolCallStreaming { .. }
                | AgentEvent::SubagentActivity { .. }
                | AgentEvent::SubagentTranscript { .. }
                | AgentEvent::HarnessTrace(_)
                | AgentEvent::PatchProposed(_)
                | AgentEvent::ApprovalRequested(_)
                | AgentEvent::ApprovalResolved(_)
                | AgentEvent::CapabilityRecorded(_)
                | AgentEvent::QuestionRequested(_)
                | AgentEvent::QuestionResolved(_)
                | AgentEvent::PlanReviewRequested(_)
                | AgentEvent::PlanReviewResolved(_)
                | AgentEvent::SudoPasswordRequested(_)
                | AgentEvent::RepeatedToolCallWarning { .. }
                | AgentEvent::RepetitionDetected { .. }
                | AgentEvent::AutoDreamStarted { .. }
                | AgentEvent::AutoDreamCompleted { .. }
                | AgentEvent::AutoDreamFailed { .. }
                | AgentEvent::SessionRecap { .. }
                | AgentEvent::StreamResuming { .. }
                | AgentEvent::SetGoalRequested { .. }
                | AgentEvent::GoalUpdated { .. }
                | AgentEvent::MicroCompactApplied { .. }
                | AgentEvent::AutoCompactStarted
                | AgentEvent::AutoCompactFailed { .. }
                | AgentEvent::PlanProposed { .. }
                | AgentEvent::AgentModeChanged { .. }
                | AgentEvent::NotificationRequested { .. }
                | AgentEvent::UpdateAvailable { .. } => {
                    self.dropped += 1;
                }
            }
        }
    }

    fn finalize(&mut self) {
        self.finalize_current();
    }
}

fn build_final_metrics(
    folder: &Folder,
    usage: Option<&SessionUsageSnapshot>,
) -> Option<FinalMetrics> {
    let mut total_prompt = 0u64;
    let mut total_completion = 0u64;
    let mut total_cached = 0u64;
    for step in &folder.steps {
        if let Some(m) = &step.metrics {
            total_prompt += m.prompt_tokens.unwrap_or(0);
            total_completion += m.completion_tokens.unwrap_or(0);
            total_cached += m.cached_tokens.unwrap_or(0);
        }
    }
    let total_cost_usd = usage.and_then(|u| u.cost_known.then_some(u.cost_usd));
    Some(FinalMetrics {
        total_prompt_tokens: (total_prompt > 0).then_some(total_prompt),
        total_completion_tokens: (total_completion > 0).then_some(total_completion),
        total_cached_tokens: (total_cached > 0).then_some(total_cached),
        total_cost_usd,
        total_steps: Some(folder.steps.len() as u64),
    })
}

fn user_message(text: &str, content_parts: &[ContentPart]) -> Message {
    let parts: Vec<AtifContentPart> = content_parts
        .iter()
        .filter_map(|p| match p {
            ContentPart::Text { text } => Some(AtifContentPart::Text { text: text.clone() }),
            ContentPart::Image { media_type, data } => Some(AtifContentPart::Image {
                media_type: media_type.clone(),
                data: data.clone(),
            }),
            // Audio / Video have no ATIF ContentPart home — dropped.
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        return Message::text(text);
    }
    // Prepend the textual prompt only when no text part already carries it
    // (navi often includes the prompt itself in `content_parts`).
    let already_has_text = parts
        .iter()
        .any(|p| matches!(p, AtifContentPart::Text { text: t } if t == text));
    if !text.is_empty() && !already_has_text {
        let mut all = Vec::with_capacity(parts.len() + 1);
        all.push(AtifContentPart::Text {
            text: text.to_string(),
        });
        all.extend(parts);
        Message::Parts(all)
    } else {
        Message::Parts(parts)
    }
}

/// ATIF `arguments` MUST be a JSON object. Coerce non-object inputs into one.
fn coerce_arguments(input: &Value) -> Value {
    if input.is_object() {
        input.clone()
    } else {
        serde_json::json!({ "value": input })
    }
}

/// Renders a tool result `Value` as compact text, dropping the internal
/// multimodal `_navi_content_parts` key (mirrors `session_replay`).
fn tool_result_text(output: &Value) -> String {
    match output {
        Value::String(s) => s.clone(),
        Value::Object(obj) => {
            let mut copy = Map::new();
            for (k, v) in obj {
                if k == crate::tool::NAVI_CONTENT_PARTS_KEY {
                    continue;
                }
                copy.insert(k.clone(), v.clone());
            }
            Value::Object(copy).to_string()
        }
        other => other.to_string(),
    }
}

fn format_unix_secs(secs: u64) -> Option<String> {
    OffsetDateTime::from_unix_timestamp(secs as i64)
        .ok()?
        .format(&Rfc3339)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionId, SessionSnapshot};
    use crate::tool::{ToolInvocation, ToolResult};
    use serde_json::json;
    use std::path::PathBuf;

    fn snapshot(events: Vec<AgentEvent>) -> SessionSnapshot {
        SessionSnapshot {
            version: 1,
            id: SessionId::new("test-session".to_string()),
            title: Some("test".to_string()),
            project: PathBuf::from("/tmp"),
            created_at: 1_700_000_000,
            updated_at: 1_700_000_010,
            events,
            memory: None,
            goal: None,
            usage: None,
        }
    }

    fn opts() -> AtifExportOptions<'static> {
        AtifExportOptions {
            agent_version: "0.0.0-test",
            model_name: "test-model",
            redact_secrets: false,
        }
    }

    #[test]
    fn empty_session_produces_minimal_trajectory() {
        let t = build_trajectory(&snapshot(Vec::new()), &opts());
        assert_eq!(t.schema_version, "ATIF-v1.7");
        assert_eq!(t.agent.name, "navi");
        assert_eq!(t.agent.version, "0.0.0-test");
        assert_eq!(t.agent.model_name.as_deref(), Some("test-model"));
        assert!(t.steps.is_empty());
        assert!(t.notes.is_none());
        assert_eq!(t.final_metrics.unwrap().total_steps, Some(0));
    }

    #[test]
    fn user_then_model_output_maps_to_user_and_agent_steps() {
        let events = vec![
            AgentEvent::UserTaskSubmitted {
                text: "do the thing".to_string(),
                content_parts: Vec::new(),
                submitted_at: Some(1_700_000_000),
            },
            AgentEvent::ModelOutput {
                text: "on it".to_string(),
                thinking: Some("planning".to_string()),
            },
        ];
        let t = build_trajectory(&snapshot(events), &opts());
        assert_eq!(t.steps.len(), 2);
        assert_eq!(t.steps[0].source, StepSource::User);
        assert_eq!(t.steps[0].message, Message::text("do the thing"));
        assert_eq!(
            t.steps[0].timestamp.as_deref(),
            Some("2023-11-14T22:13:20Z")
        );
        assert_eq!(t.steps[1].source, StepSource::Agent);
        assert_eq!(t.steps[1].message, Message::text("on it"));
        assert_eq!(t.steps[1].reasoning_content.as_deref(), Some("planning"));
    }

    #[test]
    fn tool_call_and_result_correlate_into_one_agent_step() {
        let events = vec![
            AgentEvent::ModelOutput {
                text: "writing file".to_string(),
                thinking: None,
            },
            AgentEvent::ToolRequested(ToolInvocation {
                id: "call-1".to_string(),
                tool_name: "write".to_string(),
                input: json!({"path": "/tmp/x", "content": "hi"}),
            }),
            AgentEvent::ToolCompleted(ToolResult {
                invocation_id: "call-1".to_string(),
                ok: true,
                output: json!("File created"),
            }),
        ];
        let t = build_trajectory(&snapshot(events), &opts());
        assert_eq!(t.steps.len(), 1);
        let step = &t.steps[0];
        assert_eq!(step.source, StepSource::Agent);
        assert_eq!(step.tool_calls.len(), 1);
        assert_eq!(step.tool_calls[0].tool_call_id, "call-1");
        assert_eq!(step.tool_calls[0].function_name, "write");
        assert_eq!(
            step.tool_calls[0].arguments,
            json!({"path": "/tmp/x", "content": "hi"})
        );
        let obs = step.observation.as_ref().expect("observation");
        assert_eq!(obs.results.len(), 1);
        assert_eq!(obs.results[0].source_call_id.as_deref(), Some("call-1"));
        assert_eq!(obs.results[0].content, Some(Message::text("File created")));
    }

    #[test]
    fn late_tool_result_lands_in_producing_step() {
        // Result arrives after the next ModelOutput opened a new step; it must
        // still correlate back to the step that issued the call.
        let events = vec![
            AgentEvent::ModelOutput {
                text: "call a".to_string(),
                thinking: None,
            },
            AgentEvent::ToolRequested(ToolInvocation {
                id: "a".to_string(),
                tool_name: "t".to_string(),
                input: json!({}),
            }),
            AgentEvent::ModelOutput {
                text: "call b".to_string(),
                thinking: None,
            },
            AgentEvent::ToolRequested(ToolInvocation {
                id: "b".to_string(),
                tool_name: "t".to_string(),
                input: json!({}),
            }),
            AgentEvent::ToolCompleted(ToolResult {
                invocation_id: "a".to_string(),
                ok: true,
                output: json!("a-result"),
            }),
            AgentEvent::ToolCompleted(ToolResult {
                invocation_id: "b".to_string(),
                ok: true,
                output: json!("b-result"),
            }),
        ];
        let t = build_trajectory(&snapshot(events), &opts());
        assert_eq!(t.steps.len(), 2);
        assert_eq!(t.steps[0].tool_calls.len(), 1);
        assert_eq!(
            t.steps[0].observation.as_ref().unwrap().results[0].content,
            Some(Message::text("a-result"))
        );
        assert_eq!(t.steps[1].tool_calls.len(), 1);
        assert_eq!(
            t.steps[1].observation.as_ref().unwrap().results[0].content,
            Some(Message::text("b-result"))
        );
    }

    #[test]
    fn usage_reported_attaches_metrics_and_final_metrics() {
        let events = vec![
            AgentEvent::ModelOutput {
                text: "hi".to_string(),
                thinking: None,
            },
            AgentEvent::UsageReported {
                input_tokens: 100,
                output_tokens: 5,
                cache_creation_tokens: 10,
                cache_read_tokens: 20,
            },
        ];
        let t = build_trajectory(&snapshot(events), &opts());
        let m = t.steps[0].metrics.as_ref().expect("metrics");
        assert_eq!(m.prompt_tokens, Some(100));
        assert_eq!(m.completion_tokens, Some(5));
        assert_eq!(m.cached_tokens, Some(20));
        assert_eq!(m.extra.as_ref().unwrap()["cache_creation_tokens"], 10);
        let fm = t.final_metrics.unwrap();
        assert_eq!(fm.total_prompt_tokens, Some(100));
        assert_eq!(fm.total_completion_tokens, Some(5));
        assert_eq!(fm.total_cached_tokens, Some(20));
        assert_eq!(fm.total_steps, Some(1));
    }

    #[test]
    fn dropped_events_are_counted_in_notes() {
        let events = vec![
            AgentEvent::ApprovalRequested(crate::event::ApprovalRequest {
                id: "a1".to_string(),
                summary: "approve write".to_string(),
                risk: crate::event::ApprovalRisk::Write,
            }),
            AgentEvent::AutoCompactStarted,
        ];
        let t = build_trajectory(&snapshot(events), &opts());
        assert!(t.steps.is_empty());
        assert!(t.notes.unwrap().contains("2 event(s)"));
    }

    #[test]
    fn auto_compact_emits_system_step() {
        let events = vec![AgentEvent::AutoCompactCompleted {
            tokens_saved: 500,
            summary: "summarized prior turns".to_string(),
            kept_recent_messages: 2,
        }];
        let t = build_trajectory(&snapshot(events), &opts());
        assert_eq!(t.steps.len(), 1);
        assert_eq!(t.steps[0].source, StepSource::System);
        assert!(t.steps[0].message != Message::empty());
        assert_eq!(
            t.steps[0].extra.as_ref().unwrap()["context_management"],
            true
        );
    }

    #[test]
    fn non_object_arguments_are_coerced() {
        let events = vec![
            AgentEvent::ToolRequested(ToolInvocation {
                id: "c".to_string(),
                tool_name: "t".to_string(),
                input: json!("raw-string-input"),
            }),
            AgentEvent::ToolCompleted(ToolResult {
                invocation_id: "c".to_string(),
                ok: true,
                output: json!("ok"),
            }),
        ];
        let t = build_trajectory(&snapshot(events), &opts());
        assert_eq!(
            t.steps[0].tool_calls[0].arguments,
            json!({"value": "raw-string-input"})
        );
    }

    #[test]
    fn multimodal_user_message_becomes_parts_array() {
        let events = vec![AgentEvent::UserTaskSubmitted {
            text: "look at this".to_string(),
            content_parts: vec![
                ContentPart::Text {
                    text: "look at this".to_string(),
                },
                ContentPart::Image {
                    media_type: "image/png".to_string(),
                    data: "base64".to_string(),
                },
            ],
            submitted_at: None,
        }];
        let t = build_trajectory(&snapshot(events), &opts());
        match &t.steps[0].message {
            Message::Parts(parts) => {
                assert_eq!(parts.len(), 2);
                assert!(matches!(parts[0], AtifContentPart::Text { .. }));
                assert!(matches!(parts[1], AtifContentPart::Image { .. }));
            }
            other => panic!("expected Parts, got {other:?}"),
        }
    }

    #[test]
    fn redaction_scrubs_secret_like_content() {
        let events = vec![AgentEvent::UserTaskSubmitted {
            text: "my token is sk-1234567890abcdef".to_string(),
            content_parts: Vec::new(),
            submitted_at: None,
        }];
        let mut o = opts();
        o.redact_secrets = true;
        let t = build_trajectory(&snapshot(events), &o);
        match &t.steps[0].message {
            Message::Text(s) => assert!(!s.contains("1234567890abcdef")),
            _ => panic!("expected text message"),
        }
    }

    #[test]
    fn to_json_roundtrips() {
        let events = vec![AgentEvent::ModelOutput {
            text: "hi".to_string(),
            thinking: None,
        }];
        let t = build_trajectory(&snapshot(events), &opts());
        let json = to_json(&t).unwrap();
        assert!(json.contains("\"ATIF-v1.7\""));
        let back: Trajectory = serde_json::from_str(&json).unwrap();
        assert_eq!(back.steps.len(), 1);
    }

    #[test]
    fn final_metrics_include_cost_when_known() {
        let mut snap = snapshot(vec![AgentEvent::ModelOutput {
            text: "hi".to_string(),
            thinking: None,
        }]);
        snap.usage = Some(SessionUsageSnapshot {
            input_tokens: 100,
            output_tokens: 5,
            cost_usd: 0.012,
            cost_known: true,
            credits_spent: None,
            credit_unit: None,
        });
        let t = build_trajectory(&snap, &opts());
        assert_eq!(t.final_metrics.unwrap().total_cost_usd, Some(0.012));
    }
}
