//! Trace-driven skill mining.

use crate::trace::{TurnOutcome, TurnTrace};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDraft {
    pub name: String,
    pub trigger: String,
    pub workflow: Vec<String>,
    pub required_tools: Vec<String>,
    pub required_capabilities: Vec<String>,
    pub verification_steps: Vec<String>,
    pub examples: Vec<String>,
    pub activated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillReplayReport {
    pub passed: bool,
    pub replay_pass_rate: f64,
    pub reason: String,
}

pub fn draft_skill_from_traces(traces: &[TurnTrace], min_repetitions: usize) -> Option<SkillDraft> {
    if traces.len() < min_repetitions {
        return None;
    }
    let successful = traces
        .iter()
        .filter(|trace| {
            matches!(
                trace.outcome,
                TurnOutcome::Success | TurnOutcome::PartialSuccess
            )
        })
        .collect::<Vec<_>>();
    if successful.len() < min_repetitions {
        return None;
    }
    let first_tools = successful
        .first()?
        .tool_calls
        .iter()
        .map(|call| call.invocation.tool_name.clone())
        .collect::<Vec<_>>();
    if first_tools.is_empty() {
        return None;
    }
    let repeated = successful.iter().all(|trace| {
        trace
            .tool_calls
            .iter()
            .map(|call| call.invocation.tool_name.clone())
            .collect::<Vec<_>>()
            == first_tools
    });
    if !repeated {
        return None;
    }

    let verification_steps = successful
        .iter()
        .flat_map(|trace| {
            trace
                .verifier_results
                .iter()
                .filter(|verifier| verifier.passed)
        })
        .map(|verifier| verifier.command.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if verification_steps.is_empty() {
        return None;
    }
    let required_capabilities = successful
        .iter()
        .flat_map(|trace| trace.capabilities.iter())
        .map(|entry| entry.capability.as_key())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    Some(SkillDraft {
        name: sanitize_name(&successful[0].task),
        trigger: successful[0].task.clone(),
        workflow: first_tools.clone(),
        required_tools: first_tools,
        required_capabilities,
        verification_steps,
        examples: successful.iter().map(|trace| trace.task.clone()).collect(),
        activated: false,
    })
}

pub fn activate_skill_after_replay(
    mut draft: SkillDraft,
    report: &SkillReplayReport,
    threshold: f64,
) -> SkillDraft {
    draft.activated = report.passed && report.replay_pass_rate >= threshold;
    draft
}

fn sanitize_name(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{ToolInvocation, ToolResult};

    fn trace(id: &str) -> TurnTrace {
        let mut trace = TurnTrace::new(id, "s", "p", "m", "Fix repeated task");
        trace.record_capability(crate::capability::CapabilityLedgerEntry {
            capability: crate::capability::Capability::RepoRead,
            scope: crate::capability::CapabilityScope::Turn(id.to_string()),
            decision: crate::capability::CapabilityDecision::Consumed,
            at_ms: 1,
            justification: "read source".to_string(),
        });
        trace.record_tool_call(
            &ToolInvocation {
                id: "read".to_string(),
                tool_name: "read_file".to_string(),
                input: serde_json::json!({"path": "src/lib.rs"}),
            },
            &ToolResult {
                invocation_id: "read".to_string(),
                ok: true,
                output: serde_json::json!({}),
            },
            1,
        );
        trace.record_verifier("test", "just test-crate navi-core", true, 1, Some(0));
        trace
    }

    #[test]
    fn repeated_successful_traces_create_skill_draft() {
        let draft = draft_skill_from_traces(&[trace("a"), trace("b")], 2).unwrap();

        assert_eq!(draft.required_tools, vec!["read_file"]);
        assert_eq!(draft.required_capabilities, vec!["repo.read"]);
        assert!(!draft.activated);
    }

    #[test]
    fn bad_replay_does_not_activate_skill() {
        let draft = draft_skill_from_traces(&[trace("a"), trace("b")], 2).unwrap();
        let activated = activate_skill_after_replay(
            draft,
            &SkillReplayReport {
                passed: false,
                replay_pass_rate: 0.0,
                reason: "failed".to_string(),
            },
            0.8,
        );

        assert!(!activated.activated);
    }
}
