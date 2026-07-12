use navi_plugin_manifest::{LockEntry, PluginManifest};
use serde::{Deserialize, Serialize};

/// Severity level for a capability or risk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Low => write!(f, "LOW"),
            Severity::Medium => write!(f, "MEDIUM"),
            Severity::High => write!(f, "HIGH"),
            Severity::Critical => write!(f, "CRITICAL"),
        }
    }
}

/// Result of an install approval check.
#[derive(Debug, Clone)]
pub struct InstallApproval {
    pub plugin_id: String,
    pub plugin_name: String,
    pub version: String,
    pub publisher: String,
    pub capabilities: Vec<CapabilityDisplay>,
    pub tools: Vec<ToolDisplay>,
    pub overall_risk: Severity,
    pub warnings: Vec<String>,
}

/// Display information for a capability.
#[derive(Debug, Clone)]
pub struct CapabilityDisplay {
    pub id: String,
    pub kind: String,
    pub description: String,
    pub severity: Severity,
}

/// Display information for a tool.
#[derive(Debug, Clone)]
pub struct ToolDisplay {
    pub id: String,
    pub summary: String,
    pub risk: Severity,
    pub capabilities: Vec<String>,
}

/// Result of an update reconsent check.
#[derive(Debug, Clone)]
pub struct UpdateReconsent {
    pub plugin_id: String,
    pub old_version: String,
    pub new_version: String,
    pub action: ReconsentAction,
    pub changes: Vec<ChangeEntry>,
    pub warnings: Vec<String>,
}

/// Action to take on an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconsentAction {
    /// Allow without reconsent.
    Allow,
    /// Block until user reconsents.
    RequireReconsent,
    /// Block by default (publisher/key change).
    Block,
}

/// A single change in an update.
#[derive(Debug, Clone)]
pub struct ChangeEntry {
    pub change_type: ChangeType,
    pub description: String,
}

/// Type of change in an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeType {
    CapabilityAdded,
    CapabilityRemoved,
    ToolAdded,
    ToolRemoved,
    ToolRiskIncreased,
    ToolSchemaChanged,
    PublisherChanged,
    SigningKeyChanged,
    CodeChanged,
    MinimumNaviIncreased,
}

/// Prepare an install approval display for a manifest.
pub fn prepare_install_approval(manifest: &PluginManifest) -> InstallApproval {
    let capabilities: Vec<CapabilityDisplay> = manifest
        .capabilities
        .iter()
        .map(|cap| {
            let (kind, description, severity) = match cap {
                navi_plugin_manifest::Capability::Filesystem {
                    scope,
                    access,
                    reason,
                    ..
                } => {
                    let sev = match access {
                        navi_plugin_manifest::FsAccess::ReadOnly => Severity::Medium,
                        navi_plugin_manifest::FsAccess::ReadWrite => Severity::High,
                    };
                    (
                        "filesystem".into(),
                        format!("{} {} ({})", scope_str(scope), access_str(access), reason),
                        sev,
                    )
                }
                navi_plugin_manifest::Capability::Network {
                    hosts,
                    methods,
                    https_only,
                    reason,
                    auth,
                    ..
                } => {
                    let has_post = methods.iter().any(|m| m.eq_ignore_ascii_case("POST"));
                    let has_auth = auth.is_some();
                    let sev = match (has_post, has_auth) {
                        (true, true) => Severity::Critical,
                        (true, false) | (false, true) => Severity::High,
                        (false, false) => Severity::Medium,
                    };
                    let scheme = if *https_only { "https" } else { "http" };
                    (
                        "network".into(),
                        format!(
                            "{}://{} ({}) [{}]",
                            scheme,
                            hosts.join(", "),
                            methods.join(", "),
                            reason
                        ),
                        sev,
                    )
                }
                navi_plugin_manifest::Capability::Tui {
                    components, reason, ..
                } => (
                    "tui".into(),
                    format!("{} ({})", components.join(", "), reason),
                    Severity::Low,
                ),
            };

            CapabilityDisplay {
                id: cap.id().into(),
                kind,
                description,
                severity,
            }
        })
        .collect();

    let tools: Vec<ToolDisplay> = manifest
        .tools
        .iter()
        .map(|tool| {
            let risk = match tool.risk {
                navi_plugin_manifest::ToolRisk::ReadOnly => Severity::Low,
                navi_plugin_manifest::ToolRisk::NetworkRead => Severity::Medium,
                navi_plugin_manifest::ToolRisk::NetworkWrite => Severity::High,
                navi_plugin_manifest::ToolRisk::Write => Severity::High,
            };
            ToolDisplay {
                id: tool.id.clone(),
                summary: tool.summary.clone(),
                risk,
                capabilities: tool.capabilities.clone(),
            }
        })
        .collect();

    let overall_risk = compute_overall_risk(&capabilities, &tools);
    let warnings = compute_warnings(&capabilities, &tools, overall_risk);

    InstallApproval {
        plugin_id: manifest.plugin.id.clone(),
        plugin_name: manifest.plugin.name.clone(),
        version: manifest.plugin.version.clone(),
        publisher: manifest.plugin.publisher.clone(),
        capabilities,
        tools,
        overall_risk,
        warnings,
    }
}

