use crate::config::{PermissionMode, SecurityConfig};
use crate::effect::{BlastRadius, EffectAnalyzer, PostDecision};
use crate::event::{AgentEvent, ApprovalRequest, SubagentTranscriptItem};
use crate::patch::PatchProposal;
use crate::session::ProjectMemory;
use crate::tool::{ToolDefinition, ToolInvocation, ToolKind, ToolResult};
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
    /// Any tool execution in restricted mode.
    Tool,
    /// A write operation that modifies the filesystem.
    Write,
    /// A shell command execution.
    Command,
    /// A guarded command that requires explicit approval outside YOLO mode
    /// (e.g. destructive `git` operations such as `git push` / `git rebase`).
    GuardedCommand,
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
        let path = self.resolve_project_path(path);
        let Ok(path) = normalize_existing_or_parent(&path) else {
            return SecurityDecision::Deny(format!("failed to resolve {}", path.display()));
        };

        if self.config.restrict_paths_to_project && !path.starts_with(&self.project_root) {
            return SecurityDecision::Deny(format!(
                "path {} is outside project {}",
                path.display(),
                self.project_root.display()
            ));
        }

        if contains_component(&path, ".agent-memory") {
            return SecurityDecision::Deny(format!(
                "project-local .agent-memory is not supported; NAVI memory lives under {}",
                self.data_dir.display()
            ));
        }

        if self.is_data_dir_private_path(&path) {
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

        // Deny list: block reads of wasteful/sensitive paths.
        if !write && self.is_path_denied(&path) {
            return SecurityDecision::Deny(format!(
                "path {} is on the deny list (wasteful or sensitive)",
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

    /// Validates a command against the blocked-commands list, guarded-commands
    /// list, and approval config.
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

        if self.is_guarded_command(program, command) {
            return SecurityDecision::NeedsApproval(SecurityRisk::GuardedCommand);
        }

        for target in extract_shell_path_mentions(program) {
            if is_dynamic_shell_target(&target) {
                continue;
            }
            let path = self.resolve_project_path(Path::new(&target));
            let Ok(path) = normalize_existing_or_parent(&path) else {
                continue;
            };
            if self.is_data_dir_private_path(&path) {
                return SecurityDecision::Deny(format!(
                    "command references NAVI private storage: {}",
                    path.display()
                ));
            }
        }

        for target in extract_shell_write_targets(program) {
            if is_dynamic_shell_target(&target) {
                return SecurityDecision::Deny(format!(
                    "command writes to an unresolved shell-expanded path: {target}"
                ));
            }
            if let SecurityDecision::Deny(reason) = self.validate_path(Path::new(&target), true) {
                return SecurityDecision::Deny(format!(
                    "command writes to a denied path via shell redirection: {reason}"
                ));
            }
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

    /// Validates an MCP server id against the configured allowlist.
    ///
    /// When `allowlist` is non-empty, only server ids present in the list
    /// are allowed. An empty allowlist permits all MCP servers.
    pub fn validate_mcp_server(&self, server_id: &str) -> SecurityDecision {
        if self.config.is_mcp_server_allowed(server_id) {
            SecurityDecision::Allow
        } else {
            SecurityDecision::Deny(format!("MCP server `{server_id}` is not in the allowlist"))
        }
    }

    /// Validates a tool invocation by dispatching to the appropriate validator
    /// based on tool kind.
    pub fn validate_tool_invocation(
        &self,
        definition: &ToolDefinition,
        invocation: &ToolInvocation,
    ) -> SecurityDecision {
        if let Some(decision) = self.tool_rule_decision(definition, invocation) {
            return decision;
        }

        let base_decision = match definition.kind {
            ToolKind::Read => self
                .path_from_invocation(invocation)
                .map(|path| self.validate_path(&path, false))
                .unwrap_or(SecurityDecision::Allow),
            ToolKind::Write => {
                if definition.name == "apply_patch" || definition.name == "write" {
                    self.validate_apply_patch_invocation(invocation)
                } else {
                    self.path_from_invocation(invocation)
                        .map(|path| self.validate_path(&path, true))
                        .unwrap_or(SecurityDecision::NeedsApproval(SecurityRisk::Write))
                }
            }
            ToolKind::Command => {
                if let Some(cwd) = invocation.input.get("cwd").and_then(Value::as_str)
                    && let SecurityDecision::Deny(reason) =
                        self.validate_path(Path::new(cwd), false)
                {
                    SecurityDecision::Deny(format!("command cwd is denied: {reason}"))
                } else if definition.name == "bash"
                    && (invocation.input.get("task_id").is_some()
                        || invocation.input.get("action").and_then(Value::as_str) == Some("list"))
                {
                    SecurityDecision::Allow
                } else if definition.name == "browser" {
                    // status/doctor are local probes; navigation needs approval by default.
                    match invocation.input.get("action").and_then(Value::as_str) {
                        Some("status" | "doctor") => SecurityDecision::Allow,
                        _ => SecurityDecision::NeedsApproval(SecurityRisk::Command),
                    }
                } else if definition.name == "mark_feature_done" {
                    self.validate_verification_steps(invocation)
                } else {
                    invocation
                        .input
                        .get("program")
                        .or_else(|| invocation.input.get("command"))
                        .and_then(Value::as_str)
                        .map(|program| self.validate_command(program))
                        .unwrap_or(SecurityDecision::NeedsApproval(SecurityRisk::Command))
                }
            }
            ToolKind::Custom => SecurityDecision::NeedsApproval(SecurityRisk::ExternalPlugin),
        };

        self.apply_permission_mode(definition, base_decision)
    }

    fn tool_rule_decision(
        &self,
        definition: &ToolDefinition,
        invocation: &ToolInvocation,
    ) -> Option<SecurityDecision> {
        let name = invocation.tool_name.as_str();
        if matches_tool_rule(name, &self.config.deny_tools, &self.config.deny_tool_regex) {
            return Some(SecurityDecision::Deny(format!(
                "tool `{}` is denied by security.tool policy",
                name
            )));
        }
        if let Some(err) = first_invalid_regex(&self.config.deny_tool_regex)
            .or_else(|| first_invalid_regex(&self.config.allow_tool_regex))
            .or_else(|| first_invalid_regex(&self.config.ask_tool_regex))
        {
            return Some(err);
        }

        if matches_tool_rule(
            name,
            &self.config.allow_tools,
            &self.config.allow_tool_regex,
        ) {
            return Some(self.safety_only_decision(definition, invocation, true));
        }

        if matches_tool_rule(name, &self.config.ask_tools, &self.config.ask_tool_regex) {
            return Some(
                match self.safety_only_decision(definition, invocation, false) {
                    SecurityDecision::Deny(reason) => SecurityDecision::Deny(reason),
                    SecurityDecision::Allow | SecurityDecision::NeedsApproval(_) => {
                        SecurityDecision::NeedsApproval(risk_for_tool_kind(definition.kind))
                    }
                },
            );
        }

        None
    }

    fn safety_only_decision(
        &self,
        definition: &ToolDefinition,
        invocation: &ToolInvocation,
        allow_after_safety: bool,
    ) -> SecurityDecision {
        let decision = match definition.kind {
            ToolKind::Read => self
                .path_from_invocation(invocation)
                .map(|path| self.validate_path(&path, false))
                .unwrap_or(SecurityDecision::Allow),
            ToolKind::Write => {
                if definition.name == "apply_patch" || definition.name == "write" {
                    self.validate_apply_patch_invocation(invocation)
                } else {
                    self.path_from_invocation(invocation)
                        .map(|path| self.validate_path(&path, true))
                        .unwrap_or(SecurityDecision::NeedsApproval(SecurityRisk::Write))
                }
            }
            ToolKind::Command => {
                if let Some(cwd) = invocation.input.get("cwd").and_then(Value::as_str)
                    && let SecurityDecision::Deny(reason) =
                        self.validate_path(Path::new(cwd), false)
                {
                    return SecurityDecision::Deny(format!("command cwd is denied: {reason}"));
                }
                if definition.name == "bash"
                    && (invocation.input.get("task_id").is_some()
                        || invocation.input.get("action").and_then(Value::as_str) == Some("list"))
                {
                    SecurityDecision::Allow
                } else if definition.name == "browser" {
                    match invocation.input.get("action").and_then(Value::as_str) {
                        Some("status" | "doctor") => SecurityDecision::Allow,
                        _ => SecurityDecision::NeedsApproval(SecurityRisk::Command),
                    }
                } else if definition.name == "mark_feature_done" {
                    self.validate_verification_steps(invocation)
                } else {
                    invocation
                        .input
                        .get("program")
                        .or_else(|| invocation.input.get("command"))
                        .and_then(Value::as_str)
                        .map(|program| self.validate_command(program))
                        .unwrap_or(SecurityDecision::NeedsApproval(SecurityRisk::Command))
                }
            }
            ToolKind::Custom => SecurityDecision::NeedsApproval(SecurityRisk::ExternalPlugin),
        };

        match decision {
            SecurityDecision::Deny(reason) => SecurityDecision::Deny(reason),
            SecurityDecision::Allow | SecurityDecision::NeedsApproval(_) if allow_after_safety => {
                SecurityDecision::Allow
            }
            other => other,
        }
    }

    fn apply_permission_mode(
        &self,
        definition: &ToolDefinition,
        decision: SecurityDecision,
    ) -> SecurityDecision {
        match decision {
            SecurityDecision::Deny(reason) => SecurityDecision::Deny(reason),
            SecurityDecision::NeedsApproval(SecurityRisk::GuardedCommand) => {
                match self.config.permission_mode {
                    PermissionMode::Yolo => SecurityDecision::Allow,
                    _ => SecurityDecision::NeedsApproval(SecurityRisk::GuardedCommand),
                }
            }
            SecurityDecision::Allow | SecurityDecision::NeedsApproval(_) => {
                match self.config.permission_mode {
                    PermissionMode::Restricted => {
                        SecurityDecision::NeedsApproval(risk_for_tool_kind(definition.kind))
                    }
                    PermissionMode::AcceptEdits => match definition.kind {
                        ToolKind::Read | ToolKind::Write => SecurityDecision::Allow,
                        ToolKind::Command => SecurityDecision::NeedsApproval(SecurityRisk::Command),
                        ToolKind::Custom => {
                            SecurityDecision::NeedsApproval(SecurityRisk::ExternalPlugin)
                        }
                    },
                    PermissionMode::Auto => SecurityDecision::Allow,
                    PermissionMode::Yolo => SecurityDecision::Allow,
                }
            }
        }
    }

    fn path_from_invocation(&self, invocation: &ToolInvocation) -> Option<PathBuf> {
        invocation
            .input
            .get("path")
            .or_else(|| invocation.input.get("file"))
            .and_then(Value::as_str)
            .map(|path| self.resolve_project_path(Path::new(path)))
    }

    fn validate_apply_patch_invocation(&self, invocation: &ToolInvocation) -> SecurityDecision {
        if invocation
            .input
            .get("path")
            .and_then(Value::as_str)
            .is_some()
        {
            return self
                .path_from_invocation(invocation)
                .map(|path| self.validate_path(&path, true))
                .unwrap_or(SecurityDecision::NeedsApproval(SecurityRisk::Write));
        }

        let mut patches = Vec::new();
        if let Some(patch) = invocation.input.get("patch").and_then(Value::as_str) {
            patches.push(patch);
        }
        if let Some(values) = invocation.input.get("patches").and_then(Value::as_array) {
            patches.extend(values.iter().filter_map(Value::as_str));
        }
        if patches.is_empty() {
            return SecurityDecision::NeedsApproval(SecurityRisk::Write);
        }
        let paths = patches
            .iter()
            .flat_map(|patch| extract_apply_patch_paths(patch))
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return SecurityDecision::NeedsApproval(SecurityRisk::Write);
        }
        for path in paths {
            match self.validate_path(Path::new(&path), true) {
                SecurityDecision::Allow | SecurityDecision::NeedsApproval(SecurityRisk::Write) => {}
                decision => return decision,
            }
        }
        SecurityDecision::NeedsApproval(SecurityRisk::Write)
    }

    fn validate_verification_steps(&self, invocation: &ToolInvocation) -> SecurityDecision {
        if let Some(steps) = invocation
            .input
            .get("verification_steps")
            .and_then(Value::as_array)
        {
            for command in steps.iter().filter_map(Value::as_str) {
                if let SecurityDecision::Deny(reason) = self.validate_command(command) {
                    return SecurityDecision::Deny(reason);
                }
            }
        }
        SecurityDecision::NeedsApproval(SecurityRisk::Command)
    }

    /// Performs a post-execution effect check on the paths touched by a tool.
    ///
    /// Analyses created, modified, and deleted paths through the
    /// [`EffectAnalyzer`] and produces a [`PostDecision`] that the harness
    /// can act on (allow, ask, deny, or roll back).
    ///
    /// `tool_name` is used for contextual messaging. `paths` are the filesystem
    /// paths the tool reported touching. `command` is the shell command string,
    /// if any (used for context, not analysed here).
    pub fn post_execution_effect_check(
        &self,
        tool_name: &str,
        paths: &[PathBuf],
        _command: Option<&str>,
    ) -> PostDecision {
        // Classify paths into created / modified / deleted.
        // We don't have reliable create-vs-modify-vs-delete metadata from the
        // generic path list, so we conservatively treat all as modified.
        let report = EffectAnalyzer::analyze(&[], paths, &[]);

        if report.key_files_affected.is_empty() {
            return PostDecision::Allow;
        }

        match report.blast_radius {
            BlastRadius::SecuritySensitive => {
                let details = report.key_files_affected.join(", ");
                PostDecision::Rollback(format!(
                    "{} touched security-sensitive file(s): {details}. \
                     Modification may expose secrets or credentials.",
                    tool_name,
                ))
            }
            BlastRadius::CiConfig => {
                let details = report.key_files_affected.join(", ");
                PostDecision::Ask(format!(
                    "{} modified CI configuration: {details}. \
                     Review before proceeding.",
                    tool_name,
                ))
            }
            BlastRadius::DependencyChange => {
                let details = report.key_files_affected.join(", ");
                PostDecision::Ask(format!(
                    "{} modified dependency/lockfile(s): {details}. \
                     This may affect builds across the team.",
                    tool_name,
                ))
            }
            BlastRadius::MultipleFiles | BlastRadius::SingleFile => PostDecision::Allow,
        }
    }

    /// Returns the normalized project root used as the execution sandbox.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Returns a reference to the security configuration.
    pub fn config(&self) -> &SecurityConfig {
        &self.config
    }

    /// Replaces the security configuration used by subsequent validations.
    pub fn set_config(&mut self, config: SecurityConfig) {
        self.config = config;
    }

    /// Returns the NAVI data directory used for persistent storage (sessions,
    /// memory, plans, credentials, logs).
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    fn is_data_dir_private_path(&self, path: &Path) -> bool {
        path.starts_with(&self.data_dir) && !path.starts_with(self.data_dir.join("plugins"))
    }

    /// Whether `program` should be treated as a guarded command.
    ///
    /// For `git`, only destructive subcommands (push/rm/reset/rebase/...) are
    /// guarded. Common operations like `status` / `add` / `commit` are not.
    fn is_guarded_command(&self, program: &str, command: &str) -> bool {
        if !self
            .config
            .guarded_commands
            .iter()
            .any(|guarded| guarded == command)
        {
            return false;
        }

        if command == "git" {
            return is_destructive_git_command(program);
        }

        true
    }

    /// Resolves relative tool paths against the project root instead of the
    /// process CWD. This keeps SDK/ACP embeddings from accidentally reading or
    /// writing outside the requested project when NAVI is launched elsewhere.
    pub fn resolve_project_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.project_root.join(path)
        }
    }

    /// Returns a copy of the invocation with security-visible path fields made
    /// absolute under the project root.
    pub fn normalize_invocation_paths(&self, invocation: &ToolInvocation) -> ToolInvocation {
        let mut invocation = invocation.clone();
        if let Value::Object(ref mut map) = invocation.input {
            for key in ["path", "file"] {
                if let Some(Value::String(value)) = map.get_mut(key) {
                    let resolved = self.resolve_project_path(Path::new(value));
                    *value = resolved.display().to_string();
                }
            }
        }
        invocation
    }

    /// Check if a path matches any entry in the deny list.
    ///
    /// Supports:
    /// - Directory name prefixes: `"node_modules"` matches `node_modules/foo/bar.js`
    /// - Glob patterns: `"*.log"` matches `debug.log`
    /// - Exact path suffixes: `"package-lock.json"` matches `foo/package-lock.json`
    pub fn is_path_denied(&self, path: &Path) -> bool {
        if self.config.deny_paths.is_empty() {
            return false;
        }
        let path_str = path.to_string_lossy();
        let path_lower = path_str.to_lowercase();

        for pattern in &self.config.deny_paths {
            let pattern_lower = pattern.to_lowercase();

            // Glob pattern: starts with * (e.g. "*.log")
            if let Some(suffix) = pattern_lower.strip_prefix('*') {
                if path_lower.ends_with(suffix) {
                    return true;
                }
                continue;
            }

            // Directory prefix: check if any component matches or path contains it.
            if path.components().any(|c| {
                if let Component::Normal(name) = c {
                    let name_lower = name.to_string_lossy().to_lowercase();
                    name_lower == pattern_lower
                } else {
                    false
                }
            }) {
                return true;
            }

            // Suffix match: "package-lock.json" matches "foo/package-lock.json"
            if path_lower.ends_with(&pattern_lower) {
                return true;
            }
        }

        false
    }

    /// Filter text output by removing lines that reference denied paths.
    ///
    /// Used by grep and fs_browser to prevent denied path references from
    /// entering the LLM context.
    pub fn filter_denied_lines(&self, text: &str) -> String {
        if self.config.deny_paths.is_empty() {
            return text.to_string();
        }

        let mut output = String::with_capacity(text.len());
        for line in text.lines() {
            if !self.line_references_denied_path(line) {
                output.push_str(line);
                output.push('\n');
            }
        }
        output
    }

    /// Check if a single line references any denied path pattern.
    fn line_references_denied_path(&self, line: &str) -> bool {
        let line_lower = line.to_lowercase();
        for pattern in &self.config.deny_paths {
            let pattern_lower = pattern.to_lowercase();

            if let Some(suffix) = pattern_lower.strip_prefix('*') {
                // For glob patterns, check if the line contains the suffix.
                if line_lower.contains(suffix) {
                    return true;
                }
            } else {
                // For exact patterns, check if the line contains the pattern.
                if line_lower.contains(&pattern_lower) {
                    return true;
                }
            }
        }
        false
    }
}

/// Redacts secrets from all events in a session snapshot.
pub fn redact_snapshot_events(events: &[AgentEvent]) -> Vec<AgentEvent> {
    events.iter().map(redact_agent_event).collect()
}

/// Redacts secrets from a single agent event's text fields.
pub fn redact_agent_event(event: &AgentEvent) -> AgentEvent {
    match event {
        AgentEvent::UserTaskSubmitted {
            text,
            content_parts,
            submitted_at,
        } => AgentEvent::UserTaskSubmitted {
            text: redact_secrets(text),
            content_parts: content_parts.clone(),
            submitted_at: *submitted_at,
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
        AgentEvent::ToolRequested(invocation) => {
            AgentEvent::ToolRequested(redact_tool_invocation(invocation))
        }
        AgentEvent::ToolCompleted(result) => AgentEvent::ToolCompleted(redact_tool_result(result)),
        AgentEvent::SubagentActivity {
            invocation_id,
            message,
        } => AgentEvent::SubagentActivity {
            invocation_id: invocation_id.clone(),
            message: redact_secrets(message),
        },
        AgentEvent::SubagentTranscript {
            invocation_id,
            item,
        } => AgentEvent::SubagentTranscript {
            invocation_id: invocation_id.clone(),
            item: redact_subagent_transcript_item(item),
        },
        AgentEvent::HarnessTrace(value) => AgentEvent::HarnessTrace(redact_json_value(value)),
        AgentEvent::HarnessStopped {
            reason,
            message,
            tool_name,
        } => AgentEvent::HarnessStopped {
            reason: reason.clone(),
            message: redact_secrets(message),
            tool_name: tool_name.clone(),
        },
        AgentEvent::PatchProposed(patch) => AgentEvent::PatchProposed(redact_patch_proposal(patch)),
        AgentEvent::ApprovalRequested(request) => {
            AgentEvent::ApprovalRequested(redact_approval_request(request))
        }
        AgentEvent::CapabilityRecorded(entry) => {
            let mut entry = entry.clone();
            entry.justification = redact_secrets(&entry.justification);
            AgentEvent::CapabilityRecorded(entry)
        }
        other => other.clone(),
    }
}

fn redact_subagent_transcript_item(item: &SubagentTranscriptItem) -> SubagentTranscriptItem {
    SubagentTranscriptItem {
        kind: item.kind,
        title: redact_secrets(&item.title),
        detail: item.detail.as_ref().map(|detail| redact_secrets(detail)),
        ok: item.ok,
    }
}

/// Redacts secrets from a `ProjectMemory`'s entry summaries so that persisted
/// memory snapshots don't leak credentials the model may have echoed back.
pub fn redact_memory(memory: &ProjectMemory) -> ProjectMemory {
    ProjectMemory {
        project_hash: memory.project_hash.clone(),
        entries: memory
            .entries
            .iter()
            .map(|entry| crate::session::MemoryEntry {
                created_at: entry.created_at,
                summary: redact_secrets(&entry.summary),
                session_id: entry.session_id.clone(),
            })
            .collect(),
    }
}

fn redact_tool_invocation(invocation: &ToolInvocation) -> ToolInvocation {
    ToolInvocation {
        id: invocation.id.clone(),
        tool_name: invocation.tool_name.clone(),
        input: redact_json_value(&invocation.input),
    }
}

fn redact_tool_result(result: &ToolResult) -> ToolResult {
    ToolResult {
        invocation_id: result.invocation_id.clone(),
        ok: result.ok,
        output: redact_json_value(&result.output),
    }
}

fn redact_patch_proposal(patch: &PatchProposal) -> PatchProposal {
    PatchProposal {
        id: patch.id.clone(),
        summary: redact_secrets(&patch.summary),
        files: patch.files.clone(),
        unified_diff: redact_secrets(&patch.unified_diff),
    }
}

fn redact_approval_request(request: &ApprovalRequest) -> ApprovalRequest {
    ApprovalRequest {
        id: request.id.clone(),
        summary: redact_secrets(&request.summary),
        risk: request.risk.clone(),
    }
}

fn redact_json_value(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(redact_secrets(text)),
        Value::Array(values) => Value::Array(values.iter().map(redact_json_value).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), redact_json_value(value)))
                .collect(),
        ),
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

    if let Some((prefix, _secret)) = token.split_once('=')
        && is_secret_assignment_name(prefix)
    {
        output.push_str(prefix);
        output.push_str("=<redacted>");
        return;
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

/// Returns true when the command line is a destructive `git` operation that
/// should require explicit approval outside YOLO mode.
fn is_destructive_git_command(program: &str) -> bool {
    let Some(subcommand) = git_primary_subcommand(program) else {
        // Unknown / bare `git` — treat as guarded to be safe.
        return true;
    };

    match subcommand.as_str() {
        // Network / history rewrites / force-delete style operations.
        "push" | "rm" | "reset" | "clean" | "rebase" | "filter-branch" | "filter-repo"
        | "update-ref" | "replace" | "gc" | "prune" | "notes" | "am" => true,
        // Subcommands that are only destructive with certain arguments.
        "branch" => git_has_delete_flag(program),
        "tag" => git_has_delete_flag(program),
        "stash" => git_subcommand_is(program, &["drop", "clear"]),
        "worktree" => git_subcommand_is(program, &["remove", "prune"]),
        "remote" => git_subcommand_is(program, &["remove", "rm", "prune"]),
        "reflog" => git_subcommand_is(program, &["delete", "expire"]),
        // Common non-destructive operations (status/add/commit/log/diff/...).
        _ => false,
    }
}

/// Extracts the primary git subcommand, skipping global options such as
/// `-C <path>`, `--git-dir=<path>`, and `-c name=value`.
fn git_primary_subcommand(program: &str) -> Option<String> {
    let tokens = shell_tokens(program);
    let mut index = 0;

    // First token should be git (possibly a path to git).
    if tokens
        .first()
        .map(|token| command_token_name(token) != "git")
        .unwrap_or(true)
    {
        return None;
    }
    index += 1;

    while index < tokens.len() {
        let token = tokens[index].as_str();
        if token == "--" {
            index += 1;
            break;
        }
        if !token.starts_with('-') {
            return Some(token.to_string());
        }

        // Options that take a following argument.
        match token {
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env" => {
                index += 2;
            }
            _ if token.starts_with("--git-dir=")
                || token.starts_with("--work-tree=")
                || token.starts_with("--namespace=")
                || token.starts_with("--config-env=")
                || token.starts_with("-c") =>
            {
                index += 1;
            }
            _ => index += 1,
        }
    }

    tokens.get(index).cloned()
}

