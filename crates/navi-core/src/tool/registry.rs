use crate::tool::metadata::ToolExposure;
use crate::tool::{ToolDefinition, ToolKind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A categorized collection of tool definitions with exposure control.
///
/// Separates *registered* tools from *visible* tools. Use `ToolSet::for_phase()`
/// to get only the tools appropriate for the current execution phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRegistry {
    /// All registered tools, keyed by name.
    pub tools: HashMap<String, RegisteredTool>,
}

/// A registered tool with its exposure and phase assignments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredTool {
    /// The full tool definition.
    pub definition: ToolDefinition,
    /// Exposure mode (direct, deferred, hidden, etc.).
    pub exposure: ToolExposure,
    /// Which phases this tool belongs to (default: all phases).
    #[serde(default)]
    pub phases: Vec<String>,
}

/// Historical Codex-style threshold kept for compatibility with older tests and
/// docs. Deferred tools are **never** auto-promoted into the model schema; they
/// stay discoverable via `tool_search` regardless of Direct tool count.
pub const MCP_TOOL_DEFER_THRESHOLD: usize = 100;

impl ToolRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Registers a tool with default exposure (Direct) and all phases.
    pub fn register(&mut self, definition: ToolDefinition) {
        let exposure = definition.metadata.exposure;
        self.tools.insert(
            definition.name.clone(),
            RegisteredTool {
                definition,
                exposure,
                phases: Vec::new(), // empty = all phases
            },
        );
    }

    /// Registers a tool with specific exposure and phases.
    pub fn register_with(
        &mut self,
        definition: ToolDefinition,
        exposure: ToolExposure,
        phases: Vec<String>,
    ) {
        self.tools.insert(
            definition.name.clone(),
            RegisteredTool {
                definition,
                exposure,
                phases,
            },
        );
    }

    /// Returns tool definitions visible in the model schema.
    ///
    /// Only `Direct` and `ModelOnly` tools are included. `Deferred` tools stay
    /// out of the request schema and must be discovered via `tool_search`
    /// (or called by name after discovery). Hidden/Internal tools never appear.
    pub fn visible_definitions(&self) -> Vec<ToolDefinition> {
        let mut defs: Vec<ToolDefinition> = self
            .tools
            .values()
            .filter(|t| matches!(t.exposure, ToolExposure::Direct | ToolExposure::ModelOnly))
            .map(|t| t.definition.clone())
            .collect();
        // HashMap iteration order is process-seeded. Sort by name so the tools
        // array in every provider request is byte-stable and prefix-cacheable.
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// Returns all visible tool names for the model (Direct + ModelOnly).
    pub fn visible_tool_names(&self) -> Vec<String> {
        self.visible_definitions()
            .into_iter()
            .map(|d| d.name)
            .collect()
    }

    /// Returns tool definitions for a specific phase.
    ///
    /// Only includes Direct-exposure tools. A tool with no explicit phases is
    /// considered available in all phases.
    pub fn for_phase(&self, phase: &str) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .filter(|t| t.exposure == ToolExposure::Direct)
            .filter(|t| t.phases.is_empty() || t.phases.iter().any(|p| p == phase))
            .map(|t| t.definition.clone())
            .collect()
    }

    /// Returns all tool names in the registry.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tools.keys().cloned().collect();
        names.sort();
        names
    }

    /// Removes every registered tool whose name starts with `prefix`.
    pub fn unregister_prefix(&mut self, prefix: &str) {
        self.tools.retain(|name, _| !name.starts_with(prefix));
    }

    /// Looks up a tool by name.
    pub fn get(&self, name: &str) -> Option<&RegisteredTool> {
        self.tools.get(name)
    }

    /// Returns all deferred tools (for tool.search discovery).
    pub fn deferred_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .filter(|t| t.exposure == ToolExposure::Deferred)
            .map(|t| t.definition.clone())
            .collect()
    }

    /// Searches tools by keyword across name, description, tags, and capabilities.
    /// Returns a ranked list of definitions (BM25-inspired scoring).
    pub fn search(&self, query: &str, max_results: usize) -> Vec<ToolDefinition> {
        let query = query.to_lowercase();
        let query_terms: Vec<&str> = query.split_whitespace().collect();
        if query_terms.is_empty() || max_results == 0 {
            return Vec::new();
        }

        let mut scored: Vec<(i32, &ToolDefinition)> = self
            .tools
            .values()
            .filter(|t| {
                // Only searchable: Direct, Deferred, ModelOnly (not Hidden, not Internal)
                matches!(
                    t.exposure,
                    ToolExposure::Direct | ToolExposure::Deferred | ToolExposure::ModelOnly
                )
            })
            .map(|t| {
                let def = &t.definition;
                let score = compute_search_score(def, &query_terms);
                (score, def)
            })
            .filter(|(score, _)| *score > 0)
            .collect();

        scored.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| left.name.cmp(&right.name))
        });
        scored
            .into_iter()
            .take(max_results)
            .map(|(_, def)| def.clone())
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// BM25-inspired scoring for tool search.
fn compute_search_score(def: &ToolDefinition, terms: &[&str]) -> i32 {
    let mut score = 0i32;

    for term in terms {
        // Exact name match: highest weight
        if def.name.to_lowercase() == *term {
            score += 100;
            continue;
        }
        // Name substring match
        if def.name.to_lowercase().contains(term) {
            score += 50;
        }
        // Description match
        if def.description.to_lowercase().contains(term) {
            score += 20;
        }
        if def.metadata.namespace.to_lowercase().contains(term) {
            score += 18;
        }
        // Tag match
        for tag in &def.metadata.tags {
            if tag.to_lowercase().contains(term) {
                score += 15;
            }
        }
        // Capability match
        for cap in &def.metadata.capabilities {
            if cap.to_lowercase().contains(term) {
                score += 10;
            }
        }
        for example in &def.metadata.examples {
            if example.to_string().to_lowercase().contains(term) {
                score += 6;
            }
        }
        // Kind-based boost
        let kind_str = match def.kind {
            ToolKind::Read => "read",
            ToolKind::Write => "write",
            ToolKind::Command => "command",
            ToolKind::Custom => "custom",
        };
        if kind_str.contains(term) {
            score += 5;
        }
    }

    score
}

