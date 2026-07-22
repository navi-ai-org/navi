//! Run / agent policy intersection (never widens past run policy).

use serde::{Deserialize, Serialize};

use super::types::NESTED_WORKFLOW_TOOLS;

/// Hard ceiling for `max_parallel` (settings/tool input cannot exceed).
pub const MAX_PARALLEL_CEILING: usize = 64;
/// Hard ceiling for `max_agents`.
pub const MAX_AGENTS_CEILING: usize = 5000;

/// Default read-oriented tool set for explorer runs.
pub const DEFAULT_READ_TOOLS: &[&str] = &[
    "read_file",
    "read",
    "search",
    "grep",
    "fs_browser",
    "list_dir",
    "glob",
    "view_file",
];

/// Run-level permission envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPolicy {
    pub profile: String,
    pub approval: String,
    pub tools: Vec<String>,
    pub path_allow: Vec<String>,
    pub path_deny: Vec<String>,
    pub create_files: bool,
    pub create_dirs: bool,
    pub write_allow: Vec<String>,
}

/// Per-`agent()` option overrides (all optional).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPolicyOpts {
    pub profile: Option<String>,
    pub tools: Option<Vec<String>>,
    pub approval: Option<String>,
    pub path_allow: Option<Vec<String>>,
    pub path_deny: Option<Vec<String>>,
    pub create_files: Option<bool>,
    pub create_dirs: Option<bool>,
    pub write_allow: Option<Vec<String>>,
    pub model: Option<String>,
    pub max_tokens: Option<usize>,
    pub label: Option<String>,
}

/// Effective policy after intersection + nested-tool strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveAgentPolicy {
    pub profile: String,
    pub approval: String,
    pub tools: Vec<String>,
    pub path_allow: Vec<String>,
    pub path_deny: Vec<String>,
    pub create_files: bool,
    pub create_dirs: bool,
    pub write_allow: Vec<String>,
}

impl EffectiveAgentPolicy {
    /// True when writes are possible under this policy.
    pub fn allows_any_write(&self) -> bool {
        self.create_files || self.create_dirs || !self.write_allow.is_empty()
    }

    /// Whether a write to `path` is allowed (write_allow non-empty and matches).
    pub fn allows_write_path(&self, path: &str) -> bool {
        if self.write_allow.is_empty() {
            return false;
        }
        if self.path_deny.iter().any(|d| path_matches(d, path)) {
            return false;
        }
        self.write_allow.iter().any(|a| path_matches(a, path))
    }

    /// Whether creating a new file is allowed.
    pub fn allows_create_file(&self, path: &str) -> bool {
        self.create_files && self.allows_write_path(path)
    }
}

/// Default run policy: read-only explorer.
pub fn default_run_policy() -> RunPolicy {
    RunPolicy {
        profile: "explorer".into(),
        approval: "read_only".into(),
        tools: DEFAULT_READ_TOOLS
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
        path_allow: vec!["**".into()],
        path_deny: vec![".git".into(), ".git/**".into(), "**/.git/**".into()],
        create_files: false,
        create_dirs: false,
        write_allow: vec![],
    }
}

pub fn clamp_max_parallel(value: usize) -> usize {
    value.clamp(1, MAX_PARALLEL_CEILING)
}

pub fn clamp_max_agents(value: usize) -> usize {
    value.clamp(1, MAX_AGENTS_CEILING)
}

/// Intersect agent opts with run policy (AND / set intersection). Never widens.
pub fn intersect_agent_policy(run: &RunPolicy, opts: &AgentPolicyOpts) -> EffectiveAgentPolicy {
    let profile = opts.profile.clone().unwrap_or_else(|| run.profile.clone());

    // Tools: opts ∩ run, then strip orchestration.
    let base_tools = opts.tools.clone().unwrap_or_else(|| run.tools.clone());
    let run_set: std::collections::BTreeSet<_> = run.tools.iter().cloned().collect();
    let mut tools: Vec<String> = base_tools
        .into_iter()
        .filter(|t| run_set.contains(t))
        .collect();
    // If opts omitted tools, keep run tools (already in base_tools).
    // Always strip nested orchestration.
    tools.retain(|t| !NESTED_WORKFLOW_TOOLS.contains(&t.as_str()));
    tools.sort();
    tools.dedup();

    // path_allow: intersection when both set; empty opts → run.
    let path_allow = match &opts.path_allow {
        Some(extra) if !extra.is_empty() => intersect_paths(&run.path_allow, extra),
        _ => run.path_allow.clone(),
    };

    // path_deny: union (deny wins).
    let mut path_deny = run.path_deny.clone();
    if let Some(extra) = &opts.path_deny {
        path_deny.extend(extra.iter().cloned());
    }
    path_deny.sort();
    path_deny.dedup();

    // Non-widening flags: omit on agent ⇒ inherit run; agent cannot enable if run forbids.
    let create_files = opts.create_files.unwrap_or(run.create_files) && run.create_files;
    let create_dirs = opts.create_dirs.unwrap_or(run.create_dirs) && run.create_dirs;

    // write_allow: intersection. Empty opts or empty run ⇒ no writes.
    let write_allow = match &opts.write_allow {
        Some(extra) => {
            if run.write_allow.is_empty() {
                // Run forbids writes unless run write_allow was explicitly open.
                // Spec: empty write_allow on run ⇒ no writes. Intersect with empty = empty.
                // Exception: if run has empty write_allow, still empty.
                Vec::new()
            } else {
                intersect_paths(&run.write_allow, extra)
            }
        }
        None => {
            // No per-agent write_allow → inherit run (often empty).
            run.write_allow.clone()
        }
    };

    // Spec: empty write_allow ⇒ no writes even if profile is implementer.
    // Profile may be implementer but create/write stay false without write_allow.

    let approval = opts
        .approval
        .clone()
        .unwrap_or_else(|| run.approval.clone());

    EffectiveAgentPolicy {
        profile,
        approval,
        tools,
        path_allow,
        path_deny,
        create_files,
        create_dirs,
        write_allow,
    }
}

