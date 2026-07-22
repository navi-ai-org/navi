//! Tool body expand/collapse policy (compact).
//!
//! Useful output (edits/diffs, errors, small structured results) opens by default.
//! Noisy tools (reads, greps, shell dumps) stay collapsed unless the user opens them.
//!
//! `full_tool_view` (Ctrl+O Expand All) never wipes per-tool user intent: forced
//! opens stay open when leaving expand-all, and the currently selected tool is
//! pinned open so Ctrl+O cannot "close what I just opened".

use std::collections::HashSet;

use navi_sdk::{ToolInvocation, ToolResult};

/// Why a tool body is (or isn't) shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolBodyReason {
    /// User forced this tool open.
    UserExpanded,
    /// Policy auto-opens useful content (diffs, errors, …).
    AutoUseful,
    /// Global expand-all (Ctrl+O).
    ExpandAll,
    /// User forced this tool closed (overrides auto / expand-all).
    UserCollapsed,
    /// Default compact one-liner.
    CollapsedDefault,
}

impl ToolBodyReason {
    pub(crate) fn is_visible(self) -> bool {
        matches!(
            self,
            Self::UserExpanded | Self::AutoUseful | Self::ExpandAll
        )
    }
}

/// Returns whether this tool's detailed body should be shown.
pub(crate) fn tool_body_visible(
    invocation: &ToolInvocation,
    result: &ToolResult,
    full_tool_view: bool,
    expanded: &HashSet<String>,
    collapsed: &HashSet<String>,
) -> bool {
    tool_body_reason(invocation, result, full_tool_view, expanded, collapsed).is_visible()
}

pub(crate) fn tool_body_reason(
    invocation: &ToolInvocation,
    result: &ToolResult,
    full_tool_view: bool,
    expanded: &HashSet<String>,
    collapsed: &HashSet<String>,
) -> ToolBodyReason {
    let id = result.invocation_id.as_str();
    if collapsed.contains(id) {
        return ToolBodyReason::UserCollapsed;
    }
    if expanded.contains(id) {
        return ToolBodyReason::UserExpanded;
    }
    if full_tool_view {
        return ToolBodyReason::ExpandAll;
    }
    if tool_auto_expand(invocation, result) {
        return ToolBodyReason::AutoUseful;
    }
    ToolBodyReason::CollapsedDefault
}

/// Tools whose output is high-signal for reading .
pub(crate) fn tool_auto_expand(invocation: &ToolInvocation, result: &ToolResult) -> bool {
    // Failures are always useful.
    if !result.ok {
        return true;
    }

    let name = invocation.tool_name.as_str();
    match name {
        // Edits / patches — show the diff.
        "apply_patch" | "code_edit" => true,
        "write" | "write_file" => true,

        // Questions need the expanded prompt in chat.
        "question" | "request_user_input" | "ask_user_question" => true,
        // Plan create opens a review modal — keep chat to a one-line summary
        // (never dump the full todos JSON into the scrollback).
        "plan" => plan_tool_auto_expand(invocation, result),

        // Small structured confirmations can stay compact unless error.
        // Noisy exploration — collapsed by default.
        "read" | "read_file" | "view_file" | "grep" | "fs_browser" | "search" | "list_dir"
        | "glob" | "bash" | "ast_search" | "symbol_goto" | "symbol_references" | "repo_explore"
        | "history_ops" | "package_manager" | "code" | "code_exec" => false,

        // Subagent header is enough; drill in on click.
        "subagent" => false,

        // Everything else: open only if there is short, meaningful body text.
        _ => short_useful_body(result),
    }
}

/// When to auto-open a `plan` tool body in chat.
///
/// Create/update that open (or just opened) the review modal stay compact.
/// Errors and short progress updates (complete_step) may expand.
fn plan_tool_auto_expand(invocation: &ToolInvocation, result: &ToolResult) -> bool {
    if !result.ok {
        return true;
    }
    let action = invocation
        .input
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match action {
        // Modal is the plan UI — do not also expand raw JSON/todos in chat.
        "create" | "update" => false,
        // Progress updates: compact header is enough (active plan strip shows checklist).
        "complete_step" => false,
        "get" | "list" | "active" => false,
        _ => short_useful_body(result),
    }
}

fn short_useful_body(result: &ToolResult) -> bool {
    let obj = match result.output.as_object() {
        Some(o) => o,
        None => return false,
    };
    // Prefer small textual payloads.
    for key in ["content", "diff", "patch", "message", "summary", "plan"] {
        if let Some(text) = obj.get(key).and_then(|v| v.as_str()) {
            let lines = text.lines().count();
            if (1..40).contains(&lines) {
                return true;
            }
        }
    }
    false
}

/// Toggle one tool's body with force-open / force-close overrides.
///
/// Returns whether the body is visible after the toggle.
pub(crate) fn toggle_tool_body(
    invocation: &ToolInvocation,
    result: &ToolResult,
    full_tool_view: bool,
    expanded: &mut HashSet<String>,
    collapsed: &mut HashSet<String>,
) -> bool {
    let id = result.invocation_id.clone();
    let currently = tool_body_visible(invocation, result, full_tool_view, expanded, collapsed);
    if currently {
        // Force closed (overrides auto + expand-all until toggled again).
        expanded.remove(&id);
        collapsed.insert(id);
        false
    } else {
        collapsed.remove(&id);
        expanded.insert(id);
        true
    }
}

