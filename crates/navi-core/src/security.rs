use crate::config::SecurityConfig;
use crate::event::AgentEvent;
use crate::patch::PatchProposal;
use crate::tool::{ToolDefinition, ToolInvocation, ToolKind};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

/// Validates tool invocations against security constraints: path restrictions,
/// blocked commands, `.git` protection, and NAVI private storage.
#[derive(Debug, Clone)]
pub struct SecurityPolicy {
    project_root: PathBuf,
    data_dir: PathBuf,
    config: SecurityConfig,
}

/// The outcome of a security validation check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityDecision {
    /// The invocation is allowed without user confirmation.
    Allow,
    /// The invocation requires explicit user approval due to the identified risk.
    NeedsApproval(SecurityRisk),
    /// The invocation is denied with an explanation.
    Deny(String),
}

/// The kind of risk identified by a security check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityRisk {
    /// A write operation that modifies the filesystem.
    Write,
    /// A shell command execution.
    Command,
    /// Loading an external native plugin.
    ExternalPlugin,
}

impl SecurityPolicy {
    /// Creates a new policy from the project root, data directory, and security config.
    pub fn new(project_root: PathBuf, data_dir: PathBuf, config: SecurityConfig) -> Result<Self> {
        Ok(Self {
            project_root: normalize_existing_or_parent(&project_root)
                .with_context(|| format!("failed to resolve {}", project_root.display()))?,
            data_dir: normalize_existing_or_parent(&data_dir)
                .with_context(|| format!("failed to resolve {}", data_dir.display()))?,
            config,
        })
    }

    /// Validates a file path, checking project restrictions, `.git` protection,
    /// and NAVI private storage.
    pub fn validate_path(&self, path: &Path, write: bool) -> SecurityDecision {
        let Ok(path) = normalize_existing_or_parent(path) else {
            return SecurityDecision::Deny(format!("failed to resolve {}", path.display()));
        };

        if self.config.restrict_paths_to_project && !path.starts_with(&self.project_root) {
            return SecurityDecision::Deny(format!(
                "path {} is outside project {}",
                path.display(),
                self.project_root.display()
            ));
        }

        if path.starts_with(&self.data_dir) {
            return SecurityDecision::Deny(format!(
                "path {} is inside NAVI private storage",
                path.display()
            ));
        }

        if write && self.config.protect_git_metadata && contains_component(&path, ".git") {
            return SecurityDecision::Deny(format!(
                "writes to git metadata are blocked: {}",
                path.display()
            ));
        }

        if write {
            SecurityDecision::NeedsApproval(SecurityRisk::Write)
        } else {
            SecurityDecision::Allow
        }
    }

    /// Validates all paths in a patch proposal.
    pub fn validate_patch(&self, patch: &PatchProposal) -> SecurityDecision {
        for file in &patch.files {
            match self.validate_path(file, true) {
                SecurityDecision::Allow | SecurityDecision::NeedsApproval(SecurityRisk::Write) => {}
                decision => return decision,
            }
        }
        SecurityDecision::NeedsApproval(SecurityRisk::Write)
    }

    /// Validates a command against the blocked-commands list and approval config.
    pub fn validate_command(&self, program: &str) -> SecurityDecision {
        let command = command_name(program);
        if self
            .config
            .blocked_commands
            .iter()
            .any(|blocked| blocked == command)
        {
            return SecurityDecision::Deny(format!("command `{command}` is blocked"));
        }

        SecurityDecision::NeedsApproval(SecurityRisk::Command)
    }

    /// Validates a plugin library path, requiring approval unless external plugins
    /// are explicitly allowed.
    pub fn validate_plugin_path(&self, path: &Path) -> SecurityDecision {
        let Ok(path) = normalize_existing_or_parent(path) else {
            return SecurityDecision::Deny(format!("failed to resolve {}", path.display()));
        };

        if self.config.allow_external_plugins {
            return SecurityDecision::NeedsApproval(SecurityRisk::ExternalPlugin);
        }

        let project_plugin_dir = self.project_root.join(".navi").join("plugins");
        let data_plugin_dir = self.data_dir.join("plugins");
        if path.starts_with(project_plugin_dir) || path.starts_with(data_plugin_dir) {
            SecurityDecision::NeedsApproval(SecurityRisk::ExternalPlugin)
        } else {
            SecurityDecision::Deny(format!(
                "plugin {} is outside trusted plugin directories",
                path.display()
            ))
        }
    }

