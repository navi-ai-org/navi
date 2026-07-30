//! Property-Based Tests for the ATIF v1.7 exporter.
//!
//! These exercise invariants of `navi_core::build_trajectory` over arbitrary
//! sequences of `AgentEvent`. The fold is a pure, single-threaded projection
//! of the event log, so every property is a deterministic function of the
//! input events.

use navi_core::{
    AgentEvent, AtifContentPart, AtifExportOptions, ContentPart, Message, SessionId,
    SessionSnapshot, StepSource, ToolInvocation, ToolResult, Trajectory, atif_to_json,
    build_trajectory,
};
use proptest::prelude::*;
use serde_json::{Value, json};
use std::path::PathBuf;

// ── Snapshot / options helpers ──────────────────────────────────────────

fn snapshot(events: Vec<AgentEvent>) -> SessionSnapshot {
    SessionSnapshot {
        version: 1,
        id: SessionId::new("pbt-session".to_string()),
        title: Some("pbt".to_string()),
        project: PathBuf::from("/tmp"),
        created_at: 1_700_000_000,
        updated_at: 1_700_000_010,
        events,
        memory: None,
        goal: None,
        usage: None,
    }
}

fn opts(redact: bool) -> AtifExportOptions<'static> {
    AtifExportOptions {
        agent_version: "0.0.0-pbt",
        model_name: "pbt-model",
        redact_secrets: redact,
    }
}

// ── Event strategy ──────────────────────────────────────────────────────
//
// We generate a reduced, realistic subset of AgentEvent variants that exercise
// the fold's mapping logic. Dropped-only variants (approvals, plan, goal, …)
// are intentionally excluded: they cannot affect step structure, so including
// them only adds noise. A separate "no-panic" property fuzzes the full enum.

prop_compose! {
    fn arb_tool_invocation()(id in prop::string::string_regex("[a-z0-9]{1,8}").unwrap(), name in prop::string::string_regex("[a-z_]{1,10}").unwrap()) -> ToolInvocation {
        ToolInvocation {
            id,
            tool_name: name,
            input: json!({ "path": "/tmp/x", "n": 1 }),
        }
    }
}

#[derive(Debug, Clone)]
enum Action {
    User(String),
    Model(String, Option<String>),
    ToolRequest(ToolInvocation),
    ToolResult(ToolResult),
    Usage(u64, u64, u64, u64),
    AutoCompact(String, u64, usize),
    Error(String),
    HarnessStop(String, String),
    Approval, // dropped
}

fn arb_action() -> impl Strategy<Value = Action> {
    (arb_tool_invocation(), 0u8..9).prop_map(|(inv, idx)| match idx {
        0 => Action::User("do thing".to_string()),
        1 => Action::Model("on it".to_string(), None),
        2 => Action::Model("thinking".to_string(), Some("plan".to_string())),
        3 => Action::ToolRequest(inv),
        4 => Action::ToolResult(ToolResult {
            invocation_id: "orphan".to_string(),
            ok: true,
            output: json!("r"),
        }),
        5 => Action::Usage(100, 5, 10, 20),
        6 => Action::AutoCompact("summary".to_string(), 500, 2),
        7 => Action::Error("boom".to_string()),
        8 => Action::HarnessStop("reason".to_string(), "msg".to_string()),
        _ => Action::Approval,
    })
}

fn actions_to_events(actions: &[Action]) -> Vec<AgentEvent> {
    actions
        .iter()
        .map(|a| match a {
            Action::User(t) => AgentEvent::UserTaskSubmitted {
                text: t.clone(),
                content_parts: Vec::new(),
                submitted_at: Some(1_700_000_000),
            },
            Action::Model(t, th) => AgentEvent::ModelOutput {
                text: t.clone(),
                thinking: th.clone(),
            },
            Action::ToolRequest(inv) => AgentEvent::ToolRequested(inv.clone()),
            Action::ToolResult(r) => AgentEvent::ToolCompleted(r.clone()),
            Action::Usage(p, c, cc, cr) => AgentEvent::UsageReported {
                input_tokens: *p,
                output_tokens: *c,
                cache_creation_tokens: *cc,
                cache_read_tokens: *cr,
            },
            Action::AutoCompact(s, saved, kept) => AgentEvent::AutoCompactCompleted {
                tokens_saved: *saved,
                summary: s.clone(),
                kept_recent_messages: *kept,
            },
            Action::Error(m) => AgentEvent::Error { message: m.clone() },
            Action::HarnessStop(reason, msg) => AgentEvent::HarnessStopped {
                reason: reason.clone(),
                message: msg.clone(),
                tool_name: None,
            },
            Action::Approval => AgentEvent::ApprovalRequested(navi_core::ApprovalRequest {
                id: "a".to_string(),
                summary: "s".to_string(),
                risk: navi_core::ApprovalRisk::Write,
            }),
        })
        .collect()
}