fn git_has_delete_flag(program: &str) -> bool {
    shell_tokens(program).iter().any(|token| {
        matches!(token.as_str(), "-d" | "-D" | "--delete" | "--delete-tag")
            || token.starts_with("--delete=")
    })
}

fn git_subcommand_is(program: &str, candidates: &[&str]) -> bool {
    let tokens = shell_tokens(program);
    // Find primary subcommand, then look at the next non-option token.
    let Some(primary) = git_primary_subcommand(program) else {
        return false;
    };
    let Some(primary_idx) = tokens.iter().position(|token| token == &primary) else {
        return false;
    };
    tokens
        .iter()
        .skip(primary_idx + 1)
        .find(|token| !token.starts_with('-') && *token != "--")
        .is_some_and(|token| candidates.iter().any(|candidate| candidate == token))
}

fn contains_component(path: &Path, needle: &str) -> bool {
    path.components().any(|component| match component {
        Component::Normal(value) => value == needle,
        _ => false,
    })
}

pub(crate) fn extract_shell_write_targets(command: &str) -> Vec<String> {
    let tokens = shell_tokens(command);
    let mut targets = Vec::new();
    let mut index = 0;

    while index < tokens.len() {
        let token = &tokens[index];
        let command_name = command_token_name(token);

        if command_name == "sed" {
            index = collect_sed_in_place_targets(&tokens, index + 1, &mut targets);
            continue;
        }

        if command_name == "perl" {
            index = collect_perl_in_place_targets(&tokens, index + 1, &mut targets);
            continue;
        }

        if is_output_redirection_operator(token) {
            if let Some(target) = tokens
                .get(index + 1)
                .and_then(|value| clean_shell_target(value))
            {
                push_unique_string(&mut targets, &target);
            }
            index += 2;
            continue;
        }

        if let Some(target) = attached_output_redirection_target(token) {
            push_unique_string(&mut targets, &target);
            index += 1;
            continue;
        }

        if command_token_name(token) == "tee" {
            index += 1;
            while index < tokens.len() && !is_shell_command_separator(&tokens[index]) {
                let arg = &tokens[index];
                if arg == "--" {
                    index += 1;
                    continue;
                }
                if !arg.starts_with('-')
                    && let Some(target) = clean_shell_target(arg)
                {
                    push_unique_string(&mut targets, &target);
                }
                index += 1;
            }
            continue;
        }

        index += 1;
    }

    targets
}