    /// Validates a tool invocation by dispatching to the appropriate validator
    /// based on tool kind.
    pub fn validate_tool_invocation(
        &self,
        definition: &ToolDefinition,
        invocation: &ToolInvocation,
    ) -> SecurityDecision {
        match definition.kind {
            ToolKind::Read => self
                .path_from_invocation(invocation)
                .map(|path| self.validate_path(&path, false))
                .unwrap_or(SecurityDecision::Allow),
            ToolKind::Write => self
                .path_from_invocation(invocation)
                .map(|path| self.validate_path(&path, true))
                .unwrap_or(SecurityDecision::NeedsApproval(SecurityRisk::Write)),
            ToolKind::Command => invocation
                .input
                .get("program")
                .or_else(|| invocation.input.get("command"))
                .and_then(Value::as_str)
                .map(|program| self.validate_command(program))
                .unwrap_or_else(|| {
                    if definition.name == "bash"
                        && (invocation.input.get("task_id").is_some()
                            || invocation.input.get("action").and_then(Value::as_str)
                                == Some("list"))
                    {
                        SecurityDecision::Allow
                    } else {
                        SecurityDecision::NeedsApproval(SecurityRisk::Command)
                    }
                }),
            ToolKind::Custom => SecurityDecision::NeedsApproval(SecurityRisk::ExternalPlugin),
        }
    }

    fn path_from_invocation(&self, invocation: &ToolInvocation) -> Option<PathBuf> {
        invocation
            .input
            .get("path")
            .or_else(|| invocation.input.get("file"))
            .and_then(Value::as_str)
            .map(PathBuf::from)
    }
}

/// Redacts secrets from all events in a session snapshot.
pub fn redact_snapshot_events(events: &[AgentEvent]) -> Vec<AgentEvent> {
    events.iter().map(redact_agent_event).collect()
}

/// Redacts secrets from a single agent event's text fields.
pub fn redact_agent_event(event: &AgentEvent) -> AgentEvent {
    match event {
        AgentEvent::UserTaskSubmitted { text } => AgentEvent::UserTaskSubmitted {
            text: redact_secrets(text),
        },
        AgentEvent::ModelOutput { text, thinking } => AgentEvent::ModelOutput {
            text: redact_secrets(text),
            thinking: thinking.as_ref().map(|t| redact_secrets(t)),
        },
        AgentEvent::ModelDelta { text } => AgentEvent::ModelDelta {
            text: redact_secrets(text),
        },
        AgentEvent::ModelThinkingDelta { text } => AgentEvent::ModelThinkingDelta {
            text: redact_secrets(text),
        },
        AgentEvent::Error { message } => AgentEvent::Error {
            message: redact_secrets(message),
        },
        other => other.clone(),
    }
}

/// Replaces API keys, bearer tokens, and other secret patterns in text with
/// `[REDACTED]`.
pub fn redact_secrets(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut token = String::new();

    for ch in text.chars() {
        if ch.is_whitespace() {
            push_redacted_token(&mut output, &token);
            token.clear();
            output.push(ch);
        } else {
            token.push(ch);
        }
    }
    push_redacted_token(&mut output, &token);

    output
}

fn push_redacted_token(output: &mut String, token: &str) {
    if token.is_empty() {
        return;
    }

    if let Some((prefix, _secret)) = token.split_once('=') {
        if is_secret_assignment_name(prefix) {
            output.push_str(prefix);
            output.push_str("=<redacted>");
            return;
        }
    }

    let trimmed = token.trim_matches(|ch: char| {
        matches!(
            ch,
            '"' | '\'' | '`' | ',' | ';' | ':' | ')' | '(' | '[' | ']' | '{' | '}'
        )
    });

    if looks_like_secret_token(trimmed) {
        output.push_str(&token.replace(trimmed, "<redacted>"));
    } else {
        output.push_str(token);
    }
}

fn looks_like_secret_token(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    let known_prefix = [
        "sk-",
        "sk_",
        "sk-proj-",
        "xai-",
        "anthropic_",
        "ghp_",
        "github_pat_",
        "glpat-",
        "hf_",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix));

    known_prefix
        && token
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .count()
            >= 16
}

fn is_secret_assignment_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.contains("API_KEY")
        || upper.contains("ACCESS_TOKEN")
        || upper.contains("AUTH_TOKEN")
        || upper.contains("SECRET")
        || upper == "TOKEN"
}

fn command_name(program: &str) -> &str {
    let program = program.split_whitespace().next().unwrap_or(program);
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
}

fn contains_component(path: &Path, needle: &str) -> bool {
    path.components().any(|component| match component {
        Component::Normal(value) => value == needle,
        _ => false,
    })
}

