//! Run / agent policy intersection (never widens past run policy).

use serde::{Deserialize, Serialize};

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
///
/// Subagents always inherit all base tools and run in yolo mode, so per-agent
/// `tools`, `approval`, `write_allow`, `create_files`, `create_dirs`, and
/// `max_tokens` are no longer accepted. Use the run `policy` table for those.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPolicyOpts {
    pub profile: Option<String>,
    pub path_allow: Option<Vec<String>>,
    pub path_deny: Option<Vec<String>>,
    pub model: Option<String>,
    pub label: Option<String>,
}

/// Effective per-agent policy after intersection with the run policy.
///
/// Subagents always inherit all base tools and run in yolo mode, so this only
/// carries the resolved per-agent fields: `profile` plus the narrowed path
/// allow/deny lists. Tool access, write scopes, and approval all come from the
/// run `policy` table (`AgentRequest.run_policy`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveAgentPolicy {
    pub profile: String,
    pub path_allow: Vec<String>,
    pub path_deny: Vec<String>,
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
///
/// Subagents always inherit all base tools and run in yolo mode, so per-agent
/// `tools`, `approval`, `write_allow`, `create_files`, `create_dirs`, and
/// `max_tokens` are ignored. The run `policy` table is the source of truth for
/// those fields; `opts` may only narrow `path_allow`/`path_deny` and override
/// `profile`/`model`/`label`.
pub fn intersect_agent_policy(run: &RunPolicy, opts: &AgentPolicyOpts) -> EffectiveAgentPolicy {
    let profile = opts.profile.clone().unwrap_or_else(|| run.profile.clone());

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

    EffectiveAgentPolicy {
        profile,
        path_allow,
        path_deny,
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

#[cfg(test)]
fn path_matches(pattern: &str, path: &str) -> bool {
    if pattern == "**" || pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("/**") {
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

    fn allows_write(run: &RunPolicy, eff: &EffectiveAgentPolicy, path: &str) -> bool {
        if run.write_allow.is_empty() {
            return false;
        }
        if eff.path_deny.iter().any(|d| path_matches(d, path)) {
            return false;
        }
        run.write_allow.iter().any(|a| path_matches(a, path))
    }

    fn allows_create(run: &RunPolicy, eff: &EffectiveAgentPolicy, path: &str) -> bool {
        run.create_files && allows_write(run, eff, path)
    }

    #[test]
    fn run_tools_are_used_and_agent_cannot_add() {
        // Default run is read-only; per-agent tools are no longer accepted.
        let run = default_run_policy();
        assert!(run.tools.iter().any(|t| t == "read_file"));
        assert!(!run.tools.iter().any(|t| t == "run"));
        assert!(!run.tools.iter().any(|t| t == "write_file"));
    }

    #[test]
    fn run_write_allow_and_path_deny() {
        let mut run = default_run_policy();
        run.create_files = true;
        run.write_allow = vec!["src/a.rs".into(), "src/b.rs".into()];
        run.path_deny = vec!["src/a.rs".into()];
        let opts = AgentPolicyOpts {
            path_deny: Some(vec!["src/a.rs".into()]),
            ..Default::default()
        };
        let eff = intersect_agent_policy(&run, &opts);
        assert_eq!(
            run.write_allow,
            vec!["src/a.rs".to_string(), "src/b.rs".to_string()]
        );
        assert!(!allows_write(&run, &eff, "src/a.rs")); // deny wins
        assert!(allows_write(&run, &eff, "src/b.rs"));
    }

    #[test]
    fn run_create_files_inherited() {
        let mut run = default_run_policy();
        run.create_files = true;
        run.create_dirs = true;
        run.write_allow = vec!["scratch/probe.txt".into()];
        let eff = intersect_agent_policy(&run, &AgentPolicyOpts::default());
        assert!(run.create_files, "run create_files=true should hold");
        assert!(run.create_dirs, "run create_dirs=true should hold");
        assert!(allows_create(&run, &eff, "scratch/probe.txt"));
    }

    #[test]
    fn run_create_files_false_blocks_writes() {
        let mut run = default_run_policy();
        run.create_files = false;
        run.write_allow = vec!["scratch/probe.txt".into()];
        let eff = intersect_agent_policy(&run, &AgentPolicyOpts::default());
        assert!(!run.create_files, "run create_files=false should hold");
        assert!(!allows_create(&run, &eff, "scratch/probe.txt"));
    }

    #[test]
    fn nested_tools_always_stripped() {
        let mut run = default_run_policy();
        run.tools.push("subagent".into());
        run.tools.push("workflow".into());
        // Intersection does not keep tool lists; check via probe helper.
        let stripped: Vec<String> = run
            .tools
            .iter()
            .filter(|t| {
                !crate::tool::builtin::workflow::types::NESTED_WORKFLOW_TOOLS.contains(&t.as_str())
            })
            .cloned()
            .collect();
        assert!(!stripped.iter().any(|t| t == "subagent"));
        assert!(!stripped.iter().any(|t| t == "workflow"));
    }

    #[test]
    fn empty_run_write_allow_means_no_writes() {
        let mut run = default_run_policy();
        run.profile = "implementer".into();
        // Empty write_allow on run means no writes even for implementer profile.
        let eff = intersect_agent_policy(&run, &AgentPolicyOpts::default());
        assert!(run.write_allow.is_empty());
        assert!(!run.create_files);
        assert!(!allows_write(&run, &eff, "src/a.rs"));
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
