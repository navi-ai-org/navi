use crate::config::SecurityConfig;
use crate::event::AgentEvent;
use crate::patch::PatchProposal;
use crate::tool::{ToolDefinition, ToolInvocation, ToolKind};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SecurityPolicy {
    project_root: PathBuf,
    data_dir: PathBuf,
    config: SecurityConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityDecision {
    Allow,
    NeedsApproval(SecurityRisk),
    Deny(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityRisk {
    Write,
    Command,
    ExternalPlugin,
}

impl SecurityPolicy {
    pub fn new(project_root: PathBuf, data_dir: PathBuf, config: SecurityConfig) -> Result<Self> {
        Ok(Self {
            project_root: normalize_existing_or_parent(&project_root)
                .with_context(|| format!("failed to resolve {}", project_root.display()))?,
            data_dir: normalize_existing_or_parent(&data_dir)
                .with_context(|| format!("failed to resolve {}", data_dir.display()))?,
            config,
        })
    }

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

    pub fn validate_patch(&self, patch: &PatchProposal) -> SecurityDecision {
        for file in &patch.files {
            match self.validate_path(file, true) {
                SecurityDecision::Allow | SecurityDecision::NeedsApproval(SecurityRisk::Write) => {}
                decision => return decision,
            }
        }
        SecurityDecision::NeedsApproval(SecurityRisk::Write)
    }

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

pub fn redact_snapshot_events(events: &[AgentEvent]) -> Vec<AgentEvent> {
    events.iter().map(redact_agent_event).collect()
}

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
}