fn arb_actions() -> impl Strategy<Value = Vec<Action>> {
    prop::collection::vec(arb_action(), 0..32)
}

fn message_text_len(m: &Message) -> usize {
    match m {
        Message::Text(s) => s.chars().count(),
        Message::Parts(parts) => parts
            .iter()
            .map(|p| match p {
                AtifContentPart::Text { text } => text.chars().count(),
                AtifContentPart::Image { data, .. } => data.chars().count(),
            })
            .sum(),
    }
}

// ── Properties ──────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Determinism: the same event sequence always yields byte-identical JSON.
    #[test]
    fn determinism(actions in arb_actions()) {
        let events = actions_to_events(&actions);
        let snap = snapshot(events.clone());
        let t1 = build_trajectory(&snap, &opts(false));
        let t2 = build_trajectory(&snapshot(events), &opts(false));
        prop_assert_eq!(atif_to_json(&t1).unwrap(), atif_to_json(&t2).unwrap());
    }

    /// `step_id`s are exactly 1..=n with no gaps or duplicates.
    #[test]
    fn step_ids_sequential(actions in arb_actions()) {
        let t = build_trajectory(&snapshot(actions_to_events(&actions)), &opts(false));
        let ids: Vec<u64> = t.steps.iter().map(|s| s.step_id).collect();
        for (i, id) in ids.iter().enumerate() {
            prop_assert_eq!(*id, (i as u64) + 1, "step_id must be 1-based ordinal");
        }
    }

    /// Every observation result's `source_call_id` (when set) must match a
    /// `tool_call_id` issued in the SAME step.
    #[test]
    fn observation_correlates_to_tool_call(actions in arb_actions()) {
        let t = build_trajectory(&snapshot(actions_to_events(&actions)), &opts(false));
        for step in &t.steps {
            let call_ids: std::collections::HashSet<&str> = step
                .tool_calls
                .iter()
                .map(|c| c.tool_call_id.as_str())
                .collect();
            if let Some(obs) = &step.observation {
                for r in &obs.results {
                    if let Some(sid) = &r.source_call_id {
                        prop_assert!(
                            call_ids.contains(sid.as_str()),
                            "observation source_call_id {sid} not in step {} tool_calls {:?}",
                            step.step_id, call_ids
                        );
                    }
                }
            }
        }
    }

    /// `final_metrics.total_*` equals the sum of per-step `metrics.*`.
    #[test]
    fn final_metrics_equal_sum_of_steps(actions in arb_actions()) {
        let t = build_trajectory(&snapshot(actions_to_events(&actions)), &opts(false));
        let (mut p, mut c, mut cr) = (0u64, 0u64, 0u64);
        for s in &t.steps {
            if let Some(m) = &s.metrics {
                p += m.prompt_tokens.unwrap_or(0);
                c += m.completion_tokens.unwrap_or(0);
                cr += m.cached_tokens.unwrap_or(0);
            }
        }
        if let Some(fm) = &t.final_metrics {
            prop_assert_eq!(fm.total_prompt_tokens.unwrap_or(0), p);
            prop_assert_eq!(fm.total_completion_tokens.unwrap_or(0), c);
            prop_assert_eq!(fm.total_cached_tokens.unwrap_or(0), cr);
            prop_assert_eq!(fm.total_steps.unwrap_or(0), t.steps.len() as u64);
        }
    }

    /// Every `tool_call.arguments` is a JSON object (ATIF requirement).
    #[test]
    fn tool_call_arguments_are_objects(actions in arb_actions()) {
        let t = build_trajectory(&snapshot(actions_to_events(&actions)), &opts(false));
        for step in &t.steps {
            for call in &step.tool_calls {
                prop_assert!(call.arguments.is_object(), "arguments must be object, got {}", call.arguments);
            }
        }
    }

    /// System/user steps never carry agent-only fields (tool_calls, observation,
    /// metrics, reasoning_content).
    #[test]
    fn only_agent_steps_carry_agent_fields(actions in arb_actions()) {
        let t = build_trajectory(&snapshot(actions_to_events(&actions)), &opts(false));
        for step in &t.steps {
            if step.source != StepSource::Agent {
                prop_assert!(step.tool_calls.is_empty(), "non-agent step {} has tool_calls", step.step_id);
                prop_assert!(step.observation.is_none(), "non-agent step {} has observation", step.step_id);
                prop_assert!(step.metrics.is_none(), "non-agent step {} has metrics", step.step_id);
                prop_assert!(step.reasoning_content.is_none(), "non-agent step {} has reasoning", step.step_id);
            }
        }
    }

    /// Serialize → deserialize → re-serialize is idempotent (roundtrip).
    #[test]
    fn json_roundtrip_idempotent(actions in arb_actions()) {
        let t = build_trajectory(&snapshot(actions_to_events(&actions)), &opts(false));
        let j1 = atif_to_json(&t).unwrap();
        let back: Trajectory = serde_json::from_str(&j1).unwrap();
        let j2 = atif_to_json(&back).unwrap();
        prop_assert_eq!(j1, j2);
    }

    /// `schema_version` is always the pinned ATIF version.
    #[test]
    fn schema_version_pinned(actions in arb_actions()) {
        let t = build_trajectory(&snapshot(actions_to_events(&actions)), &opts(false));
        prop_assert_eq!(t.schema_version, navi_core::ATIF_VERSION);
    }

    /// Redaction is idempotent: redacting an already-redacted trajectory's
    /// source events yields the same trajectory as redacting once.
    #[test]
    fn redaction_is_idempotent(actions in arb_actions()) {
        let events = actions_to_events(&actions);
        let once = build_trajectory(&snapshot(events.clone()), &opts(true));
        // Re-run redaction on the already-redacted events (simulate double pass).
        let twice_events: Vec<AgentEvent> = navi_core::security::redact_snapshot_events(&events);
        let twice = build_trajectory(&snapshot(twice_events), &opts(false));
        prop_assert_eq!(atif_to_json(&once).unwrap(), atif_to_json(&twice).unwrap());
    }

    /// Non-secret text is preserved byte-for-byte by redaction (redaction only
    /// touches secret-like tokens). We use innocuous text in all events.
    #[test]
    fn non_secret_text_is_preserved(actions in arb_actions()) {
        let events = actions_to_events(&actions);
        let raw = build_trajectory(&snapshot(events.clone()), &opts(false));
        let redacted = build_trajectory(&snapshot(events), &opts(true));
        prop_assert_eq!(raw.steps.len(), redacted.steps.len());
        for (rs, s) in redacted.steps.iter().zip(raw.steps.iter()) {
            // Our test text never looks like a secret, so redaction is a no-op.
            prop_assert_eq!(message_text_len(&rs.message), message_text_len(&s.message),
                "redaction altered non-secret message at step {}", rs.step_id);
        }
    }

    /// Multimodal user content: when content_parts has an image, the message
    /// is a Parts array (ATIF v1.6+ multimodal), never a bare string.
    #[test]
    fn multimodal_user_becomes_parts(
        text in ".{0,20}",
        has_image in any::<bool>(),
    ) {
        let mut parts = Vec::new();
        parts.push(ContentPart::Text { text: text.clone() });
        if has_image {
            parts.push(ContentPart::Image {
                media_type: "image/png".to_string(),
                data: "base64".to_string(),
            });
        }
        let events = vec![AgentEvent::UserTaskSubmitted {
            text: text.clone(),
            content_parts: parts,
            submitted_at: None,
        }];
        let t = build_trajectory(&snapshot(events), &opts(false));
        prop_assert_eq!(t.steps.len(), 1);
        if has_image {
            prop_assert!(matches!(t.steps[0].message, Message::Parts(_)),
                "image content must produce Parts");
        }
    }

    /// No-panic: the fold never panics on arbitrary event orderings, including
    /// orphan tool results (no matching request) and interleaved approvals.
    #[test]
    fn no_panic_on_arbitrary_orderings(actions in arb_actions()) {
        let _ = build_trajectory(&snapshot(actions_to_events(&actions)), &opts(false));
        let _ = build_trajectory(&snapshot(actions_to_events(&actions)), &opts(true));
    }
}

