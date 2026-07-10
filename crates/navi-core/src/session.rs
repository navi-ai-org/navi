use crate::event::AgentEvent;
use crate::security::{redact_memory, redact_snapshot_events};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::task;

/// Unique identifier for a session, wrapping a string id like `"session-1719612345000"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    /// Creates a new `SessionId` from the given string.
    pub fn new(id: String) -> Self {
        Self(id)
    }

    /// Returns the id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the id and returns the inner string.
    pub fn into_inner(self) -> String {
        self.0
    }
}

/// Accumulated session memory for a project, used to inject past context into new sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMemory {
    /// Hash identifying the project directory.
    pub project_hash: String,
    /// Ordered memory entries from past sessions.
    pub entries: Vec<MemoryEntry>,
}

/// A single memory entry from a completed session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unix timestamp (seconds) when this entry was created.
    pub created_at: u64,
    /// Short summary of what the session covered.
    pub summary: String,
    /// Identifier of the originating session.
    pub session_id: String,
}

/// Derives a short title from session events by looking for a markdown heading
/// in the first model output, falling back to a cleaned version of the first
/// user task text.
///
/// Returns `None` if no suitable text is found.
pub fn session_title_from_events(events: &[AgentEvent]) -> Option<String> {
    events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ModelOutput { text, .. } => title_from_model_text(text),
            _ => None,
        })
        .or_else(|| {
            events.iter().find_map(|event| match event {
                AgentEvent::UserTaskSubmitted { text, .. } => title_from_user_text(text),
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

/// Sanitizes text into a short session title by trimming whitespace, quotes,
/// markdown markers, and truncating to 80 characters.
///
/// Returns `None` if the cleaned text is empty.
pub fn clean_session_title(text: &str) -> Option<String> {
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

/// Generates a session title using a model provider.
///
/// Sends the first user message and assistant response to the model with a
/// prompt asking for a concise title. Returns `None` if the model call fails
/// or produces empty output.
pub async fn generate_session_title(
    user_message: &str,
    assistant_response: &str,
    model_provider: &dyn crate::model::ModelProvider,
    model_name: &str,
) -> Option<String> {
    let prompt = format!(
        "Generate a concise session title (max 60 chars) for this conversation. \
         Return ONLY the title, no quotes or formatting.\n\n\
         User: {}\nAssistant: {}",
        truncate_for_title(user_message, 500),
        truncate_for_title(assistant_response, 500),
    );

    let request = crate::model::ModelRequest {
        model: model_name.to_string(),
        instructions: None,
        messages: vec![
            crate::model::ModelMessage::system(
                "You are a title generator. Return only a short, descriptive title.",
            ),
            crate::model::ModelMessage::user(prompt),
        ],
        thinking: crate::model::ThinkingConfig::Off,
        tools: vec![],
    };

    match model_provider.complete(request).await {
        Ok(response) => clean_session_title(&response.text),
        Err(err) => {
            tracing::warn!(error = %err, "session title generation failed");
            None
        }
    }
}

fn truncate_for_title(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let truncated: String = text.chars().take(max_chars).collect();
        format!("{truncated}...")
    }
}

impl ProjectMemory {
    /// Returns at most `max` of the most recent memory entries.
    pub fn recent_entries(&self, max: usize) -> &[MemoryEntry] {
        let start = self.entries.len().saturating_sub(max);
        &self.entries[start..]
    }

    /// Formats up to `max` recent entries into a text block suitable for
    /// injection into the system prompt. Returns `None` if there are no entries.
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

/// Persists [`SessionSnapshot`] JSON files to disk under `<data_dir>/sessions/`.
///
/// By default, secret redaction is enabled so API keys and tokens are scrubbed
/// from saved event text.
#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
    data_dir: PathBuf,
    redact_secrets: bool,
}

fn default_session_version() -> u32 {
    1
}

/// Accumulated token/cost usage for a session (persisted with the snapshot).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SessionUsageSnapshot {
    /// Cumulative prompt/context tokens billed this session.
    #[serde(default)]
    pub input_tokens: u64,
    /// Cumulative completion tokens billed this session.
    #[serde(default)]
    pub output_tokens: u64,
    /// Estimated spend in USD from list rates × tokens (when known).
    #[serde(default)]
    pub cost_usd: f64,
    /// True once at least one turn had usable list pricing.
    #[serde(default)]
    pub cost_known: bool,
    /// Estimated prepaid credits spent (e.g. Hypercredits = USD / $0.05).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits_spent: Option<f64>,
    /// Credit unit label when `credits_spent` is set (e.g. `hypercredits`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_unit: Option<String>,
}

/// A serializable snapshot of a complete session, persisted to disk as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Snapshot schema version; currently `1`.
    #[serde(default = "default_session_version")]
    pub version: u32,
    /// Unique session identifier.
    pub id: SessionId,
    /// Short human-readable title, derived from the first user/assistant message.
    #[serde(default)]
    pub title: Option<String>,
    /// Project directory this session belongs to.
    pub project: PathBuf,
    /// Unix timestamp (seconds) when the session was created.
    #[serde(default)]
    pub created_at: u64,
    /// Unix timestamp (seconds) when the session was last updated.
    #[serde(default)]
    pub updated_at: u64,
    /// All agent events recorded during the session.
    pub events: Vec<AgentEvent>,
    /// Optional project memory snapshot co-persisted with the session.
    #[serde(default)]
    pub memory: Option<ProjectMemory>,
    /// Optional session goal co-persisted with the session.
    #[serde(default)]
    pub goal: Option<SessionGoal>,
    /// Token and estimated cost usage for this session (restored on reload).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<SessionUsageSnapshot>,
}

/// Lightweight metadata for listing saved sessions without loading event history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshotInfo {
    /// Unique session identifier.
    pub id: SessionId,
    /// Short human-readable title, derived when the snapshot was saved.
    #[serde(default)]
    pub title: Option<String>,
    /// Project directory this session belongs to.
    pub project: PathBuf,
    /// Unix timestamp (seconds) when the session was created.
    #[serde(default)]
    pub created_at: u64,
    /// Unix timestamp (seconds) when the session was last updated.
    #[serde(default)]
    pub updated_at: u64,
}

impl SessionSnapshot {
    /// Current snapshot schema version.
    pub const CURRENT_VERSION: u32 = 1;
}

fn read_session_info(path: &Path) -> Result<SessionSnapshotInfo> {
    const METADATA_READ_LIMIT: usize = 64 * 1024;

    let mut file =
        fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut buffer = vec![0; METADATA_READ_LIMIT];
    let bytes_read = file
        .read(&mut buffer)
        .with_context(|| format!("failed to read {}", path.display()))?;
    buffer.truncate(bytes_read);

    let prefix = std::str::from_utf8(&buffer)
        .with_context(|| format!("failed to decode metadata prefix from {}", path.display()))?;

    if let Some(events_index) = prefix.find("\"events\"")
        && let Some(comma_index) = prefix[..events_index].rfind(',')
    {
        let metadata_json = format!("{}\n}}", &prefix[..comma_index]);
        return serde_json::from_str::<SessionSnapshotInfo>(&metadata_json)
            .with_context(|| format!("failed to parse metadata from {}", path.display()));
    }

    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str::<SessionSnapshotInfo>(&content)
        .with_context(|| format!("failed to parse metadata from {}", path.display()))
}

impl SessionStore {
    /// Creates a new store with secret redaction enabled.
    pub fn new(data_dir: PathBuf) -> Self {
        Self::with_redaction(data_dir, true)
    }

    /// Creates a new store with the given redaction setting.
    pub fn with_redaction(data_dir: PathBuf, redact_secrets: bool) -> Self {
        Self {
            root: data_dir.join("sessions"),
            data_dir,
            redact_secrets,
        }
    }

    /// Returns the directory where session JSON files are stored.
    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    /// Generates a new `SessionId` based on the current Unix timestamp in milliseconds.
    pub fn create_id() -> SessionId {
        let millis = current_unix_millis();
        SessionId::new(format!("session-{millis}"))
    }

    /// Serializes and saves a snapshot to disk, creating the sessions directory
    /// if needed. Applies secret redaction unless disabled.
    ///
    /// This is the blocking implementation used internally and in tests. Use
    /// [`Self::save_async`] from async contexts to avoid blocking the Tokio
    /// runtime.
    pub fn save(&self, snapshot: &SessionSnapshot) -> Result<PathBuf> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("failed to create {}", self.root.display()))?;
        crate::fs_util::set_private_dir_permissions(&self.root)?;

        let path = self.root.join(format!("{}.json", snapshot.id.as_str()));
        let snapshot = if self.redact_secrets {
            SessionSnapshot {
                version: snapshot.version,
                id: snapshot.id.clone(),
                title: snapshot.title.clone(),
                project: snapshot.project.clone(),
                created_at: snapshot.created_at,
                updated_at: snapshot.updated_at,
                goal: snapshot.goal.clone(),
                events: redact_snapshot_events(&snapshot.events),
                memory: snapshot.memory.as_ref().map(redact_memory),
                usage: snapshot.usage.clone(),
            }
        } else {
            snapshot.clone()
        };
        let data = serde_json::to_vec_pretty(&snapshot)?;
        fs::write(&path, data).with_context(|| format!("failed to write {}", path.display()))?;
        crate::fs_util::set_private_file_permissions(&path)?;

        Ok(path)
    }

    /// Async wrapper around [`Self::save`] that runs the blocking filesystem
    /// operations on the Tokio blocking thread pool.
    pub async fn save_async(&self, snapshot: SessionSnapshot) -> Result<PathBuf> {
        let store = self.clone();
        task::spawn_blocking(move || store.save(&snapshot))
            .await
            .map_err(|err| anyhow::anyhow!("save_async join error: {err}"))?
    }

    /// Loads all saved sessions from disk, sorted by most recently updated first.
    pub fn list(&self) -> Vec<SessionSnapshot> {
        let mut sessions = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json")
                    && let Ok(content) = fs::read_to_string(&path)
                    && let Ok(snapshot) = serde_json::from_str::<SessionSnapshot>(&content)
                {
                    sessions.push(snapshot);
                }
            }
        }
        sessions.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.id.as_str().cmp(a.id.as_str()))
        });
        sessions
    }

    /// Loads only session metadata from disk, sorted by most recently updated first.
    pub fn list_info(&self) -> Vec<SessionSnapshotInfo> {
        let mut sessions = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json")
                    && let Ok(info) = read_session_info(&path)
                {
                    sessions.push(info);
                }
            }
        }
        sessions.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.id.as_str().cmp(a.id.as_str()))
        });
        sessions
    }

    /// Async wrapper around [`Self::list`] that runs the blocking filesystem
    /// operations on the Tokio blocking thread pool.
    pub async fn list_async(&self) -> Vec<SessionSnapshot> {
        let store = self.clone();
        task::spawn_blocking(move || store.list())
            .await
            .unwrap_or_default()
    }

    /// Async wrapper around [`Self::list_info`] that avoids blocking the async runtime.
    pub async fn list_info_async(&self) -> Vec<SessionSnapshotInfo> {
        let store = self.clone();
        task::spawn_blocking(move || store.list_info())
            .await
            .unwrap_or_default()
    }

    /// Loads a single session by id. Returns an error if the file is missing or
    /// the snapshot version is newer than supported.
    pub fn load(&self, session_id: &str) -> Result<SessionSnapshot> {
        let path = self.root.join(format!("{session_id}.json"));
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let snapshot: SessionSnapshot = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        if snapshot.version > SessionSnapshot::CURRENT_VERSION {
            return Err(anyhow::anyhow!(
                "session snapshot version {} is newer than supported version {}",
                snapshot.version,
                SessionSnapshot::CURRENT_VERSION
            ));
        }
        Ok(snapshot)
    }

    /// Async wrapper around [`Self::load`] that runs the blocking filesystem
    /// operations on the Tokio blocking thread pool.
    pub async fn load_async(&self, session_id: String) -> Result<SessionSnapshot> {
        let store = self.clone();
        task::spawn_blocking(move || store.load(&session_id))
            .await
            .map_err(|err| anyhow::anyhow!("load_async join error: {err}"))?
    }

    /// Deletes the session file. Returns `true` if the file existed and was removed.
    pub fn delete(&self, session_id: &str) -> Result<bool> {
        let path = self.root.join(format!("{session_id}.json"));
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(err).with_context(|| format!("failed to delete {}", path.display())),
        }
    }

    /// Renames a saved session by updating its title field in the snapshot.
    /// Returns `true` if the session existed and was updated.
    pub fn rename(&self, session_id: &str, title: &str) -> Result<bool> {
        let title = title.trim();
        if title.is_empty() {
            return Err(anyhow::anyhow!("session title cannot be empty"));
        }
        let path = self.root.join(format!("{session_id}.json"));
        if !path.exists() {
            return Ok(false);
        }
        let mut snapshot = self.load(session_id)?;
        snapshot.title = Some(title.to_string());
        snapshot.updated_at = current_unix_timestamp();
        self.save(&snapshot)?;
        Ok(true)
    }

    /// Async wrapper around [`Self::rename`].
    pub async fn rename_async(&self, session_id: String, title: String) -> Result<bool> {
        let store = self.clone();
        task::spawn_blocking(move || store.rename(&session_id, &title))
            .await
            .map_err(|err| anyhow::anyhow!("rename_async join error: {err}"))?
    }

    /// Async wrapper around [`Self::delete`] that runs the blocking filesystem
    /// operations on the Tokio blocking thread pool.
    pub async fn delete_async(&self, session_id: String) -> Result<bool> {
        let store = self.clone();
        task::spawn_blocking(move || store.delete(&session_id))
            .await
            .map_err(|err| anyhow::anyhow!("delete_async join error: {err}"))?
    }

    /// Persists project memory to `<data_dir>/memory/<hash>.json`.
    pub fn save_memory(&self, project_dir: &Path, memory: &ProjectMemory) -> Result<PathBuf> {
        let memory_dir = self.data_dir.join("memory");
        fs::create_dir_all(&memory_dir)
            .with_context(|| format!("failed to create {}", memory_dir.display()))?;
        crate::fs_util::set_private_dir_permissions(&memory_dir)?;

        let hash = project_hash(project_dir);
        let path = memory_dir.join(format!("{hash}.json"));
        let data = serde_json::to_vec_pretty(memory)?;
        fs::write(&path, data).with_context(|| format!("failed to write {}", path.display()))?;
        crate::fs_util::set_private_file_permissions(&path)?;

        Ok(path)
    }

    /// Async wrapper around [`Self::save_memory`] that runs the blocking
    /// filesystem operations on the Tokio blocking thread pool.
    pub async fn save_memory_async(
        &self,
        project_dir: PathBuf,
        memory: ProjectMemory,
    ) -> Result<PathBuf> {
        let store = self.clone();
        task::spawn_blocking(move || store.save_memory(&project_dir, &memory))
            .await
            .map_err(|err| anyhow::anyhow!("save_memory_async join error: {err}"))?
    }

    /// Loads project memory from disk, returning `None` if no memory file exists.
    pub fn load_memory(&self, project_dir: &Path) -> Option<ProjectMemory> {
        let hash = project_hash(project_dir);
        let path = self.data_dir.join("memory").join(format!("{hash}.json"));
        let content = fs::read_to_string(&path).ok()?;
        match serde_json::from_str(&content) {
            Ok(memory) => Some(memory),
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "failed to parse project memory file"
                );
                None
            }
        }
    }

    /// Async wrapper around [`Self::load_memory`] that runs the blocking
    /// filesystem operations on the Tokio blocking thread pool.
    pub async fn load_memory_async(&self, project_dir: PathBuf) -> Option<ProjectMemory> {
        let store = self.clone();
        task::spawn_blocking(move || store.load_memory(&project_dir))
            .await
            .ok()
            .flatten()
    }

    /// Appends a new memory entry for the project and persists it to disk.
    pub fn add_memory_entry(
        &self,
        project_dir: &Path,
        session_id: &SessionId,
        summary: String,
    ) -> Result<PathBuf> {
        let hash = project_hash(project_dir);
        let path = self.data_dir.join("memory").join(format!("{hash}.json"));

        // Retry loop to handle concurrent writes from other NAVI instances.
        // If the file changes between load and save (detected via mtime), we
        // reload and retry instead of overwriting and losing entries.
        for _attempt in 0..3 {
            let mtime_before = fs::metadata(&path).ok().and_then(|m| m.modified().ok());
            let mut memory = self.load_memory(project_dir).unwrap_or(ProjectMemory {
                project_hash: project_hash(project_dir),
                entries: Vec::new(),
            });
            memory.entries.push(crate::session::MemoryEntry {
                created_at: current_unix_timestamp(),
                summary: summary.clone(),
                session_id: session_id.as_str().to_string(),
            });
            let data = serde_json::to_vec_pretty(&memory)?;

            // Check if the file changed while we were preparing
            let mtime_after = fs::metadata(&path).ok().and_then(|m| m.modified().ok());
            if mtime_before != mtime_after && mtime_before.is_some() {
                // File was modified by another process — retry
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }

            // Atomic write via temp file + rename
            if let Some(parent) = path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let tmp = path.with_extension("json.tmp");
            fs::write(&tmp, data)?;
            fs::rename(&tmp, &path)?;
            crate::fs_util::set_private_file_permissions(&path)?;
            return Ok(path);
        }
        anyhow::bail!("failed to add memory entry after 3 retries (concurrent write conflict)");
    }

    /// Async wrapper around [`Self::add_memory_entry`] that runs the blocking
    /// filesystem operations on the Tokio blocking thread pool.
    pub async fn add_memory_entry_async(
        &self,
        project_dir: PathBuf,
        session_id: String,
        summary: String,
    ) -> Result<PathBuf> {
        let store = self.clone();
        task::spawn_blocking(move || {
            let sid = SessionId::new(session_id);
            store.add_memory_entry(&project_dir, &sid, summary)
        })
        .await
        .map_err(|err| anyhow::anyhow!("add_memory_entry_async join error: {err}"))?
    }
}

