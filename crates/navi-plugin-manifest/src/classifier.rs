use crate::risk::{RiskAssessment, RiskLevel};
use crate::types::{Capability, FsAccess, ToolDef};

/// Classify the risk of a tool based on its capabilities.
/// This is a pure function: input capabilities → output risk level + warning.
pub fn classify_tool_risk(tool: &ToolDef, capabilities: &[Capability]) -> RiskAssessment {
    let tool_caps: Vec<&Capability> = capabilities
        .iter()
        .filter(|c| tool.capabilities.contains(&c.id().to_string()))
        .collect();

    if tool_caps.is_empty() {
        return RiskAssessment {
            level: RiskLevel::Low,
            warning: None,
            single_risks: Vec::new(),
            compound_risks: Vec::new(),
        };
    }

    let mut single_risks = Vec::new();
    let mut max_single = RiskLevel::Low;

    for cap in &tool_caps {
        let risk = single_cap_risk(cap);
        single_risks.push((cap.id().to_string(), risk));
        if risk > max_single {
            max_single = risk;
        }
    }

    let mut compound_risks = Vec::new();
    let mut max_compound = RiskLevel::Low;
    let mut compound_warning: Option<String> = None;

    // Check all pairs
    for i in 0..tool_caps.len() {
        for j in (i + 1)..tool_caps.len() {
            if let Some((risk, warning)) = compound_pair_risk(tool_caps[i], tool_caps[j]) {
                let label = format!("{}+{}", tool_caps[i].id(), tool_caps[j].id());
                compound_risks.push((label, risk));
                if risk > max_compound {
                    max_compound = risk;
                    compound_warning = Some(warning);
                }
            }
        }
    }

    // Check all triples
    for i in 0..tool_caps.len() {
        for j in (i + 1)..tool_caps.len() {
            for k in (j + 1)..tool_caps.len() {
                if let Some((risk, warning)) =
                    compound_triple_risk(tool_caps[i], tool_caps[j], tool_caps[k])
                {
                    let label = format!(
                        "{}+{}+{}",
                        tool_caps[i].id(),
                        tool_caps[j].id(),
                        tool_caps[k].id()
                    );
                    compound_risks.push((label, risk));
                    if risk > max_compound {
                        max_compound = risk;
                        compound_warning = Some(warning);
                    }
                }
            }
        }
    }

    let final_level = max_single.max(max_compound);
    let warning = if final_level >= RiskLevel::High {
        compound_warning.or_else(|| single_warning(final_level))
    } else {
        None
    };

    RiskAssessment {
        level: final_level,
        warning,
        single_risks,
        compound_risks,
    }
}

/// Risk of a single capability in isolation.
fn single_cap_risk(cap: &Capability) -> RiskLevel {
    match cap {
        Capability::Filesystem { access, .. } => match access {
            FsAccess::ReadOnly => RiskLevel::Medium,
            FsAccess::ReadWrite => RiskLevel::High,
        },
        Capability::Network { hosts, methods, .. } => {
            if hosts.iter().any(|h| h == "*") {
                RiskLevel::Critical
            } else if methods.iter().any(|m| m.eq_ignore_ascii_case("POST")) {
                RiskLevel::High
            } else {
                RiskLevel::Medium
            }
        }
        Capability::Tui { .. } => RiskLevel::Low,
    }
}