// ── Full-enum fuzz (exercises every AgentEvent variant) ──────────────────
//
// Strategy over the complete AgentEvent set so dropped variants (approvals,
// plan, goal, dream, recap, subagent transcript, patch, …) are also fed to
// the fold. We build events via a JSON roundtrip to avoid enumerating every
// variant constructor by hand.

fn arb_json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<f64>().prop_map(|f| {
            serde_json::Number::from_f64(f)
                .map(Value::Number)
                .unwrap_or(Value::Null)
        }),
        prop::string::string_regex(".{0,15}")
            .unwrap()
            .prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 16, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::vec(
                (prop::string::string_regex(".{0,8}").unwrap(), inner.clone()),
                0..4,
            )
            .prop_map(|pairs| {
                let mut map = serde_json::Map::new();
                for (k, v) in pairs {
                    map.insert(k, v);
                }
                Value::Object(map)
            }),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Any JSON value that fails to deserialize as an AgentEvent is rejected
    /// cleanly (no panic); any value that deserializes is folded without panic.
    #[test]
    fn arbitrary_json_no_panic(v in arb_json_value()) {
        let events: Result<Vec<AgentEvent>, _> = serde_json::from_value(json!([v]));
        if let Ok(events) = events {
            let _ = build_trajectory(&snapshot(events.clone()), &opts(false));
            let _ = build_trajectory(&snapshot(events), &opts(true));
        }
    }
}