/// Returns the current time as a Unix timestamp in seconds.
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

use crate::goal::types::SessionGoal;
use crate::model::ContentPart;

/// A user task submission sent to the session background loop.
pub struct Submission {
    /// The user's task text.
    pub task: String,
    /// Optional multimodal content parts (images + text).
    /// When non-empty, the session loop creates a multimodal user message.
    pub content_parts: Vec<ContentPart>,
    /// Channel to send the assistant's response back to the caller.
    pub response_tx: tokio::sync::oneshot::Sender<Result<String>>,
}

/// Commands accepted by the session background loop.
pub enum SessionCommand {
    /// Run a full agent turn for a user message.
    Turn(Submission),
    /// Drop conversation history after `keep_user_turns` user messages
    /// (and the assistant/tool messages belonging to those turns).
    /// Used when the UI edits a past user message and re-sends.
    TruncateToUserTurns {
        keep_user_turns: usize,
        response_tx: tokio::sync::oneshot::Sender<Result<usize>>,
    },
}

/// Truncate model history so only the first `keep_user_turns` user turns remain.
///
/// System/developer preamble is always kept. The cut point is the start of the
/// `(keep_user_turns + 1)`-th user message (0-based count of user messages kept).
pub fn truncate_messages_to_user_turns(
    messages: &mut Vec<crate::model::ModelMessage>,
    keep_user_turns: usize,
) {
    use crate::model::ModelRole;
    let mut seen_users = 0usize;
    let mut cut: Option<usize> = None;
    for (i, msg) in messages.iter().enumerate() {
        if msg.role == ModelRole::User {
            if seen_users == keep_user_turns {
                cut = Some(i);
                break;
            }
            seen_users += 1;
        }
    }
    if let Some(i) = cut {
        messages.truncate(i);
    }
}