fn normalize_existing_or_parent(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return path.canonicalize().map_err(Into::into);
    }

    let mut missing = Vec::new();
    let mut current = path;
    while !current.exists() {
        let component = current.file_name().with_context(|| {
            format!(
                "path {} does not exist and has no existing parent",
                path.display()
            )
        })?;
        missing.push(component.to_os_string());
        current = current.parent().with_context(|| {
            format!(
                "path {} does not exist and has no existing parent",
                path.display()
            )
        })?;
    }

    let mut normalized = current.canonicalize()?;
    for component in missing.iter().rev() {
        normalized.push(component);
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SecurityConfig;
    use crate::patch::PatchProposal;

    fn policy(project_root: PathBuf, data_dir: PathBuf) -> SecurityPolicy {
        SecurityPolicy::new(project_root, data_dir, SecurityConfig::default()).expect("policy")
    }

    #[test]
    fn denies_paths_outside_project() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        let decision = policy.validate_path(tempdir.path().join("outside.txt").as_path(), false);

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    #[test]
    fn write_inside_project_needs_approval() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project.clone(), data);

        let decision = policy.validate_path(project.join("src/lib.rs").as_path(), true);

        assert_eq!(
            decision,
            SecurityDecision::NeedsApproval(SecurityRisk::Write)
        );
    }

    #[test]
    fn denies_writes_to_git_metadata() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(project.join(".git")).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project.clone(), data);

        let decision = policy.validate_path(project.join(".git/config").as_path(), true);

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    #[test]
    fn validates_patch_files() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project.clone(), data);

        let patch = PatchProposal {
            id: "p1".to_string(),
            summary: "edit".to_string(),
            files: vec![project.join("Cargo.toml")],
            unified_diff: String::new(),
        };

        assert_eq!(
            policy.validate_patch(&patch),
            SecurityDecision::NeedsApproval(SecurityRisk::Write)
        );
    }

    #[test]
    fn denies_blocked_commands() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        let decision = policy.validate_command("/bin/rm");

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    #[test]
    fn redacts_secret_like_tokens_and_assignments() {
        let text =
            "OPENAI_API_KEY=sk-proj-1234567890abcdef and bearer sk-1234567890abcdef are present";

        assert_eq!(
            redact_secrets(text),
            "OPENAI_API_KEY=<redacted> and bearer <redacted> are present"
        );
    }

    #[test]
    fn redacts_model_output_thinking_field() {
        let event = AgentEvent::ModelOutput {
            text: "output".to_string(),
            thinking: Some("using OPENAI_API_KEY=sk-proj-1234567890abcdef".to_string()),
        };
        let redacted = redact_agent_event(&event);
        match redacted {
            AgentEvent::ModelOutput { thinking, .. } => {
                let thinking = thinking.unwrap();
                assert!(thinking.contains("OPENAI_API_KEY=<redacted>"));
                assert!(!thinking.contains("sk-proj-1234567890abcdef"));
            }
            _ => panic!("expected ModelOutput"),
        }
    }

    #[test]
    fn redacts_error_event_message() {
        let event = AgentEvent::Error {
            message: "failed with token sk-proj-1234567890abcdef".to_string(),
        };
        let redacted = redact_agent_event(&event);
        match redacted {
            AgentEvent::Error { message } => {
                assert!(message.contains("<redacted>"));
                assert!(!message.contains("sk-proj-1234567890abcdef"));
            }
            _ => panic!("expected Error"),
        }
    }

    #[test]
    fn redacts_model_delta_text() {
        let event = AgentEvent::ModelDelta {
            text: "key is OPENAI_API_KEY=sk-proj-1234567890abcdef".to_string(),
        };
        let redacted = redact_agent_event(&event);
        match redacted {
            AgentEvent::ModelDelta { text } => {
                assert!(text.contains("OPENAI_API_KEY=<redacted>"));
                assert!(!text.contains("sk-proj-1234567890abcdef"));
            }
            _ => panic!("expected ModelDelta"),
        }
    }

    #[test]
    fn redacts_model_thinking_delta_text() {
        let event = AgentEvent::ModelThinkingDelta {
            text: "secret: anthropic_1234567890abcdef".to_string(),
        };
        let redacted = redact_agent_event(&event);
        match redacted {
            AgentEvent::ModelThinkingDelta { text } => {
                assert!(text.contains("<redacted>"));
                assert!(!text.contains("anthropic_1234567890abcdef"));
            }
            _ => panic!("expected ModelThinkingDelta"),
        }
    }

    #[test]
    fn does_not_redact_tool_events() {
        let event = AgentEvent::ToolRequested(crate::tool::ToolInvocation {
            id: "c1".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({"path": "OPENAI_API_KEY=secret123"}),
        });
        let redacted = redact_agent_event(&event);
        match redacted {
            AgentEvent::ToolRequested(invocation) => {
                assert!(invocation.input.to_string().contains("secret123"));
            }
            _ => panic!("expected ToolRequested"),
        }
    }

    #[test]
    fn redacts_snapshot_events_in_bulk() {
        let events = vec![
            AgentEvent::UserTaskSubmitted {
                text: "OPENAI_API_KEY=sk-proj-1234567890abcdef".to_string(),
            },
            AgentEvent::ModelOutput {
                text: "ok".to_string(),
                thinking: Some("bearer ghp_1234567890abcdef1234".to_string()),
            },
            AgentEvent::Error {
                message: "error with token github_pat_1234567890abcdef12".to_string(),
            },
        ];
        let redacted = redact_snapshot_events(&events);
        let json = serde_json::to_string(&redacted).unwrap();
        assert!(!json.contains("sk-proj-1234567890abcdef"));
        assert!(!json.contains("ghp_1234567890abcdef1234"));
        assert!(!json.contains("github_pat_1234567890abcdef12"));
    }
}
