use crate::event::AgentEvent;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionId(pub String);

#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: SessionId,
    pub project: PathBuf,
    pub events: Vec<AgentEvent>,
}

impl SessionStore {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            root: data_dir.join("sessions"),
        }
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn create_id() -> SessionId {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        SessionId(format!("session-{millis}"))
    }

    pub fn save(&self, snapshot: &SessionSnapshot) -> Result<PathBuf> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("failed to create {}", self.root.display()))?;
        let path = self.root.join(format!("{}.json", snapshot.id.0));
        let data = serde_json::to_vec_pretty(snapshot)?;
        fs::write(&path, data).with_context(|| format!("failed to write {}", path.display()))?;
        Ok(path)
    }

    pub fn list(&self) -> Vec<SessionSnapshot> {
        let mut sessions = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        if let Ok(snapshot) = serde_json::from_str::<SessionSnapshot>(&content) {
                            sessions.push(snapshot);
                        }
                    }
                }
            }
        }
        sessions.sort_by(|a, b| b.id.0.cmp(&a.id.0));
        sessions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_writes_session_snapshot() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(tempdir.path().to_path_buf());
        let snapshot = SessionSnapshot {
            id: SessionId("test-session".to_string()),
            project: PathBuf::from("/tmp/project"),
            events: Vec::new(),
        };

        let path = store.save(&snapshot).expect("save session");
        assert!(path.exists());
        assert_eq!(path.file_name().unwrap(), "test-session.json");
    }
}
