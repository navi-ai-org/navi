use crate::event::AgentEvent;
use crate::security::redact_snapshot_events;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMemory {
    pub project_hash: String,
    pub entries: Vec<MemoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub created_at: u64,
    pub summary: String,
    pub session_id: String,
}

pub fn session_title_from_events(events: &[AgentEvent]) -> Option<String> {
    events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ModelOutput { text, .. } => title_from_model_text(text),
            _ => None,
        })
        .or_else(|| {
            events.iter().find_map(|event| match event {
                AgentEvent::UserTaskSubmitted { text } => title_from_user_text(text),
                _ => None,
            })
        })
}

fn title_from_model_text(text: &str) -> Option<String> {
    let heading = text.lines().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            Some(trimmed.trim_start_matches('#').trim())
        } else {
            None
        }
    });

    heading
        .and_then(clean_session_title)
        .or_else(|| text.lines().find_map(clean_session_title))
}

fn title_from_user_text(text: &str) -> Option<String> {
    clean_session_title(text)
}

fn clean_session_title(text: &str) -> Option<String> {
    let cleaned = text
        .trim()
        .trim_matches('`')
        .trim_matches('"')
        .trim_matches('\'')
        .trim_start_matches(['#', '-', '*', '>'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    if cleaned.is_empty() {
        return None;
    }

    Some(
        cleaned
            .chars()
            .take(80)
            .collect::<String>()
            .trim()
            .to_string(),
    )
}

impl ProjectMemory {
    pub fn recent_entries(&self, max: usize) -> &[MemoryEntry] {
        let start = self.entries.len().saturating_sub(max);
        &self.entries[start..]
    }

    pub fn format_injection(&self, max: usize) -> Option<String> {
        let entries = self.recent_entries(max);
        if entries.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        for entry in entries {
            parts.push(format!(
                "[Session {} — {}]\n{}",
                entry.session_id,
                format_timestamp(entry.created_at),
                entry.summary
            ));
        }
        Some(format!(
            "Previous session context (summarized):\n\n{}",
            parts.join("\n\n")
        ))
    }
}

fn format_timestamp(unix_secs: u64) -> String {
    let days = unix_secs / 86400;
    let hours = (unix_secs % 86400) / 3600;
    let minutes = (unix_secs % 3600) / 60;
    format!("day {days} {hours:02}:{minutes:02}")
}

fn project_hash(project_dir: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    project_dir.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
    data_dir: PathBuf,
    redact_secrets: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: SessionId,
    #[serde(default)]
    pub title: Option<String>,
    pub project: PathBuf,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
    pub events: Vec<AgentEvent>,
    #[serde(default)]
    pub memory: Option<ProjectMemory>,
}

impl SessionStore {
    pub fn new(data_dir: PathBuf) -> Self {
        Self::with_redaction(data_dir, true)
    }

    pub fn with_redaction(data_dir: PathBuf, redact_secrets: bool) -> Self {
        Self {
            root: data_dir.join("sessions"),
            data_dir,
            redact_secrets,
        }
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn create_id() -> SessionId {
        let millis = current_unix_millis();
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
                title: snapshot.title.clone(),
                project: snapshot.project.clone(),
                created_at: snapshot.created_at,
                updated_at: snapshot.updated_at,
                events: redact_snapshot_events(&snapshot.events),
                memory: snapshot.memory.clone(),
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
        sessions.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.id.0.cmp(&a.id.0))
        });
        sessions
    }

    pub fn save_memory(&self, project_dir: &Path, memory: &ProjectMemory) -> Result<PathBuf> {
        let memory_dir = self.data_dir.join("memory");
        fs::create_dir_all(&memory_dir)
            .with_context(|| format!("failed to create {}", memory_dir.display()))?;
        set_private_dir_permissions(&memory_dir)?;

        let hash = project_hash(project_dir);
        let path = memory_dir.join(format!("{hash}.json"));
        let data = serde_json::to_vec_pretty(memory)?;
        fs::write(&path, data).with_context(|| format!("failed to write {}", path.display()))?;
        set_private_file_permissions(&path)?;

        Ok(path)
    }

    pub fn load_memory(&self, project_dir: &Path) -> Option<ProjectMemory> {
        let hash = project_hash(project_dir);
        let path = self.data_dir.join("memory").join(format!("{hash}.json"));
        let content = fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    }

    pub fn add_memory_entry(
        &self,
        project_dir: &Path,
        session_id: &SessionId,
        summary: String,
    ) -> Result<PathBuf> {
        let mut memory = self.load_memory(project_dir).unwrap_or(ProjectMemory {
            project_hash: project_hash(project_dir),
            entries: Vec::new(),
        });
        memory.entries.push(MemoryEntry {
            created_at: current_unix_timestamp(),
            summary,
            session_id: session_id.0.clone(),
        });
        self.save_memory(project_dir, &memory)
    }
}