/// Check if an update requires reconsent.
pub fn check_update_reconsent(
    old_entry: &LockEntry,
    new_manifest: &PluginManifest,
    old_manifest: &PluginManifest,
) -> UpdateReconsent {
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut action = ReconsentAction::Allow;

    // Check publisher change
    if old_entry.publisher != new_manifest.plugin.publisher {
        changes.push(ChangeEntry {
            change_type: ChangeType::PublisherChanged,
            description: format!(
                "Publisher changed: {} → {}",
                old_entry.publisher, new_manifest.plugin.publisher
            ),
        });
        action = ReconsentAction::Block;
        warnings.push("Publisher change detected. Update blocked by default.".into());
    }

    // Check capability changes
    let old_cap_ids: std::collections::HashSet<&str> =
        old_manifest.capabilities.iter().map(|c| c.id()).collect();
    let new_cap_ids: std::collections::HashSet<&str> =
        new_manifest.capabilities.iter().map(|c| c.id()).collect();

    for added in new_cap_ids.difference(&old_cap_ids) {
        changes.push(ChangeEntry {
            change_type: ChangeType::CapabilityAdded,
            description: format!("Capability added: {}", added),
        });
        if action != ReconsentAction::Block {
            action = ReconsentAction::RequireReconsent;
        }
    }

    for removed in old_cap_ids.difference(&new_cap_ids) {
        changes.push(ChangeEntry {
            change_type: ChangeType::CapabilityRemoved,
            description: format!("Capability removed: {}", removed),
        });
        // Removing capabilities is always allowed
    }

    // Check tool changes
    let old_tool_ids: std::collections::HashSet<&str> =
        old_manifest.tools.iter().map(|t| t.id.as_str()).collect();
    let new_tool_ids: std::collections::HashSet<&str> =
        new_manifest.tools.iter().map(|t| t.id.as_str()).collect();

    for added in new_tool_ids.difference(&old_tool_ids) {
        changes.push(ChangeEntry {
            change_type: ChangeType::ToolAdded,
            description: format!("Tool added: {}", added),
        });
    }

    for removed in old_tool_ids.difference(&new_tool_ids) {
        changes.push(ChangeEntry {
            change_type: ChangeType::ToolRemoved,
            description: format!("Tool removed: {}", removed),
        });
    }

    // Check tool risk increases
    for new_tool in &new_manifest.tools {
        if let Some(old_tool) = old_manifest.tools.iter().find(|t| t.id == new_tool.id) {
            if tool_risk_score(&new_tool.risk) > tool_risk_score(&old_tool.risk) {
                changes.push(ChangeEntry {
                    change_type: ChangeType::ToolRiskIncreased,
                    description: format!(
                        "Tool '{}' risk increased: {:?} → {:?}",
                        new_tool.id, old_tool.risk, new_tool.risk
                    ),
                });
                if action != ReconsentAction::Block {
                    action = ReconsentAction::RequireReconsent;
                }
            }

            // Check schema changes
            if new_tool.input_schema != old_tool.input_schema {
                changes.push(ChangeEntry {
                    change_type: ChangeType::ToolSchemaChanged,
                    description: format!("Tool '{}' schema changed", new_tool.id),
                });
            }
        }
    }

    // Check minimum_navi increase
    if new_manifest.plugin.minimum_navi > old_manifest.plugin.minimum_navi {
        changes.push(ChangeEntry {
            change_type: ChangeType::MinimumNaviIncreased,
            description: format!(
                "Minimum NAVI version: {} → {}",
                old_manifest.plugin.minimum_navi, new_manifest.plugin.minimum_navi
            ),
        });
        warnings.push(format!(
            "Plugin now requires NAVI {} (was {})",
            new_manifest.plugin.minimum_navi, old_manifest.plugin.minimum_navi
        ));
    }

    // Check version change
    if old_entry.version != new_manifest.plugin.version {
        changes.push(ChangeEntry {
            change_type: ChangeType::CodeChanged,
            description: format!(
                "Version: {} → {}",
                old_entry.version, new_manifest.plugin.version
            ),
        });
    }

    UpdateReconsent {
        plugin_id: new_manifest.plugin.id.clone(),
        old_version: old_entry.version.clone(),
        new_version: new_manifest.plugin.version.clone(),
        action,
        changes,
        warnings,
    }
}