/// A lightweight tool set for a specific phase of execution.
#[derive(Debug, Clone)]
pub struct ToolSet {
    /// Phase identifier.
    pub phase: String,
    /// Tool definitions available in this phase.
    pub definitions: Vec<ToolDefinition>,
}

impl ToolSet {
    /// Creates a new tool set for the given phase from a registry.
    pub fn for_phase(registry: &ToolRegistry, phase: &str) -> Self {
        Self {
            phase: phase.to_string(),
            definitions: registry.for_phase(phase),
        }
    }

    /// Returns tool names in this set.
    pub fn names(&self) -> Vec<String> {
        self.definitions.iter().map(|d| d.name.clone()).collect()
    }
}

/// Standard phase names for tool sets.
pub mod phases {
    /// Initial planning phase: high-level reasoning, no mutations.
    pub const PLANNING: &str = "planning";
    /// Reading/exploring the repo structure and files.
    pub const READING: &str = "reading";
    /// Editing/writing files.
    pub const EDITING: &str = "editing";
    /// Running verification (build, test, lint).
    pub const VERIFYING: &str = "verifying";
    /// Reviewing changes before finalizing.
    pub const REVIEWING: &str = "reviewing";
    /// Recovery from errors or rollbacks.
    pub const RECOVERY: &str = "recovery";
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolMetadata;
    use serde_json::json;

