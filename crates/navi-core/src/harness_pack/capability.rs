//! Capability inventory and developer-facing capability card.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Snapshot of what NAVI can expose this session/build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CapabilityInventory {
    /// Direct (always-schema) tool names.
    pub direct_tools: Vec<String>,
    /// Deferred / discoverable tool names (via tool_search).
    pub deferred_tools: Vec<String>,
    /// All registered tool names (union).
    pub all_tools: Vec<String>,
    pub goals_enabled: bool,
    pub browser_available: bool,
    pub max_auto_continue_turns: u32,
    pub plugins: Vec<String>,
    pub mcp_servers: Vec<String>,
    pub harnesses_ready: Vec<String>,
}

/// Build inventory from known tool name lists and feature flags.
///
/// Pure function so tests can fix inputs without a live ToolExecutor.
pub fn inventory_from_tool_names(
    direct: impl IntoIterator<Item = impl Into<String>>,
    deferred: impl IntoIterator<Item = impl Into<String>>,
    goals_enabled: bool,
    browser_available: bool,
    max_auto_continue_turns: u32,
    plugins: impl IntoIterator<Item = impl Into<String>>,
    mcp_servers: impl IntoIterator<Item = impl Into<String>>,
    harnesses_ready: impl IntoIterator<Item = impl Into<String>>,
) -> CapabilityInventory {
    let mut direct_tools: Vec<String> = direct.into_iter().map(Into::into).collect();
    let mut deferred_tools: Vec<String> = deferred.into_iter().map(Into::into).collect();
    direct_tools.sort();
    direct_tools.dedup();
    deferred_tools.sort();
    deferred_tools.dedup();
    let mut all: BTreeSet<String> = direct_tools.iter().cloned().collect();
    for t in &deferred_tools {
        all.insert(t.clone());
    }
    let mut plugins: Vec<String> = plugins.into_iter().map(Into::into).collect();
    plugins.sort();
    plugins.dedup();
    let mut mcp_servers: Vec<String> = mcp_servers.into_iter().map(Into::into).collect();
    mcp_servers.sort();
    mcp_servers.dedup();
    let mut harnesses_ready: Vec<String> = harnesses_ready.into_iter().map(Into::into).collect();
    harnesses_ready.sort();
    harnesses_ready.dedup();
    CapabilityInventory {
        direct_tools,
        deferred_tools,
        all_tools: all.into_iter().collect(),
        goals_enabled,
        browser_available,
        max_auto_continue_turns,
        plugins,
        mcp_servers,
        harnesses_ready,
    }
}

/// Filter tool names to those present in the inventory (case-sensitive match on name).
pub fn filter_tools_to_inventory(
    requested: &[String],
    inventory: &CapabilityInventory,
) -> Vec<String> {
    let set: BTreeSet<&str> = inventory.all_tools.iter().map(|s| s.as_str()).collect();
    let mut out: Vec<String> = requested
        .iter()
        .filter(|t| set.contains(t.as_str()))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Render a short developer-facing capability card (stable for fixed inputs).
pub fn capability_card(inv: &CapabilityInventory) -> String {
    let mut lines = Vec::new();
    lines.push("## NAVI capabilities (this session)".to_string());
    lines.push(format!(
        "- goals: {} | max_auto_continue: {}",
        if inv.goals_enabled {
            "enabled"
        } else {
            "disabled"
        },
        inv.max_auto_continue_turns
    ));
    lines.push(format!(
        "- browser: {}",
        if inv.browser_available {
            "available"
        } else {
            "unavailable"
        }
    ));
    lines.push(format!("- tools.direct: [{}]", inv.direct_tools.join(", ")));
    if !inv.deferred_tools.is_empty() {
        lines.push(format!(
            "- tools.deferred: discover via tool_search ([{}])",
            inv.deferred_tools.join(", ")
        ));
    }
    if !inv.plugins.is_empty() {
        lines.push(format!("- plugins.installed: [{}]", inv.plugins.join(", ")));
    }
    if !inv.mcp_servers.is_empty() {
        lines.push(format!("- mcp.connected: [{}]", inv.mcp_servers.join(", ")));
    }
    if !inv.harnesses_ready.is_empty() {
        lines.push(format!(
            "- harnesses.ready: [{}]",
            inv.harnesses_ready.join(", ")
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CapabilityInventory {
        inventory_from_tool_names(
            ["search", "read_file", "edit", "bash"],
            ["browser", "subagent", "code"],
            true,
            true,
            50,
            ["hello-echo"],
            ["memory"],
            ["design-loop"],
        )
    }

    #[test]
    fn filter_drops_unknown_tools() {
        let inv = sample();
        let filtered = filter_tools_to_inventory(
            &["search".into(), "a11y_audit".into(), "browser".into()],
            &inv,
        );
        assert_eq!(filtered, vec!["browser".to_string(), "search".to_string()]);
    }

    #[test]
    fn card_stable_for_fixed_inputs() {
        let a = capability_card(&sample());
        let b = capability_card(&sample());
        assert_eq!(a, b);
        assert!(a.contains("goals: enabled"));
        assert!(a.contains("browser: available"));
        assert!(a.contains("search"));
        assert!(a.contains("tool_search"));
        assert!(a.contains("design-loop"));
    }
}
