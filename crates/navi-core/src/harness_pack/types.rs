//! TOML schemas for harness pack loop and graph specs.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Stop predicates for a harness loop (any match ends auto-continuation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LoopStop {
    #[default]
    GoalComplete,
    GoalBlocked,
    Budget,
    VerifyOk,
    MaxTurns,
}

/// Loop parameters applied when a harness skill is active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoopSpec {
    /// Pack / skill id (mirrors directory name when written).
    pub id: String,
    /// Maximum automatic continuation turns (0 = use global goals config).
    pub max_turns: u32,
    /// Optional token budget applied to the thread goal when set.
    pub token_budget: Option<i64>,
    /// Named stop conditions (informational + used by apply logic).
    pub stop: Vec<String>,
}

impl Default for LoopSpec {
    fn default() -> Self {
        Self {
            id: String::new(),
            max_turns: 15,
            token_budget: None,
            stop: vec![
                "goal.complete".into(),
                "goal.blocked".into(),
                "budget".into(),
                "max_turns".into(),
            ],
        }
    }
}

/// A node in the soft graph (tool policy / role hints).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphNode {
    pub id: String,
    /// Optional role label (e.g. read_only, write, verify).
    pub role: String,
    /// Tools allowed while this node is active. Empty = no skill-level lock from node.
    pub allow_tools: Vec<String>,
    /// Verifier ids referenced for verify nodes.
    pub verifiers: Vec<String>,
}

impl Default for GraphNode {
    fn default() -> Self {
        Self {
            id: String::new(),
            role: "default".into(),
            allow_tools: Vec::new(),
            verifiers: Vec::new(),
        }
    }
}

/// Directed edge between graph nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    /// Optional condition label (e.g. verify.failed). Soft-only for MVP.
    pub when: Option<String>,
}

/// Soft graph: entry node + nodes/edges. Hard edge execution is out of MVP scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphSpec {
    pub entry: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

impl Default for GraphSpec {
    fn default() -> Self {
        Self {
            entry: "main".into(),
            nodes: vec![GraphNode {
                id: "main".into(),
                role: "default".into(),
                allow_tools: Vec::new(),
                verifiers: Vec::new(),
            }],
            edges: Vec::new(),
        }
    }
}

/// Kind of verifier recipe (descriptive for MVP; recipes may be bash stubs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum VerifierKind {
    #[default]
    Bash,
    Browser,
    Plugin,
    Other,
}

/// Named verifier recipe referenced by graph nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VerifierSpec {
    pub id: String,
    pub kind: VerifierKind,
    /// Human-readable recipe (command, URL, or plugin tool name).
    pub recipe: String,
}

impl Default for VerifierSpec {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: VerifierKind::Bash,
            recipe: String::new(),
        }
    }
}

/// In-memory harness pack loaded from `{data_dir}/harnesses/<id>/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessPack {
    pub id: String,
    pub root: PathBuf,
    pub loop_spec: LoopSpec,
    pub graph: Option<GraphSpec>,
    pub verifiers: Vec<VerifierSpec>,
    /// Optional CAPABILITY.md body.
    pub capability_md: Option<String>,
    /// Optional enriched SKILL.md body stored in the pack.
    pub skill_md: Option<String>,
}

impl HarnessPack {
    /// Returns the entry node of the graph, if present.
    pub fn entry_node(&self) -> Option<&GraphNode> {
        let graph = self.graph.as_ref()?;
        graph
            .nodes
            .iter()
            .find(|n| n.id == graph.entry)
            .or_else(|| graph.nodes.first())
    }
}