/// Check compound risk for a pair of capabilities.
fn compound_pair_risk(cap_a: &Capability, cap_b: &Capability) -> Option<(RiskLevel, String)> {
    let has_fs_read = is_fs_read(cap_a) || is_fs_read(cap_b);
    let has_fs_write = is_fs_write(cap_a) || is_fs_write(cap_b);
    let has_network = is_network(cap_a) || is_network(cap_b);
    let has_network_post = is_network_post(cap_a) || is_network_post(cap_b);
    let has_network_wildcard = is_network_wildcard(cap_a) || is_network_wildcard(cap_b);
    let has_auth = has_auth_binding(cap_a) || has_auth_binding(cap_b);

    // fs_read + network_wildcard = FORBIDDEN
    if has_fs_read && has_network_wildcard {
        return Some((
            RiskLevel::Forbidden,
            "Community plugins cannot combine read access with wildcard network.".into(),
        ));
    }

    // fs_read + network_POST = CRITICAL
    if has_fs_read && has_network_post {
        return Some((
            RiskLevel::Critical,
            "CRITICAL: This tool can read project files and POST data to external servers. High risk of data exfiltration.".into(),
        ));
    }

    // write + network = CRITICAL
    if has_fs_write && has_network {
        return Some((
            RiskLevel::Critical,
            "CRITICAL: This tool can write files and access the network. Could write malicious content.".into(),
        ));
    }

    // fs_read + auth_binding = HIGH
    if has_fs_read && has_auth {
        return Some((
            RiskLevel::High,
            "This tool can read project files and has authenticated access to external services."
                .into(),
        ));
    }

    // fs_read + network_GET = HIGH
    if has_fs_read && has_network {
        return Some((
            RiskLevel::High,
            "This tool can read project files and send data to external servers. This enables data exfiltration.".into(),
        ));
    }

    None
}

/// Check compound risk for a triple of capabilities.
fn compound_triple_risk(
    cap_a: &Capability,
    cap_b: &Capability,
    cap_c: &Capability,
) -> Option<(RiskLevel, String)> {
    let has_fs_read = is_fs_read(cap_a) || is_fs_read(cap_b) || is_fs_read(cap_c);
    let has_network_post =
        is_network_post(cap_a) || is_network_post(cap_b) || is_network_post(cap_c);
    let has_auth = has_auth_binding(cap_a) || has_auth_binding(cap_b) || has_auth_binding(cap_c);

    // fs_read + auth_binding + POST = CRITICAL
    if has_fs_read && has_auth && has_network_post {
        return Some((
            RiskLevel::Critical,
            "CRITICAL: This tool can read files, authenticate to services, and send data. Very high exfiltration risk.".into(),
        ));
    }

    None
}

fn is_fs_read(cap: &Capability) -> bool {
    matches!(
        cap,
        Capability::Filesystem {
            access: FsAccess::ReadOnly,
            ..
        }
    )
}

fn is_fs_write(cap: &Capability) -> bool {
    matches!(
        cap,
        Capability::Filesystem {
            access: FsAccess::ReadWrite,
            ..
        }
    )
}

fn is_network(cap: &Capability) -> bool {
    matches!(cap, Capability::Network { .. })
}

fn is_network_post(cap: &Capability) -> bool {
    matches!(cap, Capability::Network { methods, .. }
        if methods.iter().any(|m| m.eq_ignore_ascii_case("POST")))
}

fn is_network_wildcard(cap: &Capability) -> bool {
    matches!(cap, Capability::Network { hosts, .. }
        if hosts.iter().any(|h| h == "*"))
}

fn has_auth_binding(cap: &Capability) -> bool {
    matches!(cap, Capability::Network { auth: Some(_), .. })
}