/// A handle to a background session loop that accepts [`SessionCommand`]s and
/// runs them through the turn pipeline.
#[derive(Clone)]
pub struct SessionRuntime {
    /// Channel for sending commands to the background loop.
    pub submission_tx: tokio::sync::mpsc::UnboundedSender<SessionCommand>,
}

impl SessionRuntime {
    /// Spawns a background tokio task that processes submissions sequentially
    /// through the turn pipeline, maintaining conversation history.
    pub fn spawn(
        ctx: std::sync::Arc<crate::turn::TurnContext>,
        policy: crate::harness::HarnessPolicy,
        initial_messages: Vec<crate::model::ModelMessage>,
        _memory_injection: Option<String>,
    ) -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SessionCommand>();

        tokio::spawn(async move {
            let mut messages = initial_messages;

            while let Some(command) = rx.recv().await {
                match command {
                    SessionCommand::Turn(submission) => {
                        if submission.content_parts.is_empty() {
                            messages.push(crate::model::ModelMessage::user(submission.task));
                        } else {
                            messages.push(crate::model::ModelMessage::user_multimodal(
                                submission.task,
                                submission.content_parts,
                            ));
                        }
                        let res = crate::turn::run_turn(&ctx, &mut messages, policy).await;
                        let _ = submission.response_tx.send(res);
                    }
                    SessionCommand::TruncateToUserTurns {
                        keep_user_turns,
                        response_tx,
                    } => {
                        truncate_messages_to_user_turns(&mut messages, keep_user_turns);
                        let _ = response_tx.send(Ok(messages.len()));
                    }
                }
            }
        });

