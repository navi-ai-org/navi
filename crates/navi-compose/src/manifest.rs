//! Component manifests.
//!
//! A NAVI component is declared by a manifest that the Matrix kernel accepts
//! (`provides`/`requires`/`subscriptions`/`calls`/`tier`/`trust`/`restart`).
//! On top of the kernel schema this crate adds a `kind` and a `host execution`
//! marker so the *host* knows how to run the component:
//!
//! * `entrypoint = "inproc:<id>"` → native Rust handler registered in-process
//!   (trusted tier; no isolation — see Matrix CHARTER #6);
//! * any other entrypoint → a real OS process (matrix-host / future work).
//!
//! Every navi component is declared to the kernel as `execution.kind = process`
//! because the kernel never executes anything itself: dispatch always goes
//! through the host's [`crate::native::NativeForwarder`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Prefix marking a component as host-executed in-process.
pub const INPROC_PREFIX: &str = "inproc:";

/// What kind of app surface a component provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ComponentKind {
    /// The agent engine facade (`navi.engine.driver@1`).
    Engine,
    /// A frontend (TUI, plain REPL, web bridge, …).
    Ui,
    /// A tool pack.
    Tool,
    /// A model provider.
    Provider,
    /// A compaction strategy.
    Compaction,
    /// A system-prompt builder.
    Prompt,
    /// A security policy.
    Security,
    /// A harness driver (loop/graph).
    Harness,
    /// Anything else.
    #[default]
    Other,
}

/// Execution declared in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Execution {
    /// Shorthand: `"inprocess"` / `"process"`.
    Kind(String),
    /// Detailed form.
    Detailed {
        #[serde(default = "default_exec_kind")]
        kind: String,
        #[serde(default)]
        entrypoint: Option<String>,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
}

fn default_exec_kind() -> String {
    "inprocess".to_string()
}

impl Default for Execution {
    fn default() -> Self {
        Execution::Kind("inprocess".to_string())
    }
}

impl Execution {
    /// Host-side execution marker for a native in-process component.
    pub fn inproc(id: &str) -> Self {
        Execution::Detailed {
            kind: "process".to_string(),
            entrypoint: Some(format!("{INPROC_PREFIX}{id}")),
            args: Vec::new(),
            timeout_ms: Some(30_000),
        }
    }

    /// Host-side execution marker for a real child process.
    pub fn process(entrypoint: impl Into<String>) -> Self {
        Execution::Detailed {
            kind: "process".to_string(),
            entrypoint: Some(entrypoint.into()),
            args: Vec::new(),
            timeout_ms: Some(30_000),
        }
    }

    /// The entrypoint string as declared (defaults to the in-process sentinel).
    pub fn entrypoint(&self) -> Option<&str> {
        match self {
            Execution::Kind(_) => None,
            Execution::Detailed { entrypoint, .. } => entrypoint.as_deref(),
        }
    }

    /// True when the host must run this component in-process.
    pub fn is_inproc(&self) -> bool {
        match self {
            Execution::Kind(kind) => kind != "process",
            Execution::Detailed { entrypoint, .. } => entrypoint
                .as_deref()
                .map(|e| e.starts_with(INPROC_PREFIX))
                .unwrap_or(true),
        }
    }

    /// Entrypoint for the kernel manifest (always concrete).
    fn kernel_entrypoint(&self, id: &str) -> String {
        self.entrypoint()
            .map(|e| e.to_string())
            .unwrap_or_else(|| format!("{INPROC_PREFIX}{id}"))
    }

    /// Timeout for the kernel manifest.
    fn timeout_ms(&self) -> u64 {
        match self {
            Execution::Kind(_) => 30_000,
            Execution::Detailed { timeout_ms, .. } => timeout_ms.unwrap_or(30_000),
        }
    }

    /// Extra args (empty for the in-process sentinel).
    fn args(&self) -> Vec<String> {
        match self {
            Execution::Kind(_) => Vec::new(),
            Execution::Detailed { args, .. } => args.clone(),
        }
    }
}

/// A dependency declaration (`requires`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequirementSpec {
    /// `"iface@major"` or `"iface"`.
    Simple(String),
    /// Explicit provider pin (no ambiguity allowed by the kernel).
    Detailed {
        interface: String,
        #[serde(default)]
        major: Option<u64>,
        #[serde(default)]
        provider: Option<String>,
    },
}

impl RequirementSpec {
    /// Versioned interface, e.g. `navi.engine.driver@1`.
    pub fn interface(&self) -> String {
        match self {
            RequirementSpec::Simple(spec) => spec.clone(),
            RequirementSpec::Detailed { interface, major, .. } => match major {
                Some(major) => format!("{interface}@{major}"),
                None => interface.clone(),
            },
        }
    }

    /// Pinned provider id, when declared.
    pub fn provider(&self) -> Option<&str> {
        match self {
            RequirementSpec::Simple(_) => None,
            RequirementSpec::Detailed { provider, .. } => provider.as_deref(),
        }
    }

    fn to_kernel_json(&self) -> Value {
        match self {
            RequirementSpec::Simple(spec) => json!(spec),
            RequirementSpec::Detailed { interface, major, provider } => {
                let mut obj = serde_json::Map::new();
                obj.insert("interface".into(), json!(interface));
                if let Some(major) = major {
                    obj.insert("major".into(), json!(major));
                }
                if let Some(provider) = provider {
                    obj.insert("provider".into(), json!(provider));
                }
                Value::Object(obj)
            }
        }
    }
}

/// Per-capability call policy (`calls`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CallPolicySpec {
    /// `cancel` (default) or `drain`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_withdraw: Option<String>,
    /// Drain budget in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drain_ms: Option<u64>,
}