fn single_warning(level: RiskLevel) -> Option<String> {
    match level {
        RiskLevel::High => Some("This tool has HIGH risk capabilities.".into()),
        RiskLevel::Critical => Some("CRITICAL: This tool has critical risk capabilities.".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AuthBinding, FsScope, ToolRisk};

    fn fs_read_cap(id: &str) -> Capability {
        Capability::Filesystem {
            id: id.into(),
            scope: FsScope::Project,
            access: FsAccess::ReadOnly,
            paths: vec!["src/".into()],
            reason: "Read source.".into(),
        }
    }

    fn fs_write_cap(id: &str) -> Capability {
        Capability::Filesystem {
            id: id.into(),
            scope: FsScope::Project,
            access: FsAccess::ReadWrite,
            paths: vec!["src/".into()],
            reason: "Write source.".into(),
        }
    }

    fn network_get_cap(id: &str) -> Capability {
        Capability::Network {
            id: id.into(),
            hosts: vec!["api.example.com".into()],
            methods: vec!["GET".into()],
            https_only: true,
            reason: "API access.".into(),
            auth: None,
        }
    }

    fn network_post_cap(id: &str) -> Capability {
        Capability::Network {
            id: id.into(),
            hosts: vec!["api.example.com".into()],
            methods: vec!["GET".into(), "POST".into()],
            https_only: true,
            reason: "API access.".into(),
            auth: None,
        }
    }

    fn network_wildcard_cap(id: &str) -> Capability {
        Capability::Network {
            id: id.into(),
            hosts: vec!["*".into()],
            methods: vec!["GET".into(), "POST".into()],
            https_only: true,
            reason: "Wildcard access.".into(),
            auth: None,
        }
    }

    fn network_auth_cap(id: &str) -> Capability {
        Capability::Network {
            id: id.into(),
            hosts: vec!["api.example.com".into()],
            methods: vec!["GET".into()],
            https_only: true,
            reason: "Authenticated API.".into(),
            auth: Some(AuthBinding {
                binding: "API_KEY".into(),
                inject_as: "Authorization: Bearer {secret}".into(),
            }),
        }
    }

    fn tool(id: &str, cap_ids: &[&str]) -> ToolDef {
        ToolDef {
            id: id.into(),
            summary: "Test tool.".into(),
            risk: ToolRisk::ReadOnly,
            input_schema: None,
            capabilities: cap_ids.iter().map(|s| s.to_string()).collect(),
        }
    }

    // Single capability risks

    #[test]
    fn empty_caps_is_low() {
        let t = tool("t", &[]);
        let a = classify_tool_risk(&t, &[]);
        assert_eq!(a.level, RiskLevel::Low);
    }

    #[test]
    fn fs_read_only_is_medium() {
        let caps = [fs_read_cap("fs")];
        let t = tool("t", &["fs"]);
        let a = classify_tool_risk(&t, &caps);
        assert_eq!(a.level, RiskLevel::Medium);
    }

    #[test]
    fn fs_write_is_high() {
        let caps = [fs_write_cap("fs")];
        let t = tool("t", &["fs"]);
        let a = classify_tool_risk(&t, &caps);
        assert_eq!(a.level, RiskLevel::High);
    }

    #[test]
    fn network_get_is_medium() {
        let caps = [network_get_cap("net")];
        let t = tool("t", &["net"]);
        let a = classify_tool_risk(&t, &caps);
        assert_eq!(a.level, RiskLevel::Medium);
    }

    #[test]
    fn network_post_is_high() {
        let caps = [network_post_cap("net")];
        let t = tool("t", &["net"]);
        let a = classify_tool_risk(&t, &caps);
        assert_eq!(a.level, RiskLevel::High);
    }

    #[test]
    fn network_wildcard_is_critical() {
        let caps = [network_wildcard_cap("net")];
        let t = tool("t", &["net"]);
        let a = classify_tool_risk(&t, &caps);
        assert_eq!(a.level, RiskLevel::Critical);
    }

    // Compound risks

    #[test]
    fn fs_read_plus_network_get_is_high() {
        let caps = [fs_read_cap("fs"), network_get_cap("net")];
        let t = tool("t", &["fs", "net"]);
        let a = classify_tool_risk(&t, &caps);
        assert_eq!(a.level, RiskLevel::High);
        assert!(a.warning.is_some());
    }

    #[test]
    fn fs_read_plus_network_post_is_critical() {
        let caps = [fs_read_cap("fs"), network_post_cap("net")];
        let t = tool("t", &["fs", "net"]);
        let a = classify_tool_risk(&t, &caps);
        assert_eq!(a.level, RiskLevel::Critical);
        assert!(a.warning.unwrap().contains("CRITICAL"));
    }

    #[test]
    fn fs_read_plus_auth_is_high() {
        let caps = [fs_read_cap("fs"), network_auth_cap("net")];
        let t = tool("t", &["fs", "net"]);
        let a = classify_tool_risk(&t, &caps);
        assert_eq!(a.level, RiskLevel::High);
        assert!(a.warning.unwrap().contains("authenticated"));
    }

    #[test]
    fn fs_read_plus_auth_plus_post_is_critical() {
        let caps = [
            fs_read_cap("fs"),
            Capability::Network {
                id: "net".into(),
                hosts: vec!["api.example.com".into()],
                methods: vec!["GET".into(), "POST".into()],
                https_only: true,
                reason: "API.".into(),
                auth: Some(AuthBinding {
                    binding: "KEY".into(),
                    inject_as: "Authorization: Bearer {secret}".into(),
                }),
            },
        ];
        let t = tool("t", &["fs", "net"]);
        let a = classify_tool_risk(&t, &caps);
        assert_eq!(a.level, RiskLevel::Critical);
        // Pair rule (fs_read + network_POST) fires at CRITICAL before triple
        assert!(a.warning.is_some());
    }

    #[test]
    fn write_plus_network_is_critical() {
        let caps = [fs_write_cap("fs"), network_get_cap("net")];
        let t = tool("t", &["fs", "net"]);
        let a = classify_tool_risk(&t, &caps);
        assert_eq!(a.level, RiskLevel::Critical);
        assert!(
            a.warning
                .unwrap()
                .contains("write files and access the network")
        );
    }

    #[test]
    fn fs_read_plus_network_wildcard_is_forbidden() {
        let caps = [fs_read_cap("fs"), network_wildcard_cap("net")];
        let t = tool("t", &["fs", "net"]);
        let a = classify_tool_risk(&t, &caps);
        assert_eq!(a.level, RiskLevel::Forbidden);
    }

    // Per-tool isolation

    #[test]
    fn separate_tools_do_not_elevate_each_other() {
        let caps = [fs_read_cap("fs"), network_post_cap("net")];
        let t1 = tool("search", &["fs"]);
        let t2 = tool("post", &["net"]);

        let a1 = classify_tool_risk(&t1, &caps);
        let a2 = classify_tool_risk(&t2, &caps);

        // search is only MEDIUM (fs_read alone)
        assert_eq!(a1.level, RiskLevel::Medium);
        // post is only HIGH (network_post alone)
        assert_eq!(a2.level, RiskLevel::High);
        // Neither is CRITICAL (that would require them combined)
    }

    #[test]
    fn combined_tool_is_critical() {
        let caps = [fs_read_cap("fs"), network_post_cap("net")];
        let t = tool("check_config", &["fs", "net"]);
        let a = classify_tool_risk(&t, &caps);
        assert_eq!(a.level, RiskLevel::Critical);
    }

    // Edge cases

    #[test]
    fn duplicate_caps_do_not_inflate() {
        let caps = [fs_read_cap("fs"), fs_read_cap("fs")];
        let t = tool("t", &["fs"]);
        let a = classify_tool_risk(&t, &caps);
        assert_eq!(a.level, RiskLevel::Medium);
    }

    #[test]
    fn unknown_cap_not_in_caps_list() {
        let caps = [fs_read_cap("fs")];
        let t = tool("t", &["fs", "nonexistent"]);
        let a = classify_tool_risk(&t, &caps);
        // Only fs is found, nonexistent is ignored (not in caps list)
        assert_eq!(a.level, RiskLevel::Medium);
    }

    #[test]
    fn risk_level_ordering() {
        assert!(RiskLevel::Low < RiskLevel::Medium);
        assert!(RiskLevel::Medium < RiskLevel::High);
        assert!(RiskLevel::High < RiskLevel::Critical);
        assert!(RiskLevel::Critical < RiskLevel::Forbidden);
    }

    #[test]
    fn risk_level_display() {
        assert_eq!(RiskLevel::Low.to_string(), "LOW");
        assert_eq!(RiskLevel::Critical.to_string(), "CRITICAL");
    }
}
