//! Apply harness packs on skill activation (soft graph + loop limits).

use super::store::load_pack;
use super::types::HarnessPack;
use crate::skills::SkillManifest;
use std::collections::BTreeSet;
use std::path::Path;

/// Result of applying harness packs for active skills.
#[derive(Debug, Clone, Default)]
pub struct HarnessApplyResult {
    /// Packs successfully loaded for active skills (order preserved by skill order).
    pub packs: Vec<HarnessPack>,
    /// Max auto-continue turns to apply (minimum across packs with max_turns > 0).
    pub max_auto_continue_turns: Option<u32>,
    /// Token budget to apply to a new/updated goal (minimum of pack budgets if set).
    pub token_budget: Option<i64>,
    /// Effective tool allowlist from soft graph entry nodes, merged with skill allow_tools.
    /// `None` means no allowlist restriction from harness packs.
    pub allow_tools: Option<Vec<String>>,
    /// Developer-context harness card text (empty if no packs).
    pub harness_card: String,
}

/// Derive allow_tools for a pack from the graph entry node (soft graph).
///
/// Returns `None` if there is no graph or the entry node has an empty allow_tools list
/// (meaning no node-level lock).
pub fn effective_allow_tools_for_pack(pack: &HarnessPack) -> Option<Vec<String>> {
    let node = pack.entry_node()?;
    if node.allow_tools.is_empty() {
        return None;
    }
    let mut tools = node.allow_tools.clone();
    tools.sort();
    tools.dedup();
    Some(tools)
}

/// Intersect non-empty allowlists; empty inputs are ignored.
/// Returns `None` if no non-empty lists were provided.
pub fn merge_allow_tools(lists: &[Vec<String>]) -> Option<Vec<String>> {
    let non_empty: Vec<&Vec<String>> = lists.iter().filter(|l| !l.is_empty()).collect();
    if non_empty.is_empty() {
        return None;
    }
    let mut set: BTreeSet<String> = non_empty[0].iter().cloned().collect();
    for list in non_empty.iter().skip(1) {
        set.retain(|t| list.iter().any(|a| a == t));
    }
    let mut out: Vec<String> = set.into_iter().collect();
    out.sort();
    Some(out)
}

/// Load packs for active skills and compute soft apply result.
pub fn apply_harness_for_skills(
    data_dir: &Path,
    active_skills: &[SkillManifest],
) -> HarnessApplyResult {
    let mut result = HarnessApplyResult::default();
    let mut allow_lists: Vec<Vec<String>> = Vec::new();
    let mut card_sections: Vec<String> = Vec::new();

    for skill in active_skills {
        // Soft-lock only for skills that opt into harness mode.
        // Catalog metadata / authoring builtins (harness:false) must never lock
        // the session — even if a stale pack directory exists under data_dir.
        if !skill.harness {
            continue;
        }
        if !skill.allow_tools.is_empty() {
            allow_lists.push(skill.allow_tools.clone());
        }
        let Ok(Some(pack)) = load_pack(data_dir, &skill.id) else {
            continue;
        };
        if pack.loop_spec.max_turns > 0 {
            result.max_auto_continue_turns = Some(
                result
                    .max_auto_continue_turns
                    .map(|m| m.min(pack.loop_spec.max_turns))
                    .unwrap_or(pack.loop_spec.max_turns),
            );
        }
        if let Some(budget) = pack.loop_spec.token_budget
            && budget > 0
        {
            result.token_budget =
                Some(result.token_budget.map(|b| b.min(budget)).unwrap_or(budget));
        }
        if let Some(tools) = effective_allow_tools_for_pack(&pack) {
            allow_lists.push(tools);
        }
        card_sections.push(render_harness_card_section(&pack));
        result.packs.push(pack);
    }

    result.allow_tools = merge_allow_tools(&allow_lists);
    if !card_sections.is_empty() {
        result.harness_card = format!(
            "=== Active Harness Packs ===\n{}",
            card_sections.join("\n\n")
        );
    }
    result
}

