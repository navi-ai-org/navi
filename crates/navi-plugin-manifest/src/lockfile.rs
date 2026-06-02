use serde::{Deserialize, Serialize};
use std::path::Path;

/// Lockfile entry for an installed plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockEntry {
    pub id: String,
    pub version: String,
    pub publisher: String,
    pub wasm_hash: String,
    pub capabilities_hash: String,
    pub tools_hash: String,
    pub approved_capabilities: Vec<String>,
    pub approved_at: String,
}

/// The full lockfile containing all installed plugins.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Lockfile {
    #[serde(default)]
    pub plugins: Vec<LockEntry>,
}

impl Lockfile {
    /// Load lockfile from a TOML file, or return empty if not found.
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("failed to read lockfile: {e}"))?;
        toml::from_str(&content).map_err(|e| format!("failed to parse lockfile: {e}"))
    }

    /// Save lockfile to a TOML file.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| format!("failed to serialize lockfile: {e}"))?;
        std::fs::write(path, content).map_err(|e| format!("failed to write lockfile: {e}"))
    }

    /// Find an installed plugin by ID.
    pub fn find(&self, plugin_id: &str) -> Option<&LockEntry> {
        self.plugins.iter().find(|p| p.id == plugin_id)
    }

    /// Add or update a plugin entry.
    pub fn upsert(&mut self, entry: LockEntry) {
        if let Some(existing) = self.plugins.iter_mut().find(|p| p.id == entry.id) {
            *existing = entry;
        } else {
            self.plugins.push(entry);
        }
    }

    /// Remove a plugin entry by ID.
    pub fn remove(&mut self, plugin_id: &str) -> Option<LockEntry> {
        if let Some(pos) = self.plugins.iter().position(|p| p.id == plugin_id) {
            Some(self.plugins.remove(pos))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_lockfile_loads_from_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("navi-plugins.lock");
        let lockfile = Lockfile::load(&path).unwrap();
        assert!(lockfile.plugins.is_empty());
    }

    #[test]
    fn roundtrip_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("navi-plugins.lock");

        let mut lockfile = Lockfile::default();
        lockfile.upsert(LockEntry {
            id: "test-plugin".into(),
            version: "1.0.0".into(),
            publisher: "gh:test".into(),
            wasm_hash: "sha256:abc123".into(),
            capabilities_hash: "sha256:def456".into(),
            tools_hash: "sha256:ghi789".into(),
            approved_capabilities: vec!["fs_read".into()],
            approved_at: "2026-06-01T00:00:00Z".into(),
        });

        lockfile.save(&path).unwrap();
        let loaded = Lockfile::load(&path).unwrap();
        assert_eq!(loaded.plugins.len(), 1);
        assert_eq!(loaded.plugins[0].id, "test-plugin");
    }

    #[test]
    fn find_existing_plugin() {
        let mut lockfile = Lockfile::default();
        lockfile.upsert(LockEntry {
            id: "my-plugin".into(),
            version: "0.1.0".into(),
            publisher: "gh:me".into(),
            wasm_hash: "sha256:a".into(),
            capabilities_hash: "sha256:b".into(),
            tools_hash: "sha256:c".into(),
            approved_capabilities: vec![],
            approved_at: "2026-06-01T00:00:00Z".into(),
        });

        assert!(lockfile.find("my-plugin").is_some());
        assert!(lockfile.find("other").is_none());
    }

    #[test]
    fn upsert_updates_existing() {
        let mut lockfile = Lockfile::default();
        lockfile.upsert(LockEntry {
            id: "p".into(),
            version: "1.0.0".into(),
            publisher: "gh:a".into(),
            wasm_hash: "sha256:old".into(),
            capabilities_hash: "sha256:old".into(),
            tools_hash: "sha256:old".into(),
            approved_capabilities: vec![],
            approved_at: "2026-01-01T00:00:00Z".into(),
        });
        lockfile.upsert(LockEntry {
            id: "p".into(),
            version: "2.0.0".into(),
            publisher: "gh:a".into(),
            wasm_hash: "sha256:new".into(),
            capabilities_hash: "sha256:new".into(),
            tools_hash: "sha256:new".into(),
            approved_capabilities: vec!["net".into()],
            approved_at: "2026-06-01T00:00:00Z".into(),
        });

        assert_eq!(lockfile.plugins.len(), 1);
        assert_eq!(lockfile.plugins[0].version, "2.0.0");
    }

    #[test]
    fn remove_plugin() {
        let mut lockfile = Lockfile::default();
        lockfile.upsert(LockEntry {
            id: "p".into(),
            version: "1.0.0".into(),
            publisher: "gh:a".into(),
            wasm_hash: "sha256:a".into(),
            capabilities_hash: "sha256:b".into(),
            tools_hash: "sha256:c".into(),
            approved_capabilities: vec![],
            approved_at: "2026-06-01T00:00:00Z".into(),
        });

        let removed = lockfile.remove("p");
        assert!(removed.is_some());
        assert!(lockfile.plugins.is_empty());
    }
}