    fn make_def(name: &str, kind: ToolKind, tags: &[&str], caps: &[&str]) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("Tool that does {}", name),
            kind,
            input_schema: json!({"type": "object"}),
            metadata: ToolMetadata {
                tags: tags.iter().map(|s| s.to_string()).collect(),
                capabilities: caps.iter().map(|s| s.to_string()).collect(),
                ..ToolMetadata::default()
            },
        }
    }

    #[test]
    fn registry_empty_by_default() {
        let reg = ToolRegistry::new();
        assert!(reg.visible_definitions().is_empty());
    }

    #[test]
    fn visible_definitions_are_sorted_by_name() {
        let mut reg = ToolRegistry::new();
        // Insert out of order — HashMap would otherwise yield unstable order.
        for name in ["write_file", "bash", "read_file", "apply_patch"] {
            reg.register(make_def(name, ToolKind::Read, &[], &[]));
        }
        let names: Vec<String> = reg
            .visible_definitions()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(
            names,
            vec!["apply_patch", "bash", "read_file", "write_file"]
        );
    }

    #[test]
    fn registry_register_and_retrieve() {
        let mut reg = ToolRegistry::new();
        let def = make_def(
            "read_file",
            ToolKind::Read,
            &["file", "read"],
            &["repo.read"],
        );
        reg.register(def.clone());
        assert_eq!(reg.visible_definitions().len(), 1);
        assert!(reg.get("read_file").is_some());
    }

    #[test]
    fn registry_deferred_tools_not_visible() {
        let mut reg = ToolRegistry::new();
        // Even with a tiny Direct set, Deferred must stay out of the schema.
        reg.register(make_def("read_file", ToolKind::Read, &["tool"], &[]));
        let mut def = make_def("secret_tool", ToolKind::Custom, &["power"], &[]);
        def.metadata.exposure = ToolExposure::Deferred;
        reg.register(def);
        let visible: Vec<String> = reg
            .visible_definitions()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(visible, vec!["read_file".to_string()]);
        assert_eq!(reg.deferred_definitions().len(), 1);
        // Deferred tools remain discoverable via search.
        let found = reg.search("secret", 5);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "secret_tool");
    }

    #[test]
    fn registry_never_auto_promotes_deferred_tools() {
        let mut reg = ToolRegistry::new();
        for name in ["bash", "edit", "read_file"] {
            reg.register(make_def(name, ToolKind::Read, &["core"], &[]));
        }
        for name in ["code", "package_manager", "browser"] {
            let mut def = make_def(name, ToolKind::Custom, &["power"], &[]);
            def.metadata.exposure = ToolExposure::Deferred;
            reg.register(def);
        }
        let visible = reg.visible_tool_names();
        assert_eq!(visible.len(), 3);
        assert!(!visible.iter().any(|n| n == "code"));
        assert!(!visible.iter().any(|n| n == "package_manager"));
        assert!(!visible.iter().any(|n| n == "browser"));
        // Threshold constant remains available for compatibility callers.
        assert_eq!(MCP_TOOL_DEFER_THRESHOLD, 100);
    }

    #[test]
    fn registry_hidden_tools_not_searchable() {
        let mut reg = ToolRegistry::new();
        let mut def = make_def("internal_tool", ToolKind::Custom, &["internal"], &[]);
        def.metadata.exposure = ToolExposure::Hidden;
        reg.register(def);
        assert!(reg.search("internal", 10).is_empty());
    }

    #[test]
    fn registry_search_ranking() {
        let mut reg = ToolRegistry::new();
        reg.register(make_def(
            "read_file",
            ToolKind::Read,
            &["file", "read"],
            &["repo.read"],
        ));
        reg.register(make_def(
            "write_file",
            ToolKind::Write,
            &["file", "write"],
            &["repo.write"],
        ));
        reg.register(make_def(
            "bash",
            ToolKind::Command,
            &["shell"],
            &["shell.exec"],
        ));

        let results = reg.search("file", 10);
        assert!(results.len() >= 2);
        // read_file and write_file should rank higher than bash for "file" query
        let names: Vec<&str> = results.iter().map(|d| d.name.as_str()).collect();
        // Both file tools should be present
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
    }

    #[test]
    fn registry_phase_filtering() {
        let mut reg = ToolRegistry::new();
        let read_def = make_def("read_file", ToolKind::Read, &[], &[]);
        let write_def = make_def("write_file", ToolKind::Write, &[], &[]);
        reg.register_with(read_def, ToolExposure::Direct, vec!["reading".to_string()]);
        reg.register_with(write_def, ToolExposure::Direct, vec!["editing".to_string()]);

        let reading_set = reg.for_phase("reading");
        assert_eq!(reading_set.len(), 1);
        assert_eq!(reading_set[0].name, "read_file");

        let editing_set = reg.for_phase("editing");
        assert_eq!(editing_set.len(), 1);
        assert_eq!(editing_set[0].name, "write_file");
    }

    #[test]
    fn registry_phase_empty_means_all_phases() {
        let mut reg = ToolRegistry::new();
        let def = make_def("bash", ToolKind::Command, &[], &[]);
        reg.register_with(def, ToolExposure::Direct, vec![]); // empty = all phases

        assert_eq!(reg.for_phase("planning").len(), 1);
        assert_eq!(reg.for_phase("reading").len(), 1);
        assert_eq!(reg.for_phase("editing").len(), 1);
    }

    #[test]
    fn toolset_for_phase_creates_correct_set() {
        let mut reg = ToolRegistry::new();
        reg.register(make_def("read", ToolKind::Read, &[], &[]));
        reg.register(make_def("write", ToolKind::Write, &[], &[]));

        let ts = ToolSet::for_phase(&reg, "planning");
        assert_eq!(ts.phase, "planning");
        assert_eq!(ts.definitions.len(), 2);
    }

    #[test]
    fn search_respects_max_results() {
        let mut reg = ToolRegistry::new();
        for i in 0..10 {
            reg.register(make_def(
                &format!("tool_{}", i),
                ToolKind::Custom,
                &["test"],
                &[],
            ));
        }
        let results = reg.search("test", 3);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn search_excludes_zero_score_tools() {
        let mut reg = ToolRegistry::new();
        reg.register(make_def(
            "read_file",
            ToolKind::Read,
            &["file"],
            &["repo.read"],
        ));

        assert!(reg.search("nonexistent-capability", 10).is_empty());
        assert!(reg.search("", 10).is_empty());
    }
}