fn render_harness_card_section(pack: &HarnessPack) -> String {
    let mut lines = Vec::new();
    lines.push(format!("## harness `{}`", pack.id));
    lines.push(format!("- path: {}", pack.root.display()));
    lines.push(format!("- loop.max_turns: {}", pack.loop_spec.max_turns));
    if let Some(b) = pack.loop_spec.token_budget {
        lines.push(format!("- loop.token_budget: {b}"));
    }
    lines.push(format!("- loop.stop: [{}]", pack.loop_spec.stop.join(", ")));
    if let Some(graph) = &pack.graph {
        lines.push(format!("- graph.entry: {}", graph.entry));
        let node_ids: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        lines.push(format!("- graph.nodes: [{}]", node_ids.join(", ")));
        if let Some(entry) = pack.entry_node()
            && !entry.allow_tools.is_empty()
        {
            lines.push(format!(
                "- graph.entry_allow_tools: [{}]",
                entry.allow_tools.join(", ")
            ));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness_pack::capability::inventory_from_tool_names;
    use crate::harness_pack::materialize::{MaterializeOptions, materialize_from_skill};
    use crate::skills::{SkillSource, SkillWriteScope};
    use std::path::PathBuf;

    fn skill(id: &str, allow: &[&str]) -> SkillManifest {
        SkillManifest {
            id: id.into(),
            name: id.into(),
            description: None,
            version: None,
            author: None,
            tags: vec![],
            requires: vec![],
            allow_tools: allow.iter().map(|s| (*s).to_string()).collect(),
            deny_tools: vec![],
            harness: false,
            pool: None,
            path: PathBuf::from("x"),
            instructions: "do the thing".into(),
            source: SkillSource::Store,
            scope: SkillWriteScope::User,
        }
    }

    #[test]
    fn soft_graph_entry_allow_tools() {
        let dir = tempfile::tempdir().unwrap();
        let inv = inventory_from_tool_names(
            [
                "search",
                "read_file",
                "edit",
                "write_file",
                "bash",
                "tool_search",
            ],
            ["browser"],
            true,
            true,
            50,
            None::<String>,
            None::<String>,
            None::<String>,
        );
        let s = skill("design-loop", &[]);
        materialize_from_skill(dir.path(), &s, &inv, MaterializeOptions::default()).unwrap();
        let pack = load_pack(dir.path(), "design-loop").unwrap().unwrap();
        let tools = effective_allow_tools_for_pack(&pack).unwrap();
        assert!(tools.contains(&"search".to_string()) || tools.contains(&"read_file".to_string()));
        // Entry is explore → read-oriented, not write_file-only
        assert!(tools.iter().any(|t| t == "search" || t == "read_file"));
    }

    #[test]
    fn apply_merges_loop_caps_and_card() {
        let dir = tempfile::tempdir().unwrap();
        let inv = inventory_from_tool_names(
            [
                "search",
                "read_file",
                "edit",
                "write_file",
                "bash",
                "tool_search",
                "plan",
            ],
            ["browser"],
            true,
            true,
            50,
            None::<String>,
            None::<String>,
            None::<String>,
        );
        let mut s = skill("design-loop", &[]);
        s.harness = true;
        let mut opts = MaterializeOptions::default();
        opts.default_max_turns = Some(7);
        opts.default_token_budget = Some(20_000);
        materialize_from_skill(dir.path(), &s, &inv, opts).unwrap();

        let mut active_skill = skill("design-loop", &[]);
        active_skill.harness = true;
        let active = vec![active_skill];
        let applied = apply_harness_for_skills(dir.path(), &active);
        assert_eq!(applied.packs.len(), 1);
        assert_eq!(applied.max_auto_continue_turns, Some(7));
        assert_eq!(applied.token_budget, Some(20_000));
        assert!(!applied.harness_card.is_empty());
        assert!(applied.harness_card.contains("design-loop"));
        assert!(applied.allow_tools.is_some());
    }

    #[test]
    fn merge_allow_tools_intersects() {
        let a = vec!["a".into(), "b".into(), "c".into()];
        let b = vec!["b".into(), "c".into(), "d".into()];
        let m = merge_allow_tools(&[a, b]).unwrap();
        assert_eq!(m, vec!["b".to_string(), "c".to_string()]);
    }

    #[test]
    fn catalog_create_skill_style_allow_tools_do_not_lock_without_harness_flag() {
        // Builtin create-skill has allow_tools but harness:false — catalog discovery
        // must not produce a session allowlist (root tools stay open).
        let dir = tempfile::tempdir().unwrap();
        let create = skill(
            "navi-create-skill",
            &["skill_list", "skill_save", "load_skill", "read_file"],
        );
        assert!(!create.harness);
        let applied = apply_harness_for_skills(dir.path(), &[create]);
        assert!(
            applied.allow_tools.is_none(),
            "non-harness skill allow_tools must not lock the session: {:?}",
            applied.allow_tools
        );
        assert!(applied.packs.is_empty());
    }

    #[test]
    fn empty_active_list_means_no_soft_allowlist() {
        let dir = tempfile::tempdir().unwrap();
        let applied = apply_harness_for_skills(dir.path(), &[]);
        assert!(applied.allow_tools.is_none());
        assert!(applied.packs.is_empty());
        assert!(applied.harness_card.is_empty());
    }

    #[test]
    fn harness_flagged_skill_allow_tools_lock_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = skill("design-loop", &["read_file", "search"]);
        s.harness = true;
        let applied = apply_harness_for_skills(dir.path(), &[s]);
        assert_eq!(
            applied.allow_tools.as_deref(),
            Some(["read_file".to_string(), "search".to_string()].as_slice())
        );
    }

    #[test]
    fn pack_entry_allowlist_only_when_skill_passed_as_active() {
        // Packs on disk for skills that are *not* in the active list must not
        // be consulted — callers pass only session-active manifests.
        let dir = tempfile::tempdir().unwrap();
        let inv = inventory_from_tool_names(
            [
                "search",
                "read_file",
                "edit",
                "write_file",
                "bash",
                "tool_search",
            ],
            ["browser"],
            true,
            true,
            50,
            None::<String>,
            None::<String>,
            None::<String>,
        );
        let mut s = skill("design-loop", &[]);
        s.harness = true;
        materialize_from_skill(dir.path(), &s, &inv, MaterializeOptions::default()).unwrap();

        // Not in active list → no lock even though pack exists on disk.
        let idle = apply_harness_for_skills(dir.path(), &[]);
        assert!(idle.allow_tools.is_none());

        // Active but harness:false → pack on disk must not soft-lock.
        let non_harness = skill("design-loop", &[]);
        assert!(!non_harness.harness);
        let skipped = apply_harness_for_skills(dir.path(), &[non_harness]);
        assert!(
            skipped.allow_tools.is_none(),
            "non-harness skill must ignore on-disk pack: {:?}",
            skipped.allow_tools
        );

        // Activated with harness:true → soft entry allowlist from pack.
        let mut active_skill = skill("design-loop", &[]);
        active_skill.harness = true;
        let active = apply_harness_for_skills(dir.path(), &[active_skill]);
        assert!(
            active.allow_tools.is_some(),
            "active harness skill with pack should soft-lock tools"
        );
        let tools = active.allow_tools.unwrap();
        assert!(
            tools.iter().any(|t| t == "search" || t == "read_file"),
            "expected explore-oriented entry tools, got {tools:?}"
        );
    }
}