fn collect_sed_in_place_targets(
    tokens: &[String],
    mut index: usize,
    targets: &mut Vec<String>,
) -> usize {
    let mut in_place = false;
    let mut script_seen = false;

    while index < tokens.len() && !is_shell_command_separator(&tokens[index]) {
        let token = &tokens[index];

        if token == "--" {
            index += 1;
            break;
        }

        if token == "-i" || token.starts_with("-i") {
            in_place = true;
            index += 1;
            continue;
        }

        if token == "-e" || token == "-f" {
            script_seen = true;
            index = index.saturating_add(2);
            continue;
        }

        if token.starts_with("-e") || token.starts_with("-f") {
            script_seen = true;
            index += 1;
            continue;
        }

        if token.starts_with('-') {
            index += 1;
            continue;
        }

        if in_place
            && script_seen
            && let Some(target) = clean_shell_target(token)
        {
            push_unique_string(targets, &target);
        }
        script_seen = true;
        index += 1;
    }

    while in_place && index < tokens.len() && !is_shell_command_separator(&tokens[index]) {
        if let Some(target) = clean_shell_target(&tokens[index]) {
            push_unique_string(targets, &target);
        }
        index += 1;
    }

    index
}

fn collect_perl_in_place_targets(
    tokens: &[String],
    mut index: usize,
    targets: &mut Vec<String>,
) -> usize {
    let mut in_place = false;

    while index < tokens.len() && !is_shell_command_separator(&tokens[index]) {
        let token = &tokens[index];

        if token == "--" {
            index += 1;
            break;
        }

        if token.starts_with('-') {
            if token == "-e" {
                index = index.saturating_add(2);
                continue;
            }
            if token.starts_with("-e") {
                index += 1;
                continue;
            }
            if token == "-i" || token.starts_with("-i") || token.chars().skip(1).any(|ch| ch == 'i')
            {
                in_place = true;
            }
            index += 1;
            continue;
        }

        if in_place && let Some(target) = clean_shell_target(token) {
            push_unique_string(targets, &target);
        }
        index += 1;
    }

    while in_place && index < tokens.len() && !is_shell_command_separator(&tokens[index]) {
        if let Some(target) = clean_shell_target(&tokens[index]) {
            push_unique_string(targets, &target);
        }
        index += 1;
    }

    index
}