/// Format an install approval as a human-readable string.
pub fn format_install_approval(approval: &InstallApproval) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "Plugin: {} v{}\n",
        approval.plugin_name, approval.version
    ));
    out.push_str(&format!("Publisher: {}\n", approval.publisher));
    out.push_str(&format!("Overall risk: {}\n\n", approval.overall_risk));

    if !approval.capabilities.is_empty() {
        out.push_str("Capabilities:\n");
        for cap in &approval.capabilities {
            let icon = match cap.severity {
                Severity::Low => "  ",
                Severity::Medium => "  ",
                Severity::High => "\u{26a0}",
                Severity::Critical => "\u{1f534}",
            };
            out.push_str(&format!(
                "{} {} {}: {}\n",
                icon, cap.severity, cap.kind, cap.description
            ));
        }
        out.push('\n');
    }

    if !approval.tools.is_empty() {
        out.push_str("Tools:\n");
        for tool in &approval.tools {
            out.push_str(&format!("  {} [{}] {}\n", tool.id, tool.risk, tool.summary));
        }
        out.push('\n');
    }

    if !approval.warnings.is_empty() {
        out.push_str("Warnings:\n");
        for warning in &approval.warnings {
            out.push_str(&format!("  \u{26a0} {}\n", warning));
        }
    }

    out
}

/// Format an update reconsent as a human-readable string.
pub fn format_update_reconsent(reconsent: &UpdateReconsent) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "Plugin: {} ({} → {})\n",
        reconsent.plugin_id, reconsent.old_version, reconsent.new_version
    ));
    out.push_str(&format!("Action: {:?}\n\n", reconsent.action));

    if !reconsent.changes.is_empty() {
        out.push_str("Changes:\n");
        for change in &reconsent.changes {
            let icon = match change.change_type {
                ChangeType::CapabilityAdded => "\u{2795}",
                ChangeType::CapabilityRemoved => "\u{2796}",
                ChangeType::ToolRiskIncreased => "\u{26a0}",
                ChangeType::PublisherChanged => "\u{1f6ab}",
                _ => "\u{2022}",
            };
            out.push_str(&format!("{} {}\n", icon, change.description));
        }
        out.push('\n');
    }

    if !reconsent.warnings.is_empty() {
        out.push_str("Warnings:\n");
        for warning in &reconsent.warnings {
            out.push_str(&format!("  \u{26a0} {}\n", warning));
        }
    }

    out
}

// --- Internal helpers ---

fn compute_overall_risk(capabilities: &[CapabilityDisplay], tools: &[ToolDisplay]) -> Severity {
    let cap_max = capabilities
        .iter()
        .map(|c| c.severity)
        .max()
        .unwrap_or(Severity::Low);
    let tool_max = tools.iter().map(|t| t.risk).max().unwrap_or(Severity::Low);
    cap_max.max(tool_max)
}

