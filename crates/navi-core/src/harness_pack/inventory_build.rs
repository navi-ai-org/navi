//! Build capability inventory from config + tool name lists.

use super::capability::{CapabilityInventory, inventory_from_tool_names};
use super::store::list_harness_ids;
use crate::config::NaviConfig;
use crate::tool::{ToolExposure, ToolMetadata};
use std::path::Path;

/// Partition tool names by exposure and build a full inventory snapshot.
pub fn build_capability_inventory(
    data_dir: &Path,
    config: &NaviConfig,
    tool_meta: &[(String, ToolExposure)],
    browser_available: bool,
    plugins: &[String],
    mcp_servers: &[String],
) -> CapabilityInventory {
    let mut direct = Vec::new();
    let mut deferred = Vec::new();
    for (name, exposure) in tool_meta {
        match exposure {
            ToolExposure::Direct | ToolExposure::ModelOnly => direct.push(name.clone()),
            ToolExposure::Deferred | ToolExposure::Hidden => deferred.push(name.clone()),
            ToolExposure::Internal => {}
        }
    }
    // Hidden aliases stay deferred/hidden for discovery; still list under deferred for filtering.
    let harnesses = list_harness_ids(data_dir).unwrap_or_default();
    inventory_from_tool_names(
        direct,
        deferred,
        config.goals.enabled,
        browser_available,
        config.goals.max_auto_continue_turns,
        plugins.iter().cloned(),
        mcp_servers.iter().cloned(),
        harnesses,
    )
}

/// Convenience: names + metadata from definitions.
pub fn exposure_list_from_metadata(
    items: impl IntoIterator<Item = (String, ToolMetadata)>,
) -> Vec<(String, ToolExposure)> {
    items.into_iter().map(|(n, m)| (n, m.exposure)).collect()
}