/// A NAVI component definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentManifest {
    /// Logical id, e.g. `navi.ui.tui`.
    pub id: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// What this component provides (`iface@major`).
    #[serde(default, alias = "provides")]
    pub capabilities: Vec<String>,
    /// Mandatory dependencies.
    #[serde(default)]
    pub requires: Vec<RequirementSpec>,
    /// Event topics this component subscribes to at activation.
    #[serde(default)]
    pub subscriptions: Vec<String>,
    #[serde(default)]
    pub execution: Execution,
    /// `core` (first-party, trusted) | `local` (user-installed) | `remote`.
    #[serde(default = "default_tier")]
    pub tier: String,
    #[serde(default = "default_trust")]
    pub trust: String,
    /// `never` | `on-failure` | `permanent`.
    #[serde(default = "default_restart")]
    pub restart: String,
    #[serde(default)]
    pub calls: BTreeMap<String, CallPolicySpec>,
    /// NAVI-side classification (UI/engine/tool/…).
    #[serde(default)]
    pub kind: ComponentKind,
}

fn default_version() -> String {
    "0.1.0".to_string()
}

fn default_tier() -> String {
    "core".to_string()
}

fn default_trust() -> String {
    "trusted".to_string()
}

fn default_restart() -> String {
    "on-failure".to_string()
}

impl ComponentManifest {
    /// Convenience constructor for a first-party UI component.
    pub fn ui(id: &str, description: &str, requires: Vec<RequirementSpec>) -> Self {
        Self {
            id: id.to_string(),
            version: default_version(),
            description: description.to_string(),
            capabilities: vec![format!("{id}@1")],
            requires,
            subscriptions: Vec::new(),
            execution: Execution::inproc(id),
            tier: "core".into(),
            trust: "trusted".into(),
            restart: "on-failure".into(),
            calls: BTreeMap::new(),
            kind: ComponentKind::Ui,
        }
    }

    /// The manifest as the Matrix kernel expects it.
    ///
    /// `reducer` is `noop`: the kernel's built-in reducer set is never used by
    /// NAVI (dispatch always goes through the host forwarder).
    pub fn to_kernel_json(&self) -> Value {
        let requires: Vec<Value> = self.requires.iter().map(|r| r.to_kernel_json()).collect();
        let calls: serde_json::Map<String, Value> = self
            .calls
            .iter()
            .map(|(cap, policy)| {
                let mut obj = serde_json::Map::new();
                if let Some(on_withdraw) = &policy.on_withdraw {
                    obj.insert("on_withdraw".into(), json!(on_withdraw));
                }
                if let Some(drain_ms) = policy.drain_ms {
                    obj.insert("drain_ms".into(), json!(drain_ms));
                }
                (cap.clone(), Value::Object(obj))
            })
            .collect();
        json!({
            "id": self.id,
            "version": self.version,
            "description": self.description,
            "capabilities": self.capabilities,
            "requires": requires,
            "subscriptions": self.subscriptions,
            "reducer": "noop",
            "tier": self.tier,
            "trust": self.trust,
            "restart": self.restart,
            "calls": Value::Object(calls),
            "execution": {
                "kind": "process",
                "entrypoint": self.execution.kernel_entrypoint(&self.id),
                "args": self.execution.args(),
                "timeout_ms": self.execution.timeout_ms(),
            },
        })
    }

    /// Parses a manifest from TOML.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// Parses a manifest from JSON.
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }

    /// Serializes to TOML (on-disk form).
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_manifest_maps_to_kernel_json() {
        let manifest = ComponentManifest::ui(
            "navi.ui.tui",
            "ratatui frontend",
            vec![RequirementSpec::Detailed {
                interface: "navi.engine.driver".into(),
                major: Some(1),
                provider: Some("navi.engine".into()),
            }],
        );
        let value = manifest.to_kernel_json();
        assert_eq!(value["id"], "navi.ui.tui");
        assert_eq!(value["reducer"], "noop");
        assert_eq!(value["capabilities"][0], "navi.ui.tui@1");
        assert_eq!(value["requires"][0]["provider"], "navi.engine");
        assert_eq!(
            value["execution"]["entrypoint"],
            "inproc:navi.ui.tui",
            "host execution must be marked in-process"
        );
        assert!(manifest.execution.is_inproc());
    }

    #[test]
    fn requirement_simple_form_keeps_interface() {
        let req = RequirementSpec::Simple("navi.engine.events@1".into());
        assert_eq!(req.interface(), "navi.engine.events@1");
        assert!(req.provider().is_none());
        assert_eq!(req.to_kernel_json(), json!("navi.engine.events@1"));
    }

    #[test]
    fn toml_roundtrip_preserves_kind_and_execution() {
        let text = r#"
id = "navi.ui.plain"
description = "line frontend"
capabilities = ["navi.ui.plain@1"]
requires = [{ interface = "navi.engine.driver", major = 1, provider = "navi.engine" }]
kind = "ui"
execution = { kind = "process", entrypoint = "inproc:navi.ui.plain" }
restart = "on-failure"
"#;
        let manifest = ComponentManifest::from_toml(text).expect("toml");
        assert_eq!(manifest.kind, ComponentKind::Ui);
        assert!(manifest.execution.is_inproc());
        assert_eq!(manifest.requires.len(), 1);
        let reparsed = ComponentManifest::from_toml(&manifest.to_toml()).expect("roundtrip");
        assert_eq!(reparsed.id, manifest.id);
        assert_eq!(reparsed.kind, manifest.kind);
    }
}