fn compute_warnings(
    capabilities: &[CapabilityDisplay],
    _tools: &[ToolDisplay],
    overall_risk: Severity,
) -> Vec<String> {
    let mut warnings = Vec::new();

    // Check for exfiltration risk (fs_read + network)
    let has_fs_read = capabilities
        .iter()
        .any(|c| c.kind == "filesystem" && c.severity <= Severity::Medium);
    let has_network = capabilities.iter().any(|c| c.kind == "network");
    let has_network_post = capabilities.iter().any(|c| c.description.contains("POST"));

    if has_fs_read && has_network_post {
        warnings.push(
            "CRITICAL: This plugin can read project files AND POST data to external servers. \
             High risk of data exfiltration."
                .into(),
        );
    } else if has_fs_read && has_network {
        warnings.push(
            "HIGH RISK: This plugin can read project files and send data to external servers. \
             This combination enables data exfiltration."
                .into(),
        );
    }

    if overall_risk >= Severity::Critical {
        warnings.push(
            "This plugin requires CRITICAL capabilities. Review carefully before approving.".into(),
        );
    }

    warnings
}

fn tool_risk_score(risk: &navi_plugin_manifest::ToolRisk) -> u8 {
    match risk {
        navi_plugin_manifest::ToolRisk::ReadOnly => 1,
        navi_plugin_manifest::ToolRisk::NetworkRead => 2,
        navi_plugin_manifest::ToolRisk::NetworkWrite => 3,
        navi_plugin_manifest::ToolRisk::Write => 3,
    }
}

fn scope_str(scope: &navi_plugin_manifest::FsScope) -> &'static str {
    match scope {
        navi_plugin_manifest::FsScope::Project => "project",
        navi_plugin_manifest::FsScope::Workspace => "workspace",
    }
}

