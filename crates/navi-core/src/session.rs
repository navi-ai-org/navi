use crate::event::AgentEvent;
use crate::security::redact_snapshot_events;
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
    redact_secrets: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: SessionId,
    pub project: PathBuf,
    pub events: Vec<AgentEvent>,
}

impl SessionStore {
    pub fn new(data_dir: PathBuf) -> Self {
        Self::with_redaction(data_dir, true)
    }

    pub fn with_redaction(data_dir: PathBuf, redact_secrets: bool) -> Self {
        Self {
            root: data_dir.join("sessions"),
            redact_secrets,
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
        set_private_dir_permissions(&self.root)?;

        let path = self.root.join(format!("{}.json", snapshot.id.0));
        let snapshot = if self.redact_secrets {
            SessionSnapshot {
                id: snapshot.id.clone(),
                project: snapshot.project.clone(),
                events: redact_snapshot_events(&snapshot.events),
            }
        } else {
            snapshot.clone()
        };
        let data = serde_json::to_vec_pretty(&snapshot)?;
        fs::write(&path, data).with_context(|| format!("failed to write {}", path.display()))?;
        set_private_file_permissions(&path)?;

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

#[cfg(unix)]
fn set_private_dir_permissions(path: &PathBuf) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to restrict {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &PathBuf) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &PathBuf) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to restrict {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &PathBuf) -> Result<()> {
    Ok(())
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

    #[cfg(unix)]
    #[test]
    fn save_restricts_session_file_and_directory_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tempdir = tempfile::tempdir().expect("tempdir");
        let data_dir = tempdir.path().join("navi-data");
        let store = SessionStore::new(data_dir);
        let snapshot = SessionSnapshot {
            id: SessionId("private-session".to_string()),
            project: PathBuf::from("/tmp/project"),
            events: Vec::new(),
        };

        let path = store.save(&snapshot).expect("save session");
        let dir_mode = fs::metadata(store.root())
            .expect("dir metadata")
            .permissions()
            .mode()
            & 0o777;
        let file_mode = fs::metadata(path)
            .expect("file metadata")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    #[test]
    fn save_redacts_secret_like_event_content() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(tempdir.path().to_path_buf());
        let snapshot = SessionSnapshot {
            id: SessionId("redacted-session".to_string()),
            project: PathBuf::from("/tmp/project"),
            events: vec![AgentEvent::UserTaskSubmitted {
                text: "OPENAI_API_KEY=sk-proj-1234567890abcdef".to_string(),
            }],
        };

        let path = store.save(&snapshot).expect("save session");
        let content = fs::read_to_string(path).expect("read session");

        assert!(content.contains("OPENAI_API_KEY=<redacted>"));
        assert!(!content.contains("sk-proj-1234567890abcdef"));
    }

    #[test]
    fn save_can_preserve_event_content_when_redaction_is_disabled() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::with_redaction(tempdir.path().to_path_buf(), false);
        let snapshot = SessionSnapshot {
            id: SessionId("unredacted-session".to_string()),
            project: PathBuf::from("/tmp/project"),
            events: vec![AgentEvent::UserTaskSubmitted {
                text: "OPENAI_API_KEY=sk-proj-1234567890abcdef".to_string(),
            }],
        };

        let path = store.save(&snapshot).expect("save session");
        let content = fs::read_to_string(path).expect("read session");

        assert!(content.contains("sk-proj-1234567890abcdef"));
    }
}
