//! Host embedding profiles: tool visibility, security posture, and system prompts.
//!
//! These knobs let desktop/Electron hosts run NAVI without the default code-agent
//! tool surface or prompt, without forking the agent loop.

use navi_core::{
    DefaultPromptBuilder, PermissionMode, PromptBuilder, PromptCache, RenderedPrompt,
    SecurityConfig, SystemPromptInput,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

/// Which built-in tools the model may see for a host-built engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NaviToolProfile {
    /// Full code-agent surface (default): project bash/edit/read tools, plugins, MCP.
    #[default]
    CodeAgent,
    /// Only host-registered tools (no project bash/edit builtins).
    HostToolsOnly,
    /// No tools exposed to the model.
    ChatOnly,
}

impl NaviToolProfile {
    /// Parse a profile id from host config / NAPI (`code_agent`, `host_tools_only`, `chat_only`).
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "code_agent" | "default" | "code" => Some(Self::CodeAgent),
            "host_tools_only" | "host_only" | "host" => Some(Self::HostToolsOnly),
            "chat_only" | "chat" | "no_tools" | "none" => Some(Self::ChatOnly),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodeAgent => "code_agent",
            Self::HostToolsOnly => "host_tools_only",
            Self::ChatOnly => "chat_only",
        }
    }
}

/// Base system-prompt identity for a host-built engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NaviPromptProfile {
    /// Default terminal code agent (inspect, edit, verify).
    #[default]
    CodeAgent,
    /// Non-code assistant / creative conversational agent.
    Assistant,
}

impl NaviPromptProfile {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "code_agent" | "default" | "code" => Some(Self::CodeAgent),
            "assistant" | "creative" | "chat" | "non_code" | "noncode" => Some(Self::Assistant),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodeAgent => "code_agent",
            Self::Assistant => "assistant",
        }
    }
}

/// Security posture applied when building a host-facing engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NaviSecurityProfile {
    /// Existing code-agent defaults (config-driven permission mode).
    #[default]
    CodeAgent,
    /// Host / vault apps: writes and commands approval-gated; permissive is opt-in.
    HostApp,
}

impl NaviSecurityProfile {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "code_agent" | "default" | "code" => Some(Self::CodeAgent),
            "host_app" | "host" | "vault" | "restricted" => Some(Self::HostApp),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodeAgent => "code_agent",
            Self::HostApp => "host_app",
        }
    }

    /// Security config overlay for this profile (merged onto existing config).
    pub fn apply(self, security: &mut SecurityConfig) {
        match self {
            Self::CodeAgent => {}
            Self::HostApp => {
                // Write-kind and command tools need approval; never default to YOLO.
                security.permission_mode = PermissionMode::Restricted;
                security.redact_secrets_in_sessions = true;
            }
        }
    }
}

/// Pure tool-name filter used by the engine and unit tests.
///
/// - `code_agent`: keep all names (then apply optional allow/deny).
/// - `host_tools_only`: keep only `host_tool_names`.
/// - `chat_only`: keep nothing.
///
/// Optional `allow_tools` (when non-empty) further restricts to that set.
/// `deny_tools` always removes matching names.
pub fn filter_tool_names(
    registered: &[String],
    profile: NaviToolProfile,
    host_tool_names: &HashSet<String>,
    allow_tools: &[String],
    deny_tools: &[String],
) -> Vec<String> {
    let deny: HashSet<&str> = deny_tools.iter().map(|s| s.as_str()).collect();
    let allow: Option<HashSet<&str>> = if allow_tools.is_empty() {
        None
    } else {
        Some(allow_tools.iter().map(|s| s.as_str()).collect())
    };

    let base: Vec<String> = match profile {
        NaviToolProfile::CodeAgent => registered.to_vec(),
        NaviToolProfile::HostToolsOnly => registered
            .iter()
            .filter(|n| host_tool_names.contains(n.as_str()))
            .cloned()
            .collect(),
        NaviToolProfile::ChatOnly => Vec::new(),
    };

    base.into_iter()
        .filter(|n| {
            if deny.contains(n.as_str()) {
                return false;
            }
            if let Some(ref allow) = allow {
                return allow.contains(n.as_str());
            }
            true
        })
        .collect()
}

/// Non-code assistant system prompt (identity + light workflow).
///
/// Skills and host tools still attach on top via the normal renderer path when
/// used through [`ProfilePromptBuilder`].
pub fn assistant_system_prompt(project_hint: &str) -> String {
    format!(
        concat!(
            "You are NAVI in assistant mode — a helpful, creative conversational agent.\n",
            "You are not a terminal code agent. Do not assume a project workspace, shell, or file editor unless tools are provided.\n",
            "Context: {project}.\n",
            "\n",
            "Guidelines:\n",
            "1. Prefer clear, direct answers and creative collaboration over code-repository workflows.\n",
            "2. When tools are available, use them only when they help the user; otherwise answer in prose.\n",
            "3. Do not invent tool results or claim you edited files unless a tool reported success.\n",
            "4. Stay helpful for writing, roleplay, planning, and product assistance.\n",
        ),
        project = project_hint
    )
}