pub fn current_unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn current_unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

pub struct Submission {
    pub task: String,
    pub response_tx: tokio::sync::oneshot::Sender<Result<String>>,
}

#[derive(Clone)]
pub struct SessionRuntime {
    pub submission_tx: tokio::sync::mpsc::UnboundedSender<Submission>,
}

impl SessionRuntime {
    pub fn spawn(
        ctx: std::sync::Arc<crate::turn::TurnContext>,
        policy: crate::harness::HarnessPolicy,
        initial_messages: Vec<crate::model::ModelMessage>,
        memory_injection: Option<String>,
    ) -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Submission>();

        tokio::spawn(async move {
            let mut messages = if initial_messages.is_empty() {
                vec![crate::model::ModelMessage::system(
                    crate::harness::build_system_prompt_with_memory(
                        &crate::config::NaviConfig::default(),
                        &ctx.project_dir,
                        memory_injection.as_deref(),
                    ),
                )]
            } else {
                initial_messages
            };

            while let Some(submission) = rx.recv().await {
                messages.push(crate::model::ModelMessage::user(submission.task));
                let res = crate::turn::run_turn(&ctx, &mut messages, policy).await;
                let _ = submission.response_tx.send(res);
            }
        });

        Self { submission_tx: tx }
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
            title: Some("Test session".to_string()),
            project: PathBuf::from("/tmp/project"),
            created_at: 1,
            updated_at: 2,
            events: Vec::new(),
            memory: None,
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
            title: None,
            project: PathBuf::from("/tmp/project"),
            created_at: 1,
            updated_at: 2,
            events: Vec::new(),
            memory: None,
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
            title: None,
            project: PathBuf::from("/tmp/project"),
            created_at: 1,
            updated_at: 2,
            events: vec![AgentEvent::UserTaskSubmitted {
                text: "OPENAI_API_KEY=sk-proj-1234567890abcdef".to_string(),
            }],
            memory: None,
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
            title: None,
            project: PathBuf::from("/tmp/project"),
            created_at: 1,
            updated_at: 2,
            events: vec![AgentEvent::UserTaskSubmitted {
                text: "OPENAI_API_KEY=sk-proj-1234567890abcdef".to_string(),
            }],
            memory: None,
        };

        let path = store.save(&snapshot).expect("save session");
        let content = fs::read_to_string(path).expect("read session");

        assert!(content.contains("sk-proj-1234567890abcdef"));
    }

    struct MockProvider;

    #[async_trait::async_trait]
    impl crate::model::ModelProvider for MockProvider {
        fn stream(&self, _request: crate::model::ModelRequest) -> crate::model::ModelStream {
            Box::pin(futures_util::stream::iter(vec![
                Ok(crate::model::ModelStreamEvent::TextDelta {
                    text: "mock task response".to_string(),
                }),
                Ok(crate::model::ModelStreamEvent::Done),
            ]))
        }
    }

    #[tokio::test]
    async fn test_session_runtime_background_loop() {
        let tempdir = tempfile::tempdir().unwrap();
        let security_policy = crate::SecurityPolicy::new(
            tempdir.path().to_path_buf(),
            tempdir.path().to_path_buf(),
            crate::SecurityConfig::default(),
        )
        .unwrap();
        let tool_executor = std::sync::Arc::new(crate::ToolExecutor::new(security_policy));

        let ctx = std::sync::Arc::new(crate::turn::TurnContext {
            model_provider: std::sync::Arc::new(MockProvider),
            tool_executor,
            agent_control: crate::agent::AgentControl::new(),
            project_dir: tempdir.path().to_path_buf(),
            model_name: "test-model".to_string(),
            event_tx: None,
            pending_approvals: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            compact_state: std::sync::Arc::new(tokio::sync::Mutex::new(
                crate::compact::CompactState::new(128_000),
            )),
            harness_config: crate::config::HarnessConfig::default(),
            include_tool_prompt_manifest: false,
            agent_mode: None,
            context_packets: Vec::new(),
            active_skills: Vec::new(),
            cancel_requested: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            cancel_notify: std::sync::Arc::new(tokio::sync::Notify::new()),
        });

        let policy = crate::harness::HarnessPolicy {
            profile: crate::config::HarnessProfile::Small,
            observation_max_bytes: 1000,
        };

        let runtime = SessionRuntime::spawn(ctx, policy, Vec::new(), None);

        let (tx, rx) = tokio::sync::oneshot::channel();
        let submission = Submission {
            task: "hello world".to_string(),
            response_tx: tx,
        };

        runtime.submission_tx.send(submission).unwrap();

        let result = rx.await.unwrap().unwrap();
        assert_eq!(result, "mock task response");
    }

    #[test]
    fn project_memory_format_injection_returns_none_when_empty() {
        let memory = ProjectMemory {
            project_hash: "abc".to_string(),
            entries: Vec::new(),
        };
        assert!(memory.format_injection(3).is_none());
    }

    #[test]
    fn project_memory_format_injection_returns_latest_entries() {
        let memory = ProjectMemory {
            project_hash: "abc".to_string(),
            entries: vec![
                MemoryEntry {
                    created_at: 1000,
                    summary: "First session".to_string(),
                    session_id: "session-1".to_string(),
                },
                MemoryEntry {
                    created_at: 2000,
                    summary: "Second session".to_string(),
                    session_id: "session-2".to_string(),
                },
                MemoryEntry {
                    created_at: 3000,
                    summary: "Third session".to_string(),
                    session_id: "session-3".to_string(),
                },
                MemoryEntry {
                    created_at: 4000,
                    summary: "Fourth session".to_string(),
                    session_id: "session-4".to_string(),
                },
            ],
        };
        let injection = memory.format_injection(2).unwrap();
        assert!(injection.contains("Third session"));
        assert!(injection.contains("Fourth session"));
        assert!(!injection.contains("First session"));
        assert!(!injection.contains("Second session"));
    }

    #[test]
    fn save_and_load_memory_roundtrip() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(tempdir.path().to_path_buf());
        let project_dir = PathBuf::from("/tmp/test-project");

        let memory = ProjectMemory {
            project_hash: project_hash(&project_dir),
            entries: vec![MemoryEntry {
                created_at: 12345,
                summary: "Worked on auth module".to_string(),
                session_id: "session-test".to_string(),
            }],
        };

        store
            .save_memory(&project_dir, &memory)
            .expect("save memory");
        let loaded = store.load_memory(&project_dir).expect("load memory");
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].summary, "Worked on auth module");
    }

    #[test]
    fn add_memory_entry_appends_to_existing() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(tempdir.path().to_path_buf());
        let project_dir = PathBuf::from("/tmp/test-project-2");

        let session_id = SessionId("session-1".to_string());
        store
            .add_memory_entry(&project_dir, &session_id, "First summary".to_string())
            .expect("add entry 1");

        let session_id2 = SessionId("session-2".to_string());
        store
            .add_memory_entry(&project_dir, &session_id2, "Second summary".to_string())
            .expect("add entry 2");

        let loaded = store.load_memory(&project_dir).expect("load memory");
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].summary, "First summary");
        assert_eq!(loaded.entries[1].summary, "Second summary");
    }
}
