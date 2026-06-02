use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Top-level manifest structure parsed from plugin.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub plugin: PluginMeta,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    #[serde(default)]
    pub tools: Vec<ToolDef>,
}

/// Plugin identity and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMeta {
    pub id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub runtime: RuntimeKind,
    pub entry: String,
    pub wasm_hash: String,
    pub signature: String,
    /// Ed25519 public key (`ed25519:<base64>`) used to verify `signature`.
    #[serde(default)]
    pub public_key: Option<String>,
    pub minimum_navi: String,
}

/// Runtime type for the plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeKind {
    WasmComponent,
}

/// Plugin trust level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrustLevel {
    Core,
    Signed,
    Community,
    LocalDev,
}

/// A capability declared in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Capability {
    Filesystem {
        id: String,
        scope: FsScope,
        access: FsAccess,
        #[serde(default)]
        paths: Vec<String>,
        reason: String,
    },
    Network {
        id: String,
        hosts: Vec<String>,
        methods: Vec<String>,
        #[serde(default = "default_true")]
        https_only: bool,
        reason: String,
        #[serde(default)]
        auth: Option<AuthBinding>,
    },
    Tui {
        id: String,
        components: Vec<String>,
        reason: String,
    },
}

fn default_true() -> bool {
    true
}

/// Filesystem scope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FsScope {
    Project,
    Workspace,
}

/// Filesystem access mode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FsAccess {
    ReadOnly,
    ReadWrite,
}

/// Auth binding for network capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthBinding {
    pub binding: String,
    pub inject_as: String,
}

/// Tool risk classification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolRisk {
    ReadOnly,
    NetworkRead,
    NetworkWrite,
    Write,
}

/// A tool declared in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub id: String,
    pub summary: String,
    pub risk: ToolRisk,
    #[serde(default)]
    pub input_schema: Option<Value>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl Capability {
    pub fn id(&self) -> &str {
        match self {
            Capability::Filesystem { id, .. } => id,
            Capability::Network { id, .. } => id,
            Capability::Tui { id, .. } => id,
        }
    }
}
