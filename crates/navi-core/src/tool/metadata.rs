use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Rich metadata for a tool definition.
///
/// Provides the harness with enough information for routing, policy, UI,
/// concurrency, traces, verifiers, and search — without relying solely on
/// `ToolKind` for security decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolMetadata {
    /// Semantic namespace for grouping tools (e.g. "file", "code", "process", "mcp").
    #[serde(default)]
    pub namespace: String,

    /// Risk level hint for the harness: how dangerous is this tool by default.
    #[serde(default)]
    pub risk: ToolRisk,

    /// Whether this tool only reads state (never mutates repo, memory, or config).
    #[serde(default)]
    pub is_read_only: bool,

    /// Whether this tool is safe to call concurrently with other tools.
    #[serde(default)]
    pub is_concurrency_safe: bool,

    /// Whether this tool supports streaming output via events.
    #[serde(default)]
    pub supports_streaming: bool,

    /// Whether this tool can process multiple items in a single call.
    #[serde(default)]
    pub supports_batch: bool,

    /// Whether this tool supports rollback (undo of its effects).
    #[serde(default)]
    pub supports_rollback: bool,

    /// Maximum output bytes the tool produces before truncation.
    #[serde(default)]
    pub max_output_bytes: Option<usize>,

    /// Visibility/exposure mode for tool registry routing.
    #[serde(default)]
    pub exposure: ToolExposure,

    /// Capabilities required or provided by this tool.
    #[serde(default)]
    pub capabilities: Vec<String>,

    /// Verifier spec hint: which verifier to run after this tool completes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier: Option<String>,

    /// Example invocations for the model (shown in tool.search results).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<Value>,

    /// Tags for search and categorization.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// Arbitrary extended metadata (future-proof).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<HashMap<String, Value>>,
}

impl ToolMetadata {
    /// Creates a default read-only, safe, simple tool.
    pub fn read_only() -> Self {
        Self {
            is_read_only: true,
            is_concurrency_safe: true,
            ..Self::default()
        }
    }

    /// Creates a read tool (file reads, searches).
    pub fn reader(namespace: &str, tags: &[&str]) -> Self {
        Self {
            namespace: namespace.to_string(),
            risk: ToolRisk::Low,
            is_read_only: true,
            is_concurrency_safe: true,
            exposure: ToolExposure::Direct,
            capabilities: vec!["repo.read".to_string()],
            tags: tags.iter().map(|s| s.to_string()).collect(),
            ..Self::default()
        }
    }

    /// Creates a write tool (file writes, edits, patches).
    pub fn writer(namespace: &str, tags: &[&str]) -> Self {
        Self {
            namespace: namespace.to_string(),
            risk: ToolRisk::Medium,
            is_read_only: false,
            is_concurrency_safe: false,
            supports_rollback: true,
            exposure: ToolExposure::Direct,
            capabilities: vec!["repo.write".to_string()],
            tags: tags.iter().map(|s| s.to_string()).collect(),
            ..Self::default()
        }
    }

    /// Creates a command/execution tool (bash, process).
    pub fn command(namespace: &str, tags: &[&str]) -> Self {
        Self {
            namespace: namespace.to_string(),
            risk: ToolRisk::High,
            is_read_only: false,
            is_concurrency_safe: false,
            exposure: ToolExposure::Direct,
            capabilities: vec!["shell.exec".to_string()],
            tags: tags.iter().map(|s| s.to_string()).collect(),
            ..Self::default()
        }
    }

    /// Creates a tool that should be deferred (not shown by default).
    pub fn deferred(namespace: &str, risk: ToolRisk, tags: &[&str]) -> Self {
        Self {
            namespace: namespace.to_string(),
            risk,
            exposure: ToolExposure::Deferred,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            ..Self::default()
        }
    }

    /// Creates an internal/hidden tool.
    pub fn internal(namespace: &str, tags: &[&str]) -> Self {
        Self {
            namespace: namespace.to_string(),
            risk: ToolRisk::Low,
            is_read_only: true,
            is_concurrency_safe: true,
            exposure: ToolExposure::Internal,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            ..Self::default()
        }
    }
}