fn extract_shell_path_mentions(command: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for token in shell_tokens(command) {
        if is_shell_command_separator(&token) || is_output_redirection_operator(&token) {
            continue;
        }
        if let Some(target) = attached_output_redirection_target(&token) {
            push_unique_string(&mut paths, &target);
            continue;
        }
        if let Some(path) = clean_shell_target(&token)
            && looks_like_path(&path)
        {
            push_unique_string(&mut paths, &path);
        }
    }
    paths
}

fn shell_tokens(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut chars = command.chars().peekable();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            token.push(ch);
            escaped = false;
            continue;
        }

        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }

        if let Some(quote_ch) = quote {
            if ch == quote_ch {
                quote = None;
            } else {
                token.push(ch);
            }
            continue;
        }

        match ch {
            '\'' | '"' => quote = Some(ch),
            ch if ch.is_whitespace() => {
                push_shell_token(&mut tokens, &mut token);
            }
            '>' | '<' => {
                push_shell_token(&mut tokens, &mut token);
                let mut operator = String::from(ch);
                while let Some(next) = chars.peek().copied() {
                    if next == '>' || next == '<' || next == '&' || next == '|' {
                        operator.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(operator);
            }
            '&' | '|' | ';' => {
                push_shell_token(&mut tokens, &mut token);
                let mut operator = String::from(ch);
                if let Some(next) = chars.peek().copied()
                    && next == ch
                {
                    operator.push(next);
                    chars.next();
                }
                tokens.push(operator);
            }
            _ => token.push(ch),
        }
    }

    push_shell_token(&mut tokens, &mut token);
    tokens
}

fn push_shell_token(tokens: &mut Vec<String>, token: &mut String) {
    if !token.is_empty() {
        tokens.push(std::mem::take(token));
    }
}

fn is_output_redirection_operator(token: &str) -> bool {
    matches!(token, ">" | ">>" | ">|" | "&>" | "&>>")
        || token
            .strip_suffix('>')
            .or_else(|| token.strip_suffix(">>"))
            .is_some_and(|prefix| prefix.chars().all(|ch| ch.is_ascii_digit()))
}