fn intersect_paths(a: &[String], b: &[String]) -> Vec<String> {
    // If either side is a universal allow, return the other.
    let a_univ = a.iter().any(|p| p == "**" || p == "*" || p == ".");
    let b_univ = b.iter().any(|p| p == "**" || p == "*" || p == ".");
    if a_univ {
        return b.to_vec();
    }
    if b_univ {
        return a.to_vec();
    }
    let set: std::collections::BTreeSet<_> = a.iter().cloned().collect();
    b.iter().filter(|p| set.contains(*p)).cloned().collect()
}

fn path_matches(pattern: &str, path: &str) -> bool {
    if pattern == "**" || pattern == "*" {
        return true;
    }
    if pattern.ends_with("/**") {
        let prefix = &pattern[..pattern.len() - 3];
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }
    if let Some(prefix) = pattern.strip_suffix("/*") {
        if path == prefix {
            return true;
        }
        if let Some(rest) = path.strip_prefix(&format!("{prefix}/")) {
            return !rest.contains('/');
        }
        return false;
    }
    pattern == path || path.starts_with(&format!("{pattern}/"))
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    #[test]
    fn p6_opts_cannot_add_tools_outside_run() {
        let run = default_run_policy();
        let opts = AgentPolicyOpts {
            tools: Some(vec!["read_file".into(), "bash".into(), "write_file".into()]),
            ..Default::default()
        };
        let eff = intersect_agent_policy(&run, &opts);
        assert!(eff.tools.contains(&"read_file".into()));
        assert!(!eff.tools.iter().any(|t| t == "bash"));
        assert!(!eff.tools.iter().any(|t| t == "write_file"));
    }

    #[test]
    fn p7_implementer_empty_write_allow_no_writes() {
        let mut run = default_run_policy();
        run.profile = "implementer".into();
        // Even if run allowed writes, empty agent write_allow inherits run empty.
        let opts = AgentPolicyOpts {
            profile: Some("implementer".into()),
            write_allow: Some(vec![]),
            create_files: Some(true),
            ..Default::default()
        };
        let eff = intersect_agent_policy(&run, &opts);
        assert!(eff.write_allow.is_empty());
        assert!(!eff.create_files); // AND with run.create_files=false
        assert!(!eff.allows_any_write());
    }

    #[test]
    fn write_allow_intersection_and_path_deny() {
        let mut run = default_run_policy();
        run.create_files = true;
        run.write_allow = vec!["src/a.rs".into(), "src/b.rs".into()];
        let opts = AgentPolicyOpts {
            write_allow: Some(vec!["src/a.rs".into(), "src/c.rs".into()]),
            path_deny: Some(vec!["src/a.rs".into()]),
            create_files: Some(true),
            ..Default::default()
        };
        let eff = intersect_agent_policy(&run, &opts);
        assert_eq!(eff.write_allow, vec!["src/a.rs".to_string()]);
        assert!(!eff.allows_write_path("src/a.rs")); // deny wins
        assert!(!eff.allows_write_path("src/c.rs"));
    }

    #[test]
    fn create_files_inherits_from_run_when_agent_omits() {
        let mut run = default_run_policy();
        run.create_files = true;
        run.create_dirs = true;
        run.write_allow = vec!["scratch/probe.txt".into()];
        // Agent does not set create_files/create_dirs — must inherit run flags.
        let opts = AgentPolicyOpts {
            write_allow: Some(vec!["scratch/probe.txt".into()]),
            ..Default::default()
        };
        let eff = intersect_agent_policy(&run, &opts);
        assert!(eff.create_files, "run create_files=true should inherit");
        assert!(eff.create_dirs, "run create_dirs=true should inherit");
        assert_eq!(eff.write_allow, vec!["scratch/probe.txt".to_string()]);
    }

    #[test]
    fn create_files_agent_cannot_widen_past_run() {
        let mut run = default_run_policy();
        run.create_files = false;
        run.write_allow = vec!["scratch/probe.txt".into()];
        let opts = AgentPolicyOpts {
            create_files: Some(true),
            write_allow: Some(vec!["scratch/probe.txt".into()]),
            ..Default::default()
        };
        let eff = intersect_agent_policy(&run, &opts);
        assert!(!eff.create_files, "agent must not widen create_files");
    }

    #[test]
    fn nested_tools_always_stripped() {
        let mut run = default_run_policy();
        run.tools.push("subagent".into());
        run.tools.push("workflow".into());
        let opts = AgentPolicyOpts {
            tools: Some(vec![
                "read_file".into(),
                "subagent".into(),
                "workflow".into(),
            ]),
            ..Default::default()
        };
        let eff = intersect_agent_policy(&run, &opts);
        assert!(!eff.tools.iter().any(|t| t == "subagent"));
        assert!(!eff.tools.iter().any(|t| t == "workflow"));
    }

    #[test]
    fn clamp_ceilings() {
        assert_eq!(clamp_max_parallel(0), 1);
        assert_eq!(clamp_max_parallel(16), 16);
        assert_eq!(clamp_max_parallel(100), MAX_PARALLEL_CEILING);
        assert_eq!(clamp_max_agents(0), 1);
        assert_eq!(clamp_max_agents(1000), 1000);
        assert_eq!(clamp_max_agents(99999), MAX_AGENTS_CEILING);
    }
}