impl Default for ToolMetadata {
    fn default() -> Self {
        Self {
            namespace: String::new(),
            risk: ToolRisk::Unspecified,
            is_read_only: false,
            is_concurrency_safe: false,
            supports_streaming: false,
            supports_batch: false,
            supports_rollback: false,
            max_output_bytes: None,
            exposure: ToolExposure::Direct,
            capabilities: Vec::new(),
            verifier: None,
            examples: Vec::new(),
            tags: Vec::new(),
            extensions: None,
        }
    }
}

impl ToolMetadata {
    /// Returns true if this metadata is the default (unset) value.
    /// Used by ToolExecutor to detect tools that need builtin metadata injection.
    pub fn is_default(&self) -> bool {
        self.namespace.is_empty()
            && self.risk == ToolRisk::Unspecified
            && !self.is_read_only
            && !self.is_concurrency_safe
            && self.capabilities.is_empty()
            && self.tags.is_empty()
    }

    /// Builder: sets exposure mode.
    pub fn with_exposure(mut self, exposure: ToolExposure) -> Self {
        self.exposure = exposure;
        self
    }

    /// Builder: sets capabilities.
    pub fn with_capability(mut self, caps: &[&str]) -> Self {
        self.capabilities = caps.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Builder: sets max output bytes.
    pub fn with_max_output(mut self, bytes: usize) -> Self {
        self.max_output_bytes = Some(bytes);
        self
    }

    /// Builder: adds tags.
    pub fn with_tags(mut self, tags: &[&str]) -> Self {
        self.tags = tags.iter().map(|s| s.to_string()).collect();
        self
    }
}

/// Risk level hint for a tool.
///
/// Used by the policy system to decide whether a tool invocation needs
/// approval, sandboxing, or rollback capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolRisk {
    /// Risk not yet classified.
    Unspecified,
    /// Minimal risk (e.g. read tools, info tools).
    Low,
    /// Moderate risk (e.g. file writes, git operations).
    Medium,
    /// High risk (e.g. shell commands, network, MCP execution).
    High,
    /// Critical risk (e.g. credential access, guarded commands, privilege escalation).
    Critical,
}

impl Default for ToolRisk {
    fn default() -> Self {
        Self::Unspecified
    }
}

/// Exposure mode for a tool in the registry.
///
/// Controls whether a tool is visible to the model by default or needs to be
/// discovered through `tool.search`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolExposure {
    /// Tool is visible in the main prompt by default.
    Direct,
    /// Tool is registered but not visible; can be discovered via `tool.search`.
    Deferred,
    /// Tool is hidden from the model entirely; only usable by internal systems.
    Hidden,
    /// Tool is visible only to the model (not shown in UI).
    #[serde(rename = "model_only")]
    ModelOnly,
    /// Tool is for internal harness use only (not visible to model or user).
    Internal,
}

impl Default for ToolExposure {
    fn default() -> Self {
        Self::Direct
    }
}

/// Set of capabilities that a tool may require or provide.
///
/// These are used by the capability ledger (post-parity) and by exposure
/// routing.
pub mod capabilities {
    pub const REPO_READ: &str = "repo.read";
    pub const REPO_WRITE: &str = "repo.write";
    pub const REPO_WRITE_SRC: &str = "repo.write.src";
    pub const REPO_WRITE_TESTS: &str = "repo.write.tests";
    pub const REPO_WRITE_DOCS: &str = "repo.write.docs";
    pub const REPO_WRITE_CI: &str = "repo.write.ci";
    pub const REPO_WRITE_LOCKFILE: &str = "repo.write.lockfile";
    pub const SHELL_EXEC: &str = "shell.exec";
    pub const SHELL_PRIVILEGED: &str = "shell.privileged";
    pub const NETWORK_GITHUB: &str = "network.github";
    pub const NETWORK_PACKAGE: &str = "network.package";
    pub const NETWORK_GENERAL: &str = "network.general";
    pub const SECRETS_READ: &str = "secrets.read";
    pub const MCP_ACCESS: &str = "mcp.access";
    pub const AGENT_SPAWN: &str = "agent.spawn";
    pub const MEMORY_READ: &str = "memory.read";
    pub const MEMORY_WRITE: &str = "memory.write";
    pub const VERIFIER_RUN: &str = "verifier.run";
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn metadata_default_serde_roundtrip() {
        let m = ToolMetadata::default();
        let json = serde_json::to_value(&m).unwrap();
        let restored: ToolMetadata = serde_json::from_value(json).unwrap();
        assert_eq!(m.risk, restored.risk);
        assert_eq!(m.exposure, restored.exposure);
        assert_eq!(m.is_read_only, restored.is_read_only);
    }