fn attached_output_redirection_target(token: &str) -> Option<String> {
    let operators = ["&>>", "&>", ">|", ">>", ">"];
    for operator in operators {
        if let Some(target) = token.strip_prefix(operator) {
            return clean_shell_target(target);
        }
    }

    let digit_count = token.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digit_count == 0 {
        return None;
    }
    let rest = &token[digit_count..];
    for operator in [">>", ">", ">|"] {
        if let Some(target) = rest.strip_prefix(operator) {
            return clean_shell_target(target);
        }
    }
    None
}

fn clean_shell_target(target: &str) -> Option<String> {
    let target = target.trim();
    if target.is_empty()
        || target == "-"
        || target == "/dev/null"
        || target.starts_with('&')
        || target.starts_with('(')
    {
        return None;
    }
    Some(expand_home_shell_target(target))
}

fn expand_home_shell_target(target: &str) -> String {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return target.to_string();
    };
    if let Some(rest) = target.strip_prefix("~/") {
        return home.join(rest).display().to_string();
    }
    if let Some(rest) = target.strip_prefix("$HOME/") {
        return home.join(rest).display().to_string();
    }
    if let Some(rest) = target.strip_prefix("${HOME}/") {
        return home.join(rest).display().to_string();
    }
    target.to_string()
}

fn is_dynamic_shell_target(target: &str) -> bool {
    target.contains('$') || target.contains('`')
}

fn is_shell_command_separator(token: &str) -> bool {
    matches!(token, "|" | "||" | "&&" | ";" | "&")
}

fn looks_like_path(token: &str) -> bool {
    token.starts_with('/')
        || token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with("~/")
        || token.starts_with("$HOME/")
        || token.starts_with("${HOME}/")
        || token.contains('/')
}

fn command_token_name(token: &str) -> &str {
    Path::new(token)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(token)
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

pub(crate) fn extract_apply_patch_paths(patch: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            push_unique_string(&mut paths, path);
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            push_unique_string(&mut paths, path);
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            push_unique_string(&mut paths, path);
        } else if let Some(path) = line.strip_prefix("*** Move to: ") {
            push_unique_string(&mut paths, path);
        } else if let Some(path) = line.strip_prefix("--- a/") {
            push_unique_string(&mut paths, path);
        } else if let Some(path) = line.strip_prefix("+++ b/") {
            push_unique_string(&mut paths, path);
        } else if let Some(path) = line.strip_prefix("--- ")
            && path != "/dev/null"
        {
            push_unique_string(&mut paths, path);
        } else if let Some(path) = line.strip_prefix("+++ ")
            && path != "/dev/null"
        {
            push_unique_string(&mut paths, path);
        }
    }
    paths
}

fn push_unique_string(paths: &mut Vec<String>, path: &str) {
    let path = path.split('\t').next().unwrap_or(path).to_string();
    if path != "/dev/null" && !paths.contains(&path) {
        paths.push(path);
    }
}

fn risk_for_tool_kind(kind: ToolKind) -> SecurityRisk {
    match kind {
        ToolKind::Read => SecurityRisk::Tool,
        ToolKind::Write => SecurityRisk::Write,
        ToolKind::Command => SecurityRisk::Command,
        ToolKind::Custom => SecurityRisk::ExternalPlugin,
    }
}

fn matches_tool_rule(name: &str, names: &[String], patterns: &[String]) -> bool {
    names.iter().any(|candidate| candidate == name)
        || patterns
            .iter()
            .filter_map(|pattern| regex::Regex::new(pattern).ok())
            .any(|regex| regex.is_match(name))
}