        Self { submission_tx: tx }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{ToolInvocation, ToolResult};

    #[test]
    fn save_writes_session_snapshot() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(tempdir.path().to_path_buf());
        let snapshot = SessionSnapshot {
            version: SessionSnapshot::CURRENT_VERSION,
            id: SessionId::new("test-session".to_string()),
            title: Some("Test session".to_string()),
            project: PathBuf::from("/tmp/project"),
            created_at: 1,
            updated_at: 2,
            events: Vec::new(),
            memory: None,
            goal: None,
            usage: None,
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
            version: SessionSnapshot::CURRENT_VERSION,
            id: SessionId::new("private-session".to_string()),
            title: None,
            project: PathBuf::from("/tmp/project"),
            created_at: 1,
            updated_at: 2,
            events: Vec::new(),
            memory: None,
            goal: None,
            usage: None,
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
            version: SessionSnapshot::CURRENT_VERSION,
            id: SessionId::new("redacted-session".to_string()),
            title: None,
            project: PathBuf::from("/tmp/project"),
            created_at: 1,
            updated_at: 2,
            events: vec![AgentEvent::UserTaskSubmitted {
                text: "OPENAI_API_KEY=sk-proj-1234567890abcdef".to_string(),
                content_parts: vec![],
                submitted_at: None,
            }],
            memory: None,
            goal: None,
            usage: None,
        };