    #[test]
    fn metadata_read_only_preset() {
        let m = ToolMetadata::read_only();
        assert!(m.is_read_only);
        assert!(m.is_concurrency_safe);
    }

    #[test]
    fn metadata_reader_preset() {
        let m = ToolMetadata::reader("file", &["read", "search"]);
        assert!(m.is_read_only);
        assert!(m.is_concurrency_safe);
        assert_eq!(m.risk, ToolRisk::Low);
        assert_eq!(m.exposure, ToolExposure::Direct);
        assert_eq!(m.namespace, "file");
        assert!(m.capabilities.contains(&"repo.read".to_string()));
    }

    #[test]
    fn metadata_writer_preset() {
        let m = ToolMetadata::writer("file", &["edit", "patch"]);
        assert!(!m.is_read_only);
        assert!(!m.is_concurrency_safe);
        assert!(m.supports_rollback);
        assert_eq!(m.risk, ToolRisk::Medium);
    }

    #[test]
    fn metadata_command_preset() {
        let m = ToolMetadata::command("process", &["shell", "bash"]);
        assert_eq!(m.risk, ToolRisk::High);
        assert!(!m.is_concurrency_safe);
        assert!(m.capabilities.contains(&"shell.exec".to_string()));
    }

    #[test]
    fn metadata_deferred_preset() {
        let m = ToolMetadata::deferred("mcp", ToolRisk::Medium, &["mcp", "external"]);
        assert_eq!(m.exposure, ToolExposure::Deferred);
        assert_eq!(m.risk, ToolRisk::Medium);
    }

    #[test]
    fn metadata_internal_preset() {
        let m = ToolMetadata::internal("runtime", &["internal"]);
        assert_eq!(m.exposure, ToolExposure::Internal);
        assert!(m.is_read_only);
        assert!(m.is_concurrency_safe);
    }

    #[test]
    fn metadata_full_serialization() {
        let m = ToolMetadata {
            namespace: "test".to_string(),
            risk: ToolRisk::High,
            is_read_only: false,
            is_concurrency_safe: false,
            supports_streaming: true,
            supports_batch: false,
            supports_rollback: true,
            max_output_bytes: Some(65536),
            exposure: ToolExposure::Direct,
            capabilities: vec!["test.cap".to_string()],
            verifier: Some("verify.test".to_string()),
            examples: vec![json!({"key": "value"})],
            tags: vec!["tag1".to_string(), "tag2".to_string()],
            extensions: None,
        };
        let v = serde_json::to_value(&m).unwrap();
        let restored: ToolMetadata = serde_json::from_value(v).unwrap();
        assert_eq!(restored.namespace, "test");
        assert_eq!(restored.risk, ToolRisk::High);
        assert_eq!(restored.max_output_bytes, Some(65536));
        assert_eq!(restored.verifier, Some("verify.test".to_string()));
        assert_eq!(restored.tags.len(), 2);
    }

    #[test]
    fn risk_default_is_unspecified() {
        assert_eq!(ToolRisk::default(), ToolRisk::Unspecified);
    }

    #[test]
    fn exposure_default_is_direct() {
        assert_eq!(ToolExposure::default(), ToolExposure::Direct);
    }
}