fn access_str(access: &navi_plugin_manifest::FsAccess) -> &'static str {
    match access {
        navi_plugin_manifest::FsAccess::ReadOnly => "read-only",
        navi_plugin_manifest::FsAccess::ReadWrite => "read-write",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use navi_plugin_manifest::*;

    fn minimal_manifest() -> PluginManifest {
        PluginManifest {
            plugin: PluginMeta {
                id: "test".into(),
                name: "Test Plugin".into(),
                version: "1.0.0".into(),
                publisher: "gh:test".into(),
                runtime: RuntimeKind::WasmComponent,
                entry: "plugin.wasm".into(),
                wasm_hash: "sha256:abc".into(),
                signature: "ed25519:abc".into(),
                public_key: None,
                minimum_navi: "0.1.0".into(),
            },
            capabilities: vec![],
            tools: vec![],
        }
    }

    fn manifest_with_fs_read() -> PluginManifest {
        let mut m = minimal_manifest();
        m.capabilities.push(Capability::Filesystem {
            id: "fs_read".into(),
            scope: FsScope::Project,
            access: FsAccess::ReadOnly,
            paths: vec!["src/".into()],
            reason: "Read source.".into(),
        });
        m.tools.push(ToolDef {
            id: "search".into(),
            summary: "Search docs.".into(),
            risk: ToolRisk::ReadOnly,
            input_schema: None,
            capabilities: vec!["fs_read".into()],
        });
        m
    }

    fn manifest_with_network() -> PluginManifest {
        let mut m = minimal_manifest();
        m.capabilities.push(Capability::Network {
            id: "net".into(),
            hosts: vec!["api.example.com".into()],
            methods: vec!["GET".into(), "POST".into()],
            https_only: true,
            reason: "API access.".into(),
            auth: None,
        });
        m.tools.push(ToolDef {
            id: "fetch".into(),
            summary: "Fetch data.".into(),
            risk: ToolRisk::NetworkWrite,
            input_schema: None,
            capabilities: vec!["net".into()],
        });
        m
    }

    fn manifest_with_fs_and_network() -> PluginManifest {
        let mut m = minimal_manifest();
        m.capabilities.push(Capability::Filesystem {
            id: "fs_read".into(),
            scope: FsScope::Project,
            access: FsAccess::ReadOnly,
            paths: vec!["src/".into()],
            reason: "Read source.".into(),
        });
        m.capabilities.push(Capability::Network {
            id: "net".into(),
            hosts: vec!["api.example.com".into()],
            methods: vec!["GET".into(), "POST".into()],
            https_only: true,
            reason: "API access.".into(),
            auth: None,
        });
        m.tools.push(ToolDef {
            id: "check".into(),
            summary: "Check config.".into(),
            risk: ToolRisk::NetworkWrite,
            input_schema: None,
            capabilities: vec!["fs_read".into(), "net".into()],
        });
        m
    }

    #[test]
    fn install_approval_empty_manifest() {
        let m = minimal_manifest();
        let approval = prepare_install_approval(&m);
        assert_eq!(approval.overall_risk, Severity::Low);
        assert!(approval.capabilities.is_empty());
        assert!(approval.tools.is_empty());
        assert!(approval.warnings.is_empty());
    }

    #[test]
    fn install_approval_fs_read() {
        let m = manifest_with_fs_read();
        let approval = prepare_install_approval(&m);
        assert_eq!(approval.overall_risk, Severity::Medium);
        assert_eq!(approval.capabilities.len(), 1);
        assert_eq!(approval.capabilities[0].severity, Severity::Medium);
        assert_eq!(approval.tools.len(), 1);
        assert_eq!(approval.tools[0].risk, Severity::Low);
    }

    #[test]
    fn install_approval_network_post() {
        let m = manifest_with_network();
        let approval = prepare_install_approval(&m);
        assert_eq!(approval.overall_risk, Severity::High);
        assert_eq!(approval.capabilities[0].severity, Severity::High);
    }

    #[test]
    fn install_approval_exfiltration_warning() {
        let m = manifest_with_fs_and_network();
        let approval = prepare_install_approval(&m);
        assert!(!approval.warnings.is_empty());
        assert!(approval.warnings.iter().any(|w| w.contains("exfiltration")));
    }

    #[test]
    fn reconsent_no_changes() {
        let old = LockEntry {
            id: "p".into(),
            version: "1.0.0".into(),
            publisher: "gh:test".into(),
            wasm_hash: "sha256:abc".into(),
            capabilities_hash: "sha256:def".into(),
            tools_hash: "sha256:ghi".into(),
            approved_capabilities: vec![],
            approved_at: "2026-01-01".into(),
            trust_level: navi_plugin_manifest::TrustLevel::Community,
            kind: navi_plugin_manifest::PluginCatalogKind::Plugin,
        };
        let m = minimal_manifest();
        let result = check_update_reconsent(&old, &m, &m);
        assert_eq!(result.action, ReconsentAction::Allow);
    }

    #[test]
    fn reconsent_capability_added() {
        let old = LockEntry {
            id: "p".into(),
            version: "1.0.0".into(),
            publisher: "gh:test".into(),
            wasm_hash: "sha256:abc".into(),
            capabilities_hash: "sha256:def".into(),
            tools_hash: "sha256:ghi".into(),
            approved_capabilities: vec![],
            approved_at: "2026-01-01".into(),
            trust_level: navi_plugin_manifest::TrustLevel::Community,
            kind: navi_plugin_manifest::PluginCatalogKind::Plugin,
        };
        let old_m = minimal_manifest();
        let new_m = manifest_with_fs_read();
        let result = check_update_reconsent(&old, &new_m, &old_m);
        assert_eq!(result.action, ReconsentAction::RequireReconsent);
        assert!(
            result
                .changes
                .iter()
                .any(|c| c.change_type == ChangeType::CapabilityAdded)
        );
    }

    #[test]
    fn reconsent_capability_removed() {
        let old = LockEntry {
            id: "p".into(),
            version: "1.0.0".into(),
            publisher: "gh:test".into(),
            wasm_hash: "sha256:abc".into(),
            capabilities_hash: "sha256:def".into(),
            tools_hash: "sha256:ghi".into(),
            approved_capabilities: vec!["fs_read".into()],
            approved_at: "2026-01-01".into(),
            trust_level: navi_plugin_manifest::TrustLevel::Community,
            kind: navi_plugin_manifest::PluginCatalogKind::Plugin,
        };
        let old_m = manifest_with_fs_read();
        let new_m = minimal_manifest();
        let result = check_update_reconsent(&old, &new_m, &old_m);
        assert_eq!(result.action, ReconsentAction::Allow);
        assert!(
            result
                .changes
                .iter()
                .any(|c| c.change_type == ChangeType::CapabilityRemoved)
        );
    }

    #[test]
    fn reconsent_publisher_change() {
        let old = LockEntry {
            id: "p".into(),
            version: "1.0.0".into(),
            publisher: "gh:old".into(),
            wasm_hash: "sha256:abc".into(),
            capabilities_hash: "sha256:def".into(),
            tools_hash: "sha256:ghi".into(),
            approved_capabilities: vec![],
            approved_at: "2026-01-01".into(),
            trust_level: navi_plugin_manifest::TrustLevel::Community,
            kind: navi_plugin_manifest::PluginCatalogKind::Plugin,
        };
        let old_m = minimal_manifest();
        let mut new_m = minimal_manifest();
        new_m.plugin.publisher = "gh:new".into();
        let result = check_update_reconsent(&old, &new_m, &old_m);
        assert_eq!(result.action, ReconsentAction::Block);
        assert!(
            result
                .changes
                .iter()
                .any(|c| c.change_type == ChangeType::PublisherChanged)
        );
    }

    #[test]
    fn reconsent_risk_increased() {
        let old = LockEntry {
            id: "p".into(),
            version: "1.0.0".into(),
            publisher: "gh:test".into(),
            wasm_hash: "sha256:abc".into(),
            capabilities_hash: "sha256:def".into(),
            tools_hash: "sha256:ghi".into(),
            approved_capabilities: vec![],
            approved_at: "2026-01-01".into(),
            trust_level: navi_plugin_manifest::TrustLevel::Community,
            kind: navi_plugin_manifest::PluginCatalogKind::Plugin,
        };
        let mut old_m = minimal_manifest();
        old_m.tools.push(ToolDef {
            id: "t".into(),
            summary: "T".into(),
            risk: ToolRisk::ReadOnly,
            input_schema: None,
            capabilities: vec![],
        });
        let mut new_m = minimal_manifest();
        new_m.tools.push(ToolDef {
            id: "t".into(),
            summary: "T".into(),
            risk: ToolRisk::NetworkWrite,
            input_schema: None,
            capabilities: vec![],
        });
        let result = check_update_reconsent(&old, &new_m, &old_m);
        assert_eq!(result.action, ReconsentAction::RequireReconsent);
        assert!(
            result
                .changes
                .iter()
                .any(|c| c.change_type == ChangeType::ToolRiskIncreased)
        );
    }

    #[test]
    fn format_install_approval_output() {
        let m = manifest_with_fs_read();
        let approval = prepare_install_approval(&m);
        let output = format_install_approval(&approval);
        assert!(output.contains("Test Plugin"));
        assert!(output.contains("1.0.0"));
        assert!(output.contains("gh:test"));
        assert!(output.contains("filesystem"));
    }

    #[test]
    fn format_update_reconsent_output() {
        let old = LockEntry {
            id: "p".into(),
            version: "1.0.0".into(),
            publisher: "gh:test".into(),
            wasm_hash: "sha256:abc".into(),
            capabilities_hash: "sha256:def".into(),
            tools_hash: "sha256:ghi".into(),
            approved_capabilities: vec![],
            approved_at: "2026-01-01".into(),
            trust_level: navi_plugin_manifest::TrustLevel::Community,
            kind: navi_plugin_manifest::PluginCatalogKind::Plugin,
        };
        let old_m = minimal_manifest();
        let new_m = manifest_with_fs_read();
        let result = check_update_reconsent(&old, &new_m, &old_m);
        let output = format_update_reconsent(&result);
        assert!(output.contains("1.0.0"));
        assert!(output.contains("Capability added"));
    }

    #[test]
    fn severity_ordering() {
        assert!(Severity::Low < Severity::Medium);
        assert!(Severity::Medium < Severity::High);
        assert!(Severity::High < Severity::Critical);
    }

    #[test]
    fn severity_display() {
        assert_eq!(Severity::Low.to_string(), "LOW");
        assert_eq!(Severity::Critical.to_string(), "CRITICAL");
    }
}