        let path = store.save(&snapshot).expect("save session");
        let content = fs::read_to_string(path).expect("read session");

        assert!(content.contains("OPENAI_API_KEY=<redacted>"));
        assert!(!content.contains("sk-proj-1234567890abcdef"));
    }

    #[test]
    fn save_redacts_secret_like_memory_summaries() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(tempdir.path().to_path_buf());
        let snapshot = SessionSnapshot {
            version: SessionSnapshot::CURRENT_VERSION,
            id: SessionId::new("redacted-memory-session".to_string()),
            title: None,
            project: PathBuf::from("/tmp/project"),
            created_at: 1,
            updated_at: 2,
            events: Vec::new(),
            memory: Some(ProjectMemory {
                project_hash: "abc".to_string(),
                entries: vec![MemoryEntry {
                    created_at: 1_700_000_000,
                    summary: "Configured with OPENAI_API_KEY=sk-proj-abcdef0123456789".to_string(),
                    session_id: "session-x".to_string(),
                }],
            }),
            goal: None,
            usage: None,
        };

        let path = store.save(&snapshot).expect("save session");
        let content = fs::read_to_string(path).expect("read session");

        assert!(content.contains("OPENAI_API_KEY=<redacted>"));
        assert!(!content.contains("sk-proj-abcdef0123456789"));
    }

    #[test]
    fn save_can_preserve_event_content_when_redaction_is_disabled() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::with_redaction(tempdir.path().to_path_buf(), false);
        let snapshot = SessionSnapshot {
            version: SessionSnapshot::CURRENT_VERSION,
            id: SessionId::new("unredacted-session".to_string()),
            title: None,
            project: PathBuf::from("/tmp/project"),
            created_at: 1,
            updated_at: 2,
            events: vec![AgentEvent::UserTaskSubmitted {
                text: "OPENAI_API_KEY=sk-proj-1234567890abcdef".to_string(),
                content_parts: vec![],
                submitted_at: None,
            }],
            memory: None,
            goal: None,
            usage: None,
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
            model_provider: std::sync::Arc::new(std::sync::RwLock::new(std::sync::Arc::new(
                MockProvider,
            ))),
            tool_executor,
            project_dir: tempdir.path().to_path_buf(),
            data_dir: tempdir.path().join("data"),
            model_name: std::sync::Arc::new(std::sync::RwLock::new("test-model".to_string())),
            event_tx: None,
            approval_resolver: crate::runtime::ApprovalResolver::new_for_test(),
            question_resolver: crate::runtime::QuestionResolver::new_for_test(),
            plan_review_resolver: crate::runtime::PlanReviewResolver::new_for_test(),
            sudo_password_resolver: crate::runtime::SudoPasswordResolver::new_for_test(),
            compact_state: std::sync::Arc::new(tokio::sync::Mutex::new(
                crate::compact::CompactState::new(128_000),
            )),
            harness_config: crate::config::HarnessConfig::default(),
            include_tool_prompt_manifest: false,
            context_packets: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            available_skills: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            active_skills: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            prompt_cache: std::sync::Arc::new(crate::prompt::PromptCache::new()),
            instructions: std::sync::Arc::new(std::sync::RwLock::new(None)),
            components: crate::RuntimeComponents::default(),
            cancel_token: crate::cancel::CancelToken::new(),
            config: std::sync::Arc::new(std::sync::RwLock::new(
                crate::config::NaviConfig::default(),
            )),
            memory_injection: None,
            compaction_provider: None,
            agent_mode: crate::plan_mode::AgentMode::Default,
            compaction_model_name: None,
            session_id: "test-session".to_string(),
            allowed_tool_names: None,
            memory_manager: std::sync::Arc::new(std::sync::Mutex::new(None)),
        });

        let policy = crate::harness::policy_for_profile(
            &crate::config::HarnessConfig {
                observation_bytes_small: 1000,
                ..crate::config::HarnessConfig::default()
            },
            crate::config::HarnessProfile::Small,
        );

        let runtime = SessionRuntime::spawn(ctx, policy, Vec::new(), None);

        let (tx, rx) = tokio::sync::oneshot::channel();
        let submission = SessionCommand::Turn(Submission {
            task: "hello world".to_string(),
            content_parts: Vec::new(),
            response_tx: tx,
        });

        runtime.submission_tx.send(submission).unwrap();

        let result = rx.await.unwrap().unwrap();
        assert_eq!(result, "mock task response");
    }

    #[test]
    fn truncate_messages_keeps_preamble_and_prior_turns() {
        use crate::model::{ModelMessage, ModelRole};
        let mut messages = vec![
            ModelMessage::system("sys"),
            ModelMessage::developer("dev"),
            ModelMessage::user("u1"),
            ModelMessage {
                role: ModelRole::Assistant,
                content: "a1".into(),
                content_parts: vec![],
                tool_call_id: None,
                tool_name: None,
                tool_calls: vec![],
                created_at: None,
                thinking_content: None,
            },
            ModelMessage::user("u2"),
            ModelMessage {
                role: ModelRole::Assistant,
                content: "a2".into(),
                content_parts: vec![],
                tool_call_id: None,
                tool_name: None,
                tool_calls: vec![],
                created_at: None,
                thinking_content: None,
            },
            ModelMessage::user("u3"),
        ];
        truncate_messages_to_user_turns(&mut messages, 1);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[2].content, "u1");
        assert_eq!(messages[3].content, "a1");

        truncate_messages_to_user_turns(&mut messages, 0);
        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[0].role, ModelRole::System));
        assert!(matches!(messages[1].role, ModelRole::Developer));
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

        let session_id = SessionId::new("session-1".to_string());
        store
            .add_memory_entry(&project_dir, &session_id, "First summary".to_string())
            .expect("add entry 1");

        let session_id2 = SessionId::new("session-2".to_string());
        store
            .add_memory_entry(&project_dir, &session_id2, "Second summary".to_string())
            .expect("add entry 2");

        let loaded = store.load_memory(&project_dir).expect("load memory");
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].summary, "First summary");
        assert_eq!(loaded.entries[1].summary, "Second summary");
    }

    fn make_snapshot(id: &str, updated_at: u64) -> SessionSnapshot {
        SessionSnapshot {
            version: SessionSnapshot::CURRENT_VERSION,
            id: SessionId::new(id.to_string()),
            title: Some(format!("Session {id}")),
            project: PathBuf::from("/tmp/project"),
            created_at: updated_at - 10,
            updated_at,
            events: Vec::new(),
            memory: None,
            goal: None,
            usage: None,
        }
    }

    #[test]
    fn list_returns_sessions_sorted_by_updated_at() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::with_redaction(tempdir.path().to_path_buf(), false);
        store.save(&make_snapshot("s-old", 100)).expect("save");
        store.save(&make_snapshot("s-new", 300)).expect("save");
        store.save(&make_snapshot("s-mid", 200)).expect("save");

        let sessions = store.list();
        assert_eq!(sessions.len(), 3);
        assert_eq!(sessions[0].id.as_str(), "s-new");
        assert_eq!(sessions[1].id.as_str(), "s-mid");
        assert_eq!(sessions[2].id.as_str(), "s-old");
    }

    #[test]
    fn list_returns_empty_when_no_sessions() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(tempdir.path().to_path_buf());
        assert!(store.list().is_empty());
    }

    #[test]
    fn load_roundtrip_save_then_load() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::with_redaction(tempdir.path().to_path_buf(), false);
        let snapshot = make_snapshot("roundtrip-1", 500);
        store.save(&snapshot).expect("save");

        let loaded = store.load("roundtrip-1").expect("load");
        assert_eq!(loaded.id.as_str(), "roundtrip-1");
        assert_eq!(loaded.title, Some("Session roundtrip-1".to_string()));
        assert_eq!(loaded.updated_at, 500);
    }

    #[test]
    fn load_rejects_unsupported_version() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::with_redaction(tempdir.path().to_path_buf(), false);
        let mut snapshot = make_snapshot("future-session", 100);
        snapshot.version = 999;
        store.save(&snapshot).expect("save");

        let result = store.load("future-session");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("version"), "expected version error: {err}");
    }

    #[test]
    fn delete_removes_session_file() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::with_redaction(tempdir.path().to_path_buf(), false);
        store.save(&make_snapshot("del-1", 100)).expect("save");
        assert!(store.root().join("del-1.json").exists());

        let deleted = store.delete("del-1").expect("delete");
        assert!(deleted);
        assert!(!store.root().join("del-1.json").exists());
    }

    #[test]
    fn delete_returns_false_for_missing() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(tempdir.path().to_path_buf());
        let deleted = store.delete("nonexistent").expect("delete");
        assert!(!deleted);
    }

    #[test]
    fn session_snapshot_serialization_roundtrip() {
        let snapshot = SessionSnapshot {
            version: SessionSnapshot::CURRENT_VERSION,
            id: SessionId::new("ser-1".to_string()),
            title: Some("Test".to_string()),
            project: PathBuf::from("/tmp/p"),
            created_at: 1000,
            updated_at: 2000,
            events: vec![
                AgentEvent::UserTaskSubmitted {
                    text: "hello".to_string(),
                    content_parts: vec![],
                    submitted_at: None,
                },
                AgentEvent::ModelOutput {
                    text: "response".to_string(),
                    thinking: Some("reasoning".to_string()),
                },
            ],
            memory: None,
            goal: None,
            usage: None,
        };
        let json = serde_json::to_string(&snapshot).expect("serialize");
        let loaded: SessionSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(loaded.id.as_str(), "ser-1");
        assert_eq!(loaded.events.len(), 2);
    }

    #[test]
    fn session_title_from_events_prefers_model_heading() {
        let events = vec![
            AgentEvent::UserTaskSubmitted {
                text: "do something".to_string(),
                content_parts: vec![],
                submitted_at: None,
            },
            AgentEvent::ModelOutput {
                text: "# My Analysis\n\nSome content here".to_string(),
                thinking: None,
            },
        ];
        let title = session_title_from_events(&events);
        assert_eq!(title.as_deref(), Some("My Analysis"));
    }

    #[test]
    fn session_title_from_events_falls_back_to_user_text() {
        let events = vec![AgentEvent::UserTaskSubmitted {
            text: "Fix the bug".to_string(),
            content_parts: vec![],
            submitted_at: None,
        }];
        let title = session_title_from_events(&events);
        assert_eq!(title.as_deref(), Some("Fix the bug"));
    }

    #[test]
    fn clean_session_title_strips_markdown_and_truncates() {
        assert_eq!(clean_session_title("## Short"), Some("Short".to_string()));
        assert_eq!(
            clean_session_title("`code snippet`"),
            Some("code snippet".to_string())
        );
        let long = "a".repeat(200);
        let result = clean_session_title(&long).unwrap();
        assert!(result.len() <= 80);
    }

    #[test]
    fn clean_session_title_returns_none_for_empty() {
        assert!(clean_session_title("").is_none());
        assert!(clean_session_title("###").is_none());
    }

    #[test]
    fn save_and_load_preserves_events() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::with_redaction(tempdir.path().to_path_buf(), false);
        let snapshot = SessionSnapshot {
            version: SessionSnapshot::CURRENT_VERSION,
            id: SessionId::new("events-session".to_string()),
            title: None,
            project: PathBuf::from("/tmp/p"),
            created_at: 10,
            updated_at: 20,
            events: vec![
                AgentEvent::UserTaskSubmitted {
                    text: "task".to_string(),
                    content_parts: vec![],
                    submitted_at: None,
                },
                AgentEvent::ToolRequested(ToolInvocation {
                    id: "c1".to_string(),
                    tool_name: "read_file".to_string(),
                    input: serde_json::json!({"path": "x.txt"}),
                }),
                AgentEvent::ToolCompleted(ToolResult {
                    invocation_id: "c1".to_string(),
                    ok: true,
                    output: serde_json::json!("file content"),
                }),
            ],
            memory: None,
            goal: None,
            usage: None,
        };
        store.save(&snapshot).expect("save");
        let loaded = store.load("events-session").expect("load");
        assert_eq!(loaded.events.len(), 3);
    }

    // ── Regression tests ──────────────────────────────────────────────────────

    #[test]
    fn regression_corrupt_json_on_disk_skipped_by_list() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(tempdir.path().to_path_buf());

        // Write a valid session
        store.save(&make_snapshot("valid", 100)).expect("save");

        // Write a corrupt JSON file
        let corrupt_path = store.root().join("corrupt.json");
        std::fs::write(&corrupt_path, "{invalid json!!!").expect("write corrupt");

        // list() should skip the corrupt file and return only the valid one
        let sessions = store.list();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id.as_str(), "valid");
    }

    #[test]
    fn regression_list_ignores_non_json_files() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(tempdir.path().to_path_buf());

        store.save(&make_snapshot("valid", 100)).expect("save");

        // Write non-json files
        std::fs::write(store.root().join("notes.txt"), "not a session").expect("write");
        std::fs::write(store.root().join("README.md"), "# readme").expect("write");

        let sessions = store.list();
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn regression_load_missing_version_defaults_to_one() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(tempdir.path().to_path_buf());

        // Write a snapshot JSON without the "version" field
        // SessionId serializes as a plain string
        let json = serde_json::json!({
            "id": "no-version",
            "title": null,
            "project": "/tmp/p",
            "created_at": 1,
            "updated_at": 2,
            "events": [],
            "memory": null
        });
        let path = store.root().join("no-version.json");
        std::fs::create_dir_all(store.root()).expect("create sessions dir");
        std::fs::write(&path, serde_json::to_string(&json).unwrap()).expect("write");

        let loaded = store.load("no-version").expect("load");
        assert_eq!(loaded.version, 1); // default_session_version
    }

    #[test]
    fn regression_load_memory_malformed_json_returns_none() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(tempdir.path().to_path_buf());
        let project_dir = PathBuf::from("/tmp/test-project");

        // Write a corrupt memory file
        let hash = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            project_dir.hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        };
        let memory_dir = tempdir.path().join("memory");
        std::fs::create_dir_all(&memory_dir).expect("create");
        std::fs::write(memory_dir.join(format!("{hash}.json")), "not json!").expect("write");

        let loaded = store.load_memory(&project_dir);
        assert!(loaded.is_none(), "malformed memory should return None");
    }

    #[test]
    fn regression_session_title_only_tool_events_returns_none() {
        let events = vec![
            AgentEvent::ToolRequested(ToolInvocation {
                id: "c1".to_string(),
                tool_name: "read_file".to_string(),
                input: serde_json::json!({}),
            }),
            AgentEvent::ToolCompleted(ToolResult {
                invocation_id: "c1".to_string(),
                ok: true,
                output: serde_json::json!("content"),
            }),
        ];
        let title = session_title_from_events(&events);
        assert!(title.is_none(), "no user/model text should return None");
    }

    #[test]
    fn regression_project_hash_is_stable() {
        let path = PathBuf::from("/tmp/some/project/dir");
        let hash1 = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            path.hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        };
        let hash2 = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            path.hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        };
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn regression_create_id_format() {
        let id = SessionStore::create_id();
        assert!(
            id.as_str().starts_with("session-"),
            "session id must start with 'session-'"
        );
    }
}
