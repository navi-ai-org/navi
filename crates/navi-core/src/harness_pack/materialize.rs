//! Deterministic materialize: skill instructions → harness pack files.

use super::capability::{CapabilityInventory, capability_card, filter_tools_to_inventory};
use super::store::write_pack;
use super::types::{
    GraphEdge, GraphNode, GraphSpec, HarnessPack, LoopSpec, VerifierKind, VerifierSpec,
};
use crate::skills::SkillManifest;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Options controlling deterministic pack generation.
#[derive(Debug, Clone, Default)]
pub struct MaterializeOptions {
    /// Override pack id (defaults to skill id).
    pub id_override: Option<String>,
    /// Default max_turns when not inferred.
    pub default_max_turns: Option<u32>,
    /// Default token budget when not inferred.
    pub default_token_budget: Option<i64>,
}

/// Materialize a harness pack from a skill and capability inventory.
///
/// Tool names in the resulting graph are filtered to `inventory`. Unknown tools
/// are never invented: only names present in the inventory may appear.
pub fn materialize_from_skill(
    data_dir: &Path,
    skill: &SkillManifest,
    inventory: &CapabilityInventory,
    options: MaterializeOptions,
) -> Result<HarnessPack> {
    let id = options
        .id_override
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| skill.id.clone());

    let max_turns = options.default_max_turns.unwrap_or(15);
    let token_budget =
        options
            .default_token_budget
            .or(if inventory.goals_enabled { None } else { None });

    // Prefer skill allow_tools filtered to inventory; else a sensible default core set.
    let base_tools = if skill.allow_tools.is_empty() {
        default_core_tools(inventory)
    } else {
        filter_tools_to_inventory(&skill.allow_tools, inventory)
    };

    let explore_tools = filter_tools_to_inventory(
        &[
            "search".into(),
            "read_file".into(),
            "tool_search".into(),
            "browser".into(),
            "repo_explore".into(),
            "code".into(),
        ],
        inventory,
    );
    let write_tools = filter_tools_to_inventory(
        &[
            "search".into(),
            "read_file".into(),
            "edit".into(),
            "write_file".into(),
            "bash".into(),
            "plan".into(),
            "tool_search".into(),
        ],
        inventory,
    );
    let verify_tools = filter_tools_to_inventory(
        &[
            "bash".into(),
            "browser".into(),
            "read_file".into(),
            "search".into(),
        ],
        inventory,
    );

    // Linear soft graph: explore → implement → verify (nodes only if they have tools)
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut entry = "main".to_string();

    if !explore_tools.is_empty() && !write_tools.is_empty() {
        entry = "explore".into();
        nodes.push(GraphNode {
            id: "explore".into(),
            role: "read_only".into(),
            allow_tools: explore_tools,
            verifiers: vec![],
        });
        nodes.push(GraphNode {
            id: "implement".into(),
            role: "write".into(),
            allow_tools: write_tools,
            verifiers: vec![],
        });
        edges.push(GraphEdge {
            from: "explore".into(),
            to: "implement".into(),
            when: None,
        });
        if !verify_tools.is_empty() {
            nodes.push(GraphNode {
                id: "verify".into(),
                role: "verify".into(),
                allow_tools: verify_tools.clone(),
                verifiers: vec!["default_verify".into()],
            });
            edges.push(GraphEdge {
                from: "implement".into(),
                to: "verify".into(),
                when: None,
            });
            edges.push(GraphEdge {
                from: "verify".into(),
                to: "implement".into(),
                when: Some("verify.failed".into()),
            });
        }
    } else {
        nodes.push(GraphNode {
            id: "main".into(),
            role: "default".into(),
            allow_tools: base_tools.clone(),
            verifiers: vec![],
        });
    }

    let mut verifiers = Vec::new();
    if inventory.all_tools.iter().any(|t| t == "bash") {
        verifiers.push(VerifierSpec {
            id: "default_verify".into(),
            kind: VerifierKind::Bash,
            recipe: "echo 'run project tests via bash when appropriate'".into(),
        });
    }
    if inventory.browser_available {
        verifiers.push(VerifierSpec {
            id: "browser_smoke".into(),
            kind: VerifierKind::Browser,
            recipe: "browser open/goto/screenshot against local preview when available".into(),
        });
    }

    let loop_spec = LoopSpec {
        id: id.clone(),
        max_turns,
        token_budget,
        stop: vec![
            "goal.complete".into(),
            "goal.blocked".into(),
            "budget".into(),
            "max_turns".into(),
        ],
    };

    let skill_md = format!(
        "---\nname: {}\nid: {}\ndescription: {}\n---\n\n{}\n",
        skill.name,
        skill.id,
        skill.description.as_deref().unwrap_or(""),
        skill.instructions.trim()
    );

    let mut cap = capability_card(inventory);
    cap.push_str("\n\n## Materialize notes\n");
    cap.push_str(&format!(
        "- source_skill: {}\n- graph_entry: {entry}\n- nodes: {}\n",
        skill.id,
        nodes
            .iter()
            .map(|n| n.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    let gaps: Vec<&str> = skill
        .allow_tools
        .iter()
        .filter(|t| !inventory.all_tools.iter().any(|a| a == *t))
        .map(|s| s.as_str())
        .collect();
    if !gaps.is_empty() {
        cap.push_str(&format!(
            "- filtered_unknown_tools: [{}]\n",
            gaps.join(", ")
        ));
    }

    let pack = HarnessPack {
        id: id.clone(),
        root: PathBuf::new(),
        loop_spec,
        graph: Some(GraphSpec {
            entry,
            nodes,
            edges,
        }),
        verifiers,
        capability_md: Some(cap),
        skill_md: Some(skill_md),
    };

    let root = write_pack(data_dir, &pack)?;
    let mut out = pack;
    out.root = root;
    Ok(out)
}

fn default_core_tools(inventory: &CapabilityInventory) -> Vec<String> {
    filter_tools_to_inventory(
        &[
            "search".into(),
            "read_file".into(),
            "edit".into(),
            "write_file".into(),
            "bash".into(),
            "plan".into(),
            "question".into(),
            "tool_search".into(),
            "memory".into(),
        ],
        inventory,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness_pack::capability::inventory_from_tool_names;
    use crate::harness_pack::store::load_pack;
    use crate::skills::{SkillSource, SkillWriteScope};
    use std::path::PathBuf;

    fn fixture_skill(allow: &[&str]) -> SkillManifest {
        SkillManifest {
            id: "design-loop".into(),
            name: "Design Loop".into(),
            description: Some("Preview review harness".into()),
            version: Some("1.0.0".into()),
            author: None,
            tags: vec!["harness".into()],
            requires: vec![],
            allow_tools: allow.iter().map(|s| (*s).to_string()).collect(),
            deny_tools: vec![],
            path: PathBuf::from("builtin:design-loop"),
            instructions: "## Steps\n1. Explore\n2. Fix\n3. Verify preview\n".into(),
            source: SkillSource::Builtin,
            scope: SkillWriteScope::User,
        }
    }

    #[test]
    fn materialize_filters_unknown_tools_and_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let inv = inventory_from_tool_names(
            [
                "search",
                "read_file",
                "edit",
                "write_file",
                "bash",
                "plan",
                "tool_search",
            ],
            ["browser", "subagent"],
            true,
            true,
            50,
            None::<String>,
            None::<String>,
            None::<String>,
        );
        let skill = fixture_skill(&["search", "read_file", "a11y_audit", "browser", "edit"]);
        let pack = materialize_from_skill(dir.path(), &skill, &inv, MaterializeOptions::default())
            .unwrap();
        assert_eq!(pack.id, "design-loop");
        assert!(pack.root.join("loop.toml").is_file());
        assert!(pack.root.join("graph.toml").is_file());

        // No invented a11y_audit
        let graph = pack.graph.as_ref().unwrap();
        for node in &graph.nodes {
            assert!(
                !node.allow_tools.iter().any(|t| t == "a11y_audit"),
                "invented tool in {:?}",
                node.allow_tools
            );
        }
        assert!(
            graph
                .nodes
                .iter()
                .any(|n| n.allow_tools.iter().any(|t| t == "browser"))
        );

        let reloaded = load_pack(dir.path(), "design-loop").unwrap().unwrap();
        assert_eq!(reloaded.loop_spec.max_turns, 15);
        assert_eq!(reloaded.graph.as_ref().unwrap().entry, "explore");
    }
}
