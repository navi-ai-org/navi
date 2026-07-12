//! Lockfile approval checks for loaded WASM plugins.

use std::collections::HashSet;

use crate::lockfile::LockEntry;
use crate::types::PluginManifest;

/// Ensure every declared capability and tool reference is present in the lockfile entry.
pub fn verify_approved_capabilities(
    manifest: &PluginManifest,
    entry: &LockEntry,
) -> Result<(), String> {
    let approved: HashSet<&str> = entry
        .approved_capabilities
        .iter()
        .map(|s| s.as_str())
        .collect();

    for cap in &manifest.capabilities {
        let id = cap.id();
        if !approved.contains(id) {
            return Err(format!(
                "capability '{id}' is not approved in lockfile; update the plugin with reconsent"
            ));
        }
    }

    for tool in &manifest.tools {
        for cap_id in &tool.capabilities {
            if !approved.contains(cap_id.as_str()) {
                return Err(format!(
                    "tool '{}' requires unapproved capability '{cap_id}'",
                    tool.id
                ));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Capability, FsAccess, FsScope, PluginMeta, RuntimeKind, ToolDef, ToolRisk};

    fn manifest_with_fs_cap() -> PluginManifest {
        PluginManifest {
            plugin: PluginMeta {
                id: "p".into(),
                name: "p".into(),
                version: "1.0.0".into(),
                publisher: "gh:t".into(),
                runtime: RuntimeKind::WasmComponent,
                entry: "plugin.wasm".into(),
                wasm_hash: format!("sha256:{}", "a".repeat(64)),
                signature: "ed25519:00".into(),
                public_key: None,
                minimum_navi: "0.1.0".into(),
            },
            capabilities: vec![Capability::Filesystem {
                id: "fs_read".into(),
                scope: FsScope::Project,
                access: FsAccess::ReadOnly,
                paths: vec!["src/".into()],
                reason: "read".into(),
            }],
            tools: vec![ToolDef {
                id: "index".into(),
                summary: "index".into(),
                risk: ToolRisk::ReadOnly,
                input_schema: None,
                capabilities: vec!["fs_read".into()],
            }],
        }
    }

    #[test]
    fn rejects_unapproved_capability() {
        let manifest = manifest_with_fs_cap();
        let entry = LockEntry {
            id: "p".into(),
            version: "1.0.0".into(),
            publisher: "gh:t".into(),
            wasm_hash: manifest.plugin.wasm_hash.clone(),
            capabilities_hash: String::new(),
            tools_hash: String::new(),
            approved_capabilities: vec![],
            approved_at: "0".into(),
            trust_level: crate::types::TrustLevel::Community,
            kind: crate::marketplace::PluginCatalogKind::Plugin,
        };
        let err = verify_approved_capabilities(&manifest, &entry).unwrap_err();
        assert!(err.contains("fs_read"));
    }

    #[test]
    fn allows_when_capability_approved() {
        let manifest = manifest_with_fs_cap();
        let entry = LockEntry {
            id: "p".into(),
            version: "1.0.0".into(),
            publisher: "gh:t".into(),
            wasm_hash: manifest.plugin.wasm_hash.clone(),
            capabilities_hash: String::new(),
            tools_hash: String::new(),
            approved_capabilities: vec!["fs_read".into()],
            approved_at: "0".into(),
            trust_level: crate::types::TrustLevel::Community,
            kind: crate::marketplace::PluginCatalogKind::Plugin,
        };
        verify_approved_capabilities(&manifest, &entry).unwrap();
    }
}