/// Prompt builder that switches identity by [`NaviPromptProfile`].
///
/// Skills, AGENTS.md, and memory injection still flow through the default
/// renderer for `code_agent`. For `assistant`, the base instructions use
/// [`assistant_system_prompt`] and skills still appear as developer messages
/// when present in the input.
#[derive(Debug, Default)]
pub struct ProfilePromptBuilder {
    profile: NaviPromptProfile,
}

impl ProfilePromptBuilder {
    pub fn new(profile: NaviPromptProfile) -> Self {
        Self { profile }
    }

    pub fn profile(&self) -> NaviPromptProfile {
        self.profile
    }
}

impl PromptBuilder for ProfilePromptBuilder {
    fn build(&self, input: SystemPromptInput, cache: Arc<PromptCache>) -> RenderedPrompt {
        match self.profile {
            NaviPromptProfile::CodeAgent => DefaultPromptBuilder.build(input, cache),
            NaviPromptProfile::Assistant => {
                let project = input.project_dir.display().to_string();
                let mut rendered = DefaultPromptBuilder.build(input, cache);
                // Replace the code-agent base identity while keeping dynamic
                // developer messages (skills, context packets, memory).
                rendered.instructions = assistant_system_prompt(&project);
                rendered
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use navi_core::{NaviConfig, PromptCache};
    use std::path::PathBuf;

    #[test]
    fn filter_host_tools_only_drops_builtins() {
        let registered = vec![
            "bash".into(),
            "edit".into(),
            "vault_read".into(),
            "read_file".into(),
        ];
        let mut hosts = HashSet::new();
        hosts.insert("vault_read".into());
        let kept = filter_tool_names(
            &registered,
            NaviToolProfile::HostToolsOnly,
            &hosts,
            &[],
            &[],
        );
        assert_eq!(kept, vec!["vault_read".to_string()]);
    }

    #[test]
    fn filter_chat_only_keeps_nothing() {
        let registered = vec!["bash".into(), "vault_read".into()];
        let hosts = HashSet::new();
        let kept = filter_tool_names(&registered, NaviToolProfile::ChatOnly, &hosts, &[], &[]);
        assert!(kept.is_empty());
    }

    #[test]
    fn filter_allow_deny_on_code_agent() {
        let registered = vec!["bash".into(), "edit".into(), "read_file".into()];
        let hosts = HashSet::new();
        let kept = filter_tool_names(
            &registered,
            NaviToolProfile::CodeAgent,
            &hosts,
            &["read_file".into(), "bash".into()],
            &["bash".into()],
        );
        assert_eq!(kept, vec!["read_file".to_string()]);
    }

    #[test]
    fn assistant_prompt_differs_from_code_agent() {
        let cache = Arc::new(PromptCache::new());
        let input = || SystemPromptInput {
            config: NaviConfig::default(),
            project_dir: PathBuf::from("/tmp/proj"),
            memory_injection: None,
            tools: Vec::new(),
            include_tool_prompt_manifest: false,
            context_packets: Vec::new(),
            available_skills: Vec::new(),
            active_skills: Vec::new(),
            harness_card: None,
        };
        let code = ProfilePromptBuilder::new(NaviPromptProfile::CodeAgent)
            .build(input(), cache.clone())
            .instructions;
        let assistant = ProfilePromptBuilder::new(NaviPromptProfile::Assistant)
            .build(input(), cache)
            .instructions;
        assert!(code.contains("code agent") || code.contains("autonomous"));
        assert!(assistant.contains("assistant mode"));
        assert_ne!(code, assistant);
        assert!(!assistant.contains("autonomous code agent"));
    }

    #[test]
    fn host_app_security_is_restricted_not_yolo() {
        let mut sec = SecurityConfig::default();
        sec.permission_mode = PermissionMode::Yolo;
        NaviSecurityProfile::HostApp.apply(&mut sec);
        assert_eq!(sec.permission_mode, PermissionMode::Restricted);
    }

    #[test]
    fn profile_parse_accepts_aliases() {
        assert_eq!(
            NaviToolProfile::parse("host-tools-only"),
            Some(NaviToolProfile::HostToolsOnly)
        );
        assert_eq!(
            NaviPromptProfile::parse("creative"),
            Some(NaviPromptProfile::Assistant)
        );
        assert_eq!(
            NaviSecurityProfile::parse("vault"),
            Some(NaviSecurityProfile::HostApp)
        );
    }
}