fn first_invalid_regex(patterns: &[String]) -> Option<SecurityDecision> {
    patterns
        .iter()
        .find_map(|pattern| match regex::Regex::new(pattern) {
            Ok(_) => None,
            Err(err) => Some(SecurityDecision::Deny(format!(
                "invalid tool permission regex `{pattern}`: {err}"
            ))),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SecurityConfig;
    use crate::patch::PatchProposal;

    fn policy(project_root: PathBuf, data_dir: PathBuf) -> SecurityPolicy {
        SecurityPolicy::new(project_root, data_dir, SecurityConfig::default()).expect("policy")
    }

    fn policy_with_config(
        project_root: PathBuf,
        data_dir: PathBuf,
        config: SecurityConfig,
    ) -> SecurityPolicy {
        SecurityPolicy::new(project_root, data_dir, config).expect("policy")
    }

    fn tool_def(name: &str, kind: ToolKind) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            kind,
            input_schema: serde_json::json!({}),
            ..Default::default()
        }
    }

    fn tool_invocation(name: &str, input: Value) -> ToolInvocation {
        ToolInvocation {
            id: format!("{name}-1"),
            tool_name: name.to_string(),
            input,
        }
    }

    #[test]
    fn allows_paths_outside_project() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        let decision = policy.validate_path(tempdir.path().join("outside.txt").as_path(), false);

        assert_eq!(decision, SecurityDecision::Allow);
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
    fn restricted_mode_requires_approval_for_read_tools() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(project.join("src")).expect("project");
        std::fs::write(project.join("src/lib.rs"), "").expect("file");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        let decision = policy.validate_tool_invocation(
            &tool_def("read_file", ToolKind::Read),
            &tool_invocation("read_file", serde_json::json!({ "path": "src/lib.rs" })),
        );

        assert_eq!(
            decision,
            SecurityDecision::NeedsApproval(SecurityRisk::Tool)
        );
    }

    #[test]
    fn restricted_mode_requires_approval_for_apply_patch() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::write(project.join("README.md"), "Less then ideal\n").expect("file");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        let decision = policy.validate_tool_invocation(
            &tool_def("apply_patch", ToolKind::Write),
            &tool_invocation(
                "apply_patch",
                serde_json::json!({
                    "patch": "*** Begin Patch\n*** Update File: README.md\n@@\n-Less then ideal\n+Less than ideal\n*** End Patch\n"
                }),
            ),
        );

        assert_eq!(
            decision,
            SecurityDecision::NeedsApproval(SecurityRisk::Write)
        );
    }

    #[test]
    fn restricted_mode_requires_approval_for_process_actions() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);
        let def = tool_def("process", ToolKind::Command);

        for input in [
            serde_json::json!({"action": "exec", "command": "python3 -i -q", "background": true}),
            serde_json::json!({"action": "stdin", "process_id": "proc_1", "stdin_data": "print(1)\n"}),
            serde_json::json!({"action": "wait", "process_id": "proc_1"}),
            serde_json::json!({"action": "cancel", "process_id": "proc_1"}),
        ] {
            let decision =
                policy.validate_tool_invocation(&def, &tool_invocation("process", input));
            assert_eq!(
                decision,
                SecurityDecision::NeedsApproval(SecurityRisk::Command)
            );
        }
    }

    #[test]
    fn accept_edits_allows_writes_but_asks_for_commands() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(project.join("src")).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let config = SecurityConfig {
            permission_mode: PermissionMode::AcceptEdits,
            ..SecurityConfig::default()
        };
        let policy = policy_with_config(project, data, config);

        let write_decision = policy.validate_tool_invocation(
            &tool_def("write_file", ToolKind::Write),
            &tool_invocation("write_file", serde_json::json!({ "path": "src/lib.rs" })),
        );
        let command_decision = policy.validate_tool_invocation(
            &tool_def("bash", ToolKind::Command),
            &tool_invocation("bash", serde_json::json!({ "command": "cargo test" })),
        );

        assert_eq!(write_decision, SecurityDecision::Allow);
        assert_eq!(
            command_decision,
            SecurityDecision::NeedsApproval(SecurityRisk::Command)
        );
    }

    #[test]
    fn yolo_mode_allows_commands_after_safety_checks() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let config = SecurityConfig {
            permission_mode: PermissionMode::Yolo,
            ..SecurityConfig::default()
        };
        let policy = policy_with_config(project, data, config);

        let decision = policy.validate_tool_invocation(
            &tool_def("bash", ToolKind::Command),
            &tool_invocation("bash", serde_json::json!({ "command": "cargo test" })),
        );

        assert_eq!(decision, SecurityDecision::Allow);
    }

    #[test]
    fn yolo_mode_still_denies_blocked_commands() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let config = SecurityConfig {
            permission_mode: PermissionMode::Yolo,
            ..SecurityConfig::default()
        };
        let policy = policy_with_config(project, data, config);

        let decision = policy.validate_tool_invocation(
            &tool_def("bash", ToolKind::Command),
            &tool_invocation("bash", serde_json::json!({ "command": "sudo true" })),
        );

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    #[test]
    fn auto_mode_allows_non_guarded_commands() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let config = SecurityConfig {
            permission_mode: PermissionMode::Auto,
            ..SecurityConfig::default()
        };
        let policy = policy_with_config(project, data, config);

        let decision = policy.validate_tool_invocation(
            &tool_def("bash", ToolKind::Command),
            &tool_invocation("bash", serde_json::json!({ "command": "cargo test" })),
        );

        assert_eq!(decision, SecurityDecision::Allow);
    }

    #[test]
    fn auto_mode_allows_non_destructive_git_commands() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let config = SecurityConfig {
            permission_mode: PermissionMode::Auto,
            ..SecurityConfig::default()
        };
        let policy = policy_with_config(project, data, config);

        for command in [
            "git status",
            "git add .",
            "git commit -m test",
            "git diff",
            "git log --oneline",
            "git branch",
            "git checkout -b feature",
            "git switch main",
            "git restore src/main.rs",
            "git stash push -m wip",
            "git pull --ff-only",
            "git fetch origin",
        ] {
            let decision = policy.validate_tool_invocation(
                &tool_def("bash", ToolKind::Command),
                &tool_invocation("bash", serde_json::json!({ "command": command })),
            );
            assert_eq!(
                decision,
                SecurityDecision::Allow,
                "expected non-destructive git command to be allowed: {command}"
            );
        }
    }

    #[test]
    fn auto_mode_requires_approval_for_guarded_commands() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let config = SecurityConfig {
            permission_mode: PermissionMode::Auto,
            ..SecurityConfig::default()
        };
        let policy = policy_with_config(project, data, config);

        for command in [
            "git push origin main",
            "git rm -r src",
            "git reset --hard HEAD~1",
            "git clean -fd",
            "git rebase origin/main",
            "git branch -D old-feature",
            "git tag -d v1.0.0",
            "git stash drop",
            "git worktree remove ../wt",
            "git remote remove origin",
            "git reflog delete HEAD@{1}",
            "git -C /tmp/repo push origin main",
        ] {
            let decision = policy.validate_tool_invocation(
                &tool_def("bash", ToolKind::Command),
                &tool_invocation("bash", serde_json::json!({ "command": command })),
            );
            assert_eq!(
                decision,
                SecurityDecision::NeedsApproval(SecurityRisk::GuardedCommand),
                "expected destructive git command to require approval: {command}"
            );
        }
    }

    #[test]
    fn auto_mode_allows_writes() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let config = SecurityConfig {
            permission_mode: PermissionMode::Auto,
            restrict_paths_to_project: true,
            ..SecurityConfig::default()
        };
        let policy = policy_with_config(project, data, config);

        let decision = policy.validate_tool_invocation(
            &tool_def("write_file", ToolKind::Write),
            &tool_invocation(
                "write_file",
                serde_json::json!({ "path": "src/main.rs", "content": "fn main() {}" }),
            ),
        );

        assert_eq!(decision, SecurityDecision::Allow);
    }

    #[test]
    fn yolo_mode_allows_guarded_commands() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let config = SecurityConfig {
            permission_mode: PermissionMode::Yolo,
            ..SecurityConfig::default()
        };
        let policy = policy_with_config(project, data, config);

        for command in [
            "git commit -m test",
            "git push origin main",
            "git rm -r src",
            "git rebase origin/main",
        ] {
            let decision = policy.validate_tool_invocation(
                &tool_def("bash", ToolKind::Command),
                &tool_invocation("bash", serde_json::json!({ "command": command })),
            );
            assert_eq!(
                decision,
                SecurityDecision::Allow,
                "expected YOLO to allow guarded/non-guarded git command: {command}"
            );
        }
    }

    #[test]
    fn auto_mode_still_denies_blocked_commands() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let config = SecurityConfig {
            permission_mode: PermissionMode::Auto,
            ..SecurityConfig::default()
        };
        let policy = policy_with_config(project, data, config);

        let decision = policy.validate_tool_invocation(
            &tool_def("bash", ToolKind::Command),
            &tool_invocation("bash", serde_json::json!({ "command": "sudo true" })),
        );

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    #[test]
    fn tool_deny_rule_wins_over_yolo_mode() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let config = SecurityConfig {
            permission_mode: PermissionMode::Yolo,
            deny_tools: vec!["bash".to_string()],
            ..SecurityConfig::default()
        };
        let policy = policy_with_config(project, data, config);

        let decision = policy.validate_tool_invocation(
            &tool_def("bash", ToolKind::Command),
            &tool_invocation("bash", serde_json::json!({ "command": "cargo test" })),
        );

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    #[test]
    fn tool_allow_rule_can_accept_named_tool_in_restricted_mode() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(project.join("src")).expect("project");
        std::fs::write(project.join("src/lib.rs"), "").expect("file");
        std::fs::create_dir_all(&data).expect("data");
        let config = SecurityConfig {
            allow_tools: vec!["read_file".to_string()],
            ..SecurityConfig::default()
        };
        let policy = policy_with_config(project, data, config);

        let decision = policy.validate_tool_invocation(
            &tool_def("read_file", ToolKind::Read),
            &tool_invocation("read_file", serde_json::json!({ "path": "src/lib.rs" })),
        );

        assert_eq!(decision, SecurityDecision::Allow);
    }

    #[test]
    fn regex_tool_rules_can_force_approval_in_yolo_mode() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let config = SecurityConfig {
            permission_mode: PermissionMode::Yolo,
            ask_tool_regex: vec!["^plugin__".to_string()],
            ..SecurityConfig::default()
        };
        let policy = policy_with_config(project, data, config);

        let decision = policy.validate_tool_invocation(
            &tool_def("plugin__deploy", ToolKind::Custom),
            &tool_invocation("plugin__deploy", serde_json::json!({})),
        );

        assert_eq!(
            decision,
            SecurityDecision::NeedsApproval(SecurityRisk::ExternalPlugin)
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

        let decision = policy.validate_command("/bin/sudo");

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    #[test]
    fn allows_regular_project_memory_named_file() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(project.join("docs")).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project.clone(), data);

        let decision = policy.validate_path(project.join("docs/MEMORY.md").as_path(), true);

        assert_eq!(
            decision,
            SecurityDecision::NeedsApproval(SecurityRisk::Write)
        );
    }

    #[test]
    fn denies_project_side_agent_memory_direct_access() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(project.join(".agent-memory")).expect("memory");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project.clone(), data);

        let decision =
            policy.validate_path(project.join(".agent-memory/MEMORY.md").as_path(), true);

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    #[test]
    fn denies_bash_redirection_to_project_side_agent_memory() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(project.join(".agent-memory")).expect("memory");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        let decision = policy.validate_command("cat >> .agent-memory/MEMORY.md <<'EOF'\ntext\nEOF");

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    #[test]
    fn denies_bash_redirection_to_data_dir_memory() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(data.join("memory/projects/abc")).expect("memory");
        let policy = policy(project, data.clone());
        let target = data.join("memory/projects/abc/MEMORY.md");

        let decision =
            policy.validate_command(&format!("cat >> {} <<'EOF'\ntext\nEOF", target.display()));

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    #[test]
    fn denies_bash_reference_to_data_dir() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data.clone());

        let decision = policy.validate_command(&format!("ls {}", data.display()));

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    #[test]
    fn allows_bash_reference_to_data_dir_plugins() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        let plugins = data.join("plugins");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&plugins).expect("plugins");
        let policy = policy(project, data);

        let decision = policy.validate_command(&format!("ls {}", plugins.display()));

        assert_eq!(
            decision,
            SecurityDecision::NeedsApproval(SecurityRisk::Command)
        );
    }

    #[test]
    fn allows_data_dir_plugins_path() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        let plugins = data.join("plugins");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&plugins).expect("plugins");
        let policy = policy(project, data);

        let decision = policy.validate_path(plugins.join("plugin.wasm").as_path(), true);

        assert_eq!(
            decision,
            SecurityDecision::NeedsApproval(SecurityRisk::Write)
        );
    }

    #[test]
    fn denies_tee_write_to_data_dir_memory() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(data.join("memory/projects/abc")).expect("memory");
        let policy = policy(project, data.clone());
        let target = data.join("memory/projects/abc/MEMORY.md");

        let decision =
            policy.validate_command(&format!("printf text | tee -a {}", target.display()));

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    #[test]
    fn denies_bash_redirection_to_unresolved_dynamic_path() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        let decision =
            policy.validate_command("cat >> \"$NAVI_MEMORY_DIR/MEMORY.md\" <<'EOF'\ntext\nEOF");

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    #[test]
    fn allows_bash_redirection_to_regular_project_memory_named_file() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(project.join("docs")).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        let decision = policy.validate_command("cat >> docs/MEMORY.md <<'EOF'\ntext\nEOF");

        assert_eq!(
            decision,
            SecurityDecision::NeedsApproval(SecurityRisk::Command)
        );
    }

    #[test]
    fn extracts_sed_in_place_write_targets() {
        let targets = extract_shell_write_targets("sed -i 's/old/new/' src/lib.rs");

        assert_eq!(targets, vec!["src/lib.rs"]);
    }

    #[test]
    fn extracts_perl_in_place_write_targets() {
        let targets = extract_shell_write_targets("perl -pi -e 's/old/new/' src/lib.rs");

        assert_eq!(targets, vec!["src/lib.rs"]);
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
    fn redacts_tool_requested_input() {
        let event = AgentEvent::ToolRequested(crate::tool::ToolInvocation {
            id: "c1".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({
                "path": "OPENAI_API_KEY=secret123",
                "nested": {"token": "ghp_1234567890abcdef1234"}
            }),
        });
        let redacted = redact_agent_event(&event);
        match redacted {
            AgentEvent::ToolRequested(invocation) => {
                let json = invocation.input.to_string();
                assert!(json.contains("OPENAI_API_KEY=<redacted>"));
                assert!(json.contains("<redacted>"));
                assert!(!json.contains("secret123"));
                assert!(!json.contains("ghp_1234567890abcdef1234"));
            }
            _ => panic!("expected ToolRequested"),
        }
    }

    #[test]
    fn redacts_tool_completed_output() {
        let event = AgentEvent::ToolCompleted(crate::tool::ToolResult {
            invocation_id: "c1".to_string(),
            ok: true,
            output: serde_json::json!({"stdout": "token sk-proj-1234567890abcdef"}),
        });
        let redacted = redact_agent_event(&event);
        match redacted {
            AgentEvent::ToolCompleted(result) => {
                let json = result.output.to_string();
                assert!(json.contains("<redacted>"));
                assert!(!json.contains("sk-proj-1234567890abcdef"));
            }
            _ => panic!("expected ToolCompleted"),
        }
    }

    #[test]
    fn redacts_snapshot_events_in_bulk() {
        let events = vec![
            AgentEvent::UserTaskSubmitted {
                text: "OPENAI_API_KEY=sk-proj-1234567890abcdef".to_string(),
                content_parts: vec![],
                submitted_at: None,
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

    // ── Regression tests ──────────────────────────────────────────────────────

    #[cfg(unix)]
    #[test]
    fn regression_symlink_attack_allowed_when_not_restricted() {
        use std::os::unix::fs::symlink;

        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        let outside = tempdir.path().join("outside");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(outside.join("secret.txt"), "secret").expect("write");

        // Symlink inside project points to outside file
        let link = project.join("link.txt");
        symlink(outside.join("secret.txt"), &link).expect("symlink");

        let policy = policy(project.clone(), data);
        let decision = policy.validate_path(&link, false);

        assert_eq!(decision, SecurityDecision::Allow);
    }

    #[test]
    fn regression_path_traversal_allowed_when_not_restricted() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project.clone(), data);

        let traversal = project.join("../../../etc/passwd");
        let decision = policy.validate_path(&traversal, false);

        assert_eq!(decision, SecurityDecision::Allow);
    }

    #[test]
    fn regression_command_full_path_extracts_basename() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        // /usr/bin/sudo should extract "sudo" and deny it
        let decision = policy.validate_command("/usr/bin/sudo");
        assert!(
            matches!(decision, SecurityDecision::Deny(_)),
            "full-path blocked command must be denied, got: {decision:?}"
        );
    }

    #[test]
    fn regression_command_sudo_with_args_denied() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        let decision = policy.validate_command("sudo rm -rf /");
        assert!(
            matches!(decision, SecurityDecision::Deny(_)),
            "sudo with args must be denied, got: {decision:?}"
        );
    }

    #[test]
    fn regression_data_dir_path_denied() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data.clone());

        let decision = policy.validate_path(data.join("sessions/test.json").as_path(), false);
        assert!(
            matches!(decision, SecurityDecision::Deny(_)),
            "NAVI data dir must be denied, got: {decision:?}"
        );
    }

    #[test]
    fn regression_validate_patch_mixed_files_allowed_when_not_restricted() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project.clone(), data);

        // One valid file, one outside file - both allowed when not restricted
        let patch = PatchProposal {
            id: "p1".to_string(),
            summary: "edit".to_string(),
            files: vec![
                project.join("src/lib.rs"),
                tempdir.path().join("outside.txt"),
            ],
            unified_diff: String::new(),
        };

        assert_eq!(
            policy.validate_patch(&patch),
            SecurityDecision::NeedsApproval(SecurityRisk::Write)
        );
    }

    #[test]
    fn regression_redact_github_token() {
        // GITHUB_TOKEN is not in is_secret_assignment_name (which checks for
        // API_KEY, ACCESS_TOKEN, AUTH_TOKEN, SECRET, or exact TOKEN).
        // The value ghp_xxx IS caught by looks_like_secret_token.
        let text = "ghp_1234567890abcdef1234567890abcdef12";
        let redacted = redact_secrets(text);
        assert!(
            redacted.contains("<redacted>"),
            "ghp_ token value must be redacted"
        );
        assert!(
            !redacted.contains("ghp_1234567890abcdef1234567890abcdef12"),
            "token value must not appear"
        );
    }

    #[test]
    fn regression_redact_hf_token() {
        let text = "HF_TOKEN=hf_1234567890abcdef1234567890abcdef12";
        let redacted = redact_secrets(text);
        assert!(redacted.contains("<redacted>"), "HF_TOKEN must be redacted");
    }

    #[test]
    fn regression_redact_short_sk_not_clobbered() {
        // Short sk- values that don't look like real tokens should NOT be redacted
        let text = "use sk-abc for testing";
        let redacted = redact_secrets(text);
        assert_eq!(redacted, text, "short sk- value must not be redacted");
    }

    #[test]
    fn regression_redact_empty_string() {
        assert_eq!(redact_secrets(""), "");
    }

    #[test]
    fn regression_redact_nested_json_array() {
        let value = serde_json::json!({
            "items": [
                {"key": "OPENAI_API_KEY=sk-proj-1234567890abcdef"},
                {"key": "normal value"}
            ]
        });
        let redacted = redact_json_value(&value);
        let items = redacted["items"].as_array().unwrap();
        assert!(items[0]["key"].as_str().unwrap().contains("<redacted>"));
        assert_eq!(items[1]["key"].as_str().unwrap(), "normal value");
    }

    #[test]
    fn regression_mark_feature_done_denies_blocked_verification_steps() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        let decision = policy.validate_tool_invocation(
            &ToolDefinition {
                name: "mark_feature_done".to_string(),
                description: String::new(),
                kind: ToolKind::Command,
                input_schema: serde_json::json!({}),
                ..Default::default()
            },
            &ToolInvocation {
                id: "done".to_string(),
                tool_name: "mark_feature_done".to_string(),
                input: serde_json::json!({
                    "feature_id": "danger",
                    "verification_steps": ["sudo rm -rf /"]
                }),
            },
        );

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    #[test]
    fn regression_command_cwd_outside_project_denied() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        let outside = tempdir.path().join("outside");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        std::fs::create_dir_all(&outside).expect("outside");
        let policy = SecurityPolicy::new(
            project,
            data,
            SecurityConfig {
                restrict_paths_to_project: true,
                ..SecurityConfig::default()
            },
        )
        .expect("policy");

        let decision = policy.validate_tool_invocation(
            &ToolDefinition {
                name: "verifier".to_string(),
                description: String::new(),
                kind: ToolKind::Command,
                input_schema: serde_json::json!({}),
                ..Default::default()
            },
            &ToolInvocation {
                id: "verify".to_string(),
                tool_name: "verifier".to_string(),
                input: serde_json::json!({
                    "action": "run",
                    "command": "pwd",
                    "cwd": outside
                }),
            },
        );

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    #[test]
    fn regression_apply_patch_structured_git_path_denied() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(project.join(".git")).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        let decision = policy.validate_tool_invocation(
            &ToolDefinition {
                name: "apply_patch".to_string(),
                description: String::new(),
                kind: ToolKind::Write,
                input_schema: serde_json::json!({}),
                ..Default::default()
            },
            &ToolInvocation {
                id: "patch".to_string(),
                tool_name: "apply_patch".to_string(),
                input: serde_json::json!({
                    "patch": "*** Begin Patch\n*** Add File: .git/config\n+bad\n*** End Patch\n"
                }),
            },
        );

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    #[test]
    fn regression_unified_write_direct_git_path_denied() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(project.join(".git")).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        let decision = policy.validate_tool_invocation(
            &ToolDefinition {
                name: "write".to_string(),
                description: String::new(),
                kind: ToolKind::Write,
                input_schema: serde_json::json!({}),
                ..Default::default()
            },
            &ToolInvocation {
                id: "write".to_string(),
                tool_name: "write".to_string(),
                input: serde_json::json!({
                    "path": ".git/config",
                    "content": "bad"
                }),
            },
        );

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    #[test]
    fn regression_apply_patch_structured_traversal_allowed_when_not_restricted() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        let decision = policy.validate_tool_invocation(
            &ToolDefinition {
                name: "apply_patch".to_string(),
                description: String::new(),
                kind: ToolKind::Write,
                input_schema: serde_json::json!({}),
                ..Default::default()
            },
            &ToolInvocation {
                id: "patch".to_string(),
                tool_name: "apply_patch".to_string(),
                input: serde_json::json!({
                    "patch": "*** Begin Patch\n*** Add File: ../outside.txt\n+bad\n*** End Patch\n"
                }),
            },
        );

        assert_eq!(
            decision,
            SecurityDecision::NeedsApproval(SecurityRisk::Write)
        );
    }

    #[test]
    fn regression_apply_patch_unified_git_path_denied() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(project.join(".git")).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        let decision = policy.validate_tool_invocation(
            &ToolDefinition {
                name: "apply_patch".to_string(),
                description: String::new(),
                kind: ToolKind::Write,
                input_schema: serde_json::json!({}),
                ..Default::default()
            },
            &ToolInvocation {
                id: "patch".to_string(),
                tool_name: "apply_patch".to_string(),
                input: serde_json::json!({
                    "patch": "--- a/.git/config\n+++ b/.git/config\n@@ -1 +1 @@\n-old\n+new\n"
                }),
            },
        );

        assert!(matches!(decision, SecurityDecision::Deny(_)));
    }

    // ── Deny list tests ────────────────────────────────────────────────────

    #[test]
    fn deny_list_allows_node_modules_when_empty() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(project.join("node_modules/pkg")).expect("nm");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project.clone(), data);

        let decision =
            policy.validate_path(project.join("node_modules/pkg/index.js").as_path(), false);

        assert_eq!(decision, SecurityDecision::Allow);
    }

    #[test]
    fn deny_list_allows_target_when_empty() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(project.join("target/debug")).expect("target");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project.clone(), data);

        let decision = policy.validate_path(project.join("target/debug/app").as_path(), false);

        assert_eq!(decision, SecurityDecision::Allow);
    }

    #[test]
    fn deny_list_allows_log_files_when_empty() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        std::fs::write(project.join("debug.log"), "logs").expect("log");
        let policy = policy(project.clone(), data);

        let decision = policy.validate_path(project.join("debug.log").as_path(), false);

        assert_eq!(decision, SecurityDecision::Allow);
    }

    #[test]
    fn deny_list_allows_normal_files() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(project.join("src")).expect("src");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project.clone(), data);

        let decision = policy.validate_path(project.join("src/main.rs").as_path(), false);

        assert_eq!(decision, SecurityDecision::Allow);
    }

    #[test]
    fn deny_list_allows_package_lock_when_empty() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        std::fs::write(project.join("package-lock.json"), "{}").expect("lock");
        let policy = policy(project.clone(), data);

        let decision = policy.validate_path(project.join("package-lock.json").as_path(), false);

        assert_eq!(decision, SecurityDecision::Allow);
    }

    #[test]
    fn is_path_denied_glob_pattern_empty_deny_list() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        assert!(!policy.is_path_denied(Path::new("app.log")));
        assert!(!policy.is_path_denied(Path::new("logs/debug.log")));
        assert!(!policy.is_path_denied(Path::new("app.rs")));
    }

    #[test]
    fn is_path_denied_directory_prefix_empty_deny_list() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        assert!(!policy.is_path_denied(Path::new("node_modules/foo/bar.js")));
        assert!(!policy.is_path_denied(Path::new("target/debug/app")));
        assert!(!policy.is_path_denied(Path::new("src/main.rs")));
    }

    #[test]
    fn filter_denied_lines_keeps_all_when_empty_deny_list() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");
        let policy = policy(project, data);

        let text = "src/main.rs:42: fn main() {}\nnode_modules/foo/index.js:1: export {}\nsrc/lib.rs:10: pub fn test()\n";
        let filtered = policy.filter_denied_lines(text);

        assert_eq!(filtered, text);
    }

    #[test]
    fn filter_denied_lines_empty_deny_list() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");

        let config = SecurityConfig {
            deny_paths: vec![],
            ..Default::default()
        };
        let policy = SecurityPolicy::new(project, data, config).expect("policy");

        let text = "node_modules/foo/index.js:1: export {}\n";
        let filtered = policy.filter_denied_lines(text);

        assert_eq!(filtered, text);
    }

    // ── MCP server allowlist tests ──────────────────────────────────────────

    #[test]
    fn mcp_allowlist_empty_allows_all_servers() {
        let config = SecurityConfig::default();
        assert!(config.is_mcp_server_allowed("any-server"));
        assert!(config.is_mcp_server_allowed(""));
    }

    #[test]
    fn mcp_allowlist_blocks_disallowed_servers() {
        let config = SecurityConfig {
            allowed_mcp_servers: vec!["safe-server".to_string()],
            ..Default::default()
        };
        assert!(!config.is_mcp_server_allowed("unsafe-server"));
        assert!(config.is_mcp_server_allowed("safe-server"));
    }

    #[test]
    fn validate_mcp_server_respects_allowlist() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");

        let config = SecurityConfig {
            allowed_mcp_servers: vec!["trusted".to_string()],
            ..Default::default()
        };
        let policy = SecurityPolicy::new(project, data, config).expect("policy");

        assert_eq!(
            policy.validate_mcp_server("trusted"),
            SecurityDecision::Allow
        );
        assert!(matches!(
            policy.validate_mcp_server("untrusted"),
            SecurityDecision::Deny(_)
        ));
    }

    #[test]
    fn validate_mcp_server_empty_allowlist_allows_all() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project = tempdir.path().join("project");
        let data = tempdir.path().join("data");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(&data).expect("data");

        let config = SecurityConfig::default();
        let policy = SecurityPolicy::new(project, data, config).expect("policy");

        assert_eq!(
            policy.validate_mcp_server("any-server"),
            SecurityDecision::Allow
        );
    }
}