/// Smart Ctrl+O:
/// - Smart → ExpandAll (show every tool body; clear force-collapsed so expand-all wins)
/// - ExpandAll → Smart, **keeping** force-expanded tools and **pinning** `pin_id` open
///
/// This prevents "I opened a tool, hit Ctrl+O, and it closed".
pub(crate) fn toggle_expand_all_mode(
    full_tool_view: &mut bool,
    expanded: &mut HashSet<String>,
    collapsed: &mut HashSet<String>,
    pin_id: Option<&str>,
) -> bool {
    if *full_tool_view {
        *full_tool_view = false;
        // Leaving expand-all: do not wipe expanded. Clear bulk collapses so
        // auto-useful tools can reappear, but pin the active tool open.
        collapsed.clear();
        if let Some(id) = pin_id {
            expanded.insert(id.to_string());
        }
        false
    } else {
        *full_tool_view = true;
        // Expand-all should actually expand everything right now.
        collapsed.clear();
        true
    }
}

/// Extract tool invocation id from the selected chat source, if any.
pub(crate) fn selected_tool_id(source: &crate::state::ChatLineSource) -> Option<&str> {
    match source {
        crate::state::ChatLineSource::ToolResult(id) => Some(id.as_str()),
        crate::state::ChatLineSource::Subagent(id) => Some(id.as_str()),
        crate::state::ChatLineSource::ToolGroup(ids) => ids.first().map(String::as_str),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use navi_sdk::{ToolInvocation, ToolResult};
    use serde_json::json;

    fn inv(name: &str, id: &str) -> ToolInvocation {
        ToolInvocation {
            id: id.to_string(),
            tool_name: name.to_string(),
            input: json!({}),
        }
    }

    fn res(id: &str, ok: bool, output: serde_json::Value) -> ToolResult {
        ToolResult {
            invocation_id: id.to_string(),
            ok,
            output,
        }
    }

    #[test]
    fn patches_auto_expand_reads_do_not() {
        let patch = inv("apply_patch", "p1");
        let patch_res = res("p1", true, json!({"ok": true}));
        assert!(tool_auto_expand(&patch, &patch_res));

        let read = inv("read_file", "r1");
        let read_res = res("r1", true, json!({"path": "a.rs", "content": "x"}));
        assert!(!tool_auto_expand(&read, &read_res));
    }

    #[test]
    fn plan_create_stays_collapsed_questions_expand() {
        let mut plan = inv("plan", "pl1");
        plan.input = json!({ "action": "create", "title": "T" });
        let plan_res = res(
            "pl1",
            true,
            json!({ "title": "T", "needs_review": true, "steps_count": 3 }),
        );
        assert!(!tool_auto_expand(&plan, &plan_res));

        let q = inv("question", "q1");
        let q_res = res("q1", true, json!({ "prompt": "ok?" }));
        assert!(tool_auto_expand(&q, &q_res));
    }

    #[test]
    fn failures_auto_expand() {
        let bash = inv("bash", "b1");
        let fail = res("b1", false, json!({"error": "boom"}));
        assert!(tool_auto_expand(&bash, &fail));
    }

    #[test]
    fn user_collapse_overrides_auto() {
        let patch = inv("apply_patch", "p1");
        let patch_res = res("p1", true, json!({}));
        let expanded = HashSet::new();
        let mut collapsed = HashSet::new();
        assert!(tool_body_visible(
            &patch, &patch_res, false, &expanded, &collapsed
        ));
        collapsed.insert("p1".to_string());
        assert!(!tool_body_visible(
            &patch, &patch_res, false, &expanded, &collapsed
        ));
        // Collapse also wins over expand-all.
        assert!(!tool_body_visible(
            &patch, &patch_res, true, &expanded, &collapsed
        ));
    }

    #[test]
    fn ctrl_o_preserves_pin_when_leaving_expand_all() {
        let mut full = true;
        let mut expanded = HashSet::new();
        let mut collapsed = HashSet::from(["noise".to_string()]);
        let now_full =
            toggle_expand_all_mode(&mut full, &mut expanded, &mut collapsed, Some("open-me"));
        assert!(!now_full);
        assert!(!full);
        assert!(expanded.contains("open-me"));
        assert!(collapsed.is_empty());
    }

    #[test]
    fn toggle_force_closes_auto_tool() {
        let patch = inv("write_file", "w1");
        let patch_res = res("w1", true, json!({}));
        let mut expanded = HashSet::new();
        let mut collapsed = HashSet::new();
        assert!(tool_body_visible(
            &patch, &patch_res, false, &expanded, &collapsed
        ));
        assert!(!toggle_tool_body(
            &patch,
            &patch_res,
            false,
            &mut expanded,
            &mut collapsed
        ));
        assert!(collapsed.contains("w1"));
        assert!(!tool_body_visible(
            &patch, &patch_res, false, &expanded, &collapsed
        ));
    }
}
