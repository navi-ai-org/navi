use crate::config::types::{
    ApprovalConfig, HarnessConfig, HarnessProfile, LoggingConfig, McpConfig, MemoryConfig,
    ModelConfig, NaviConfig, SecurityConfig, SkillsConfig, ToolPromptManifest,
};

impl Default for NaviConfig {
    fn default() -> Self {
        Self {
            model: ModelConfig::default(),
            harness: HarnessConfig::default(),
            approvals: ApprovalConfig::default(),
            security: SecurityConfig::default(),
            logging: LoggingConfig::default(),
            providers: Vec::new(),
            plugins: Vec::new(),
            memory: MemoryConfig::default(),
            skills: SkillsConfig::default(),
            mcp: McpConfig::default(),
        }
    }
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            dirs: Vec::new(),
            active: Vec::new(),
        }
    }
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            servers: Vec::new(),
        }
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            name: "gpt-5.5".to_string(),
        }
    }
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            profile: HarnessProfile::Auto,
            tool_prompt_manifest: ToolPromptManifest::Auto,
            observation_bytes_small: 12 * 1024,
            observation_bytes_medium: 48 * 1024,
            micro_compact_gap_minutes: 60,
            autocompact_buffer_tokens: 13_000,
            autocompact_warning_buffer_tokens: 20_000,
            autocompact_error_buffer_tokens: 20_000,
            autocompact_max_output_tokens: 20_000,
            autocompact_max_consecutive_failures: 3,
        }
    }
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            allow_reads: true,
            require_for_writes: true,
            require_for_commands: true,
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            restrict_paths_to_project: true,
            protect_git_metadata: true,
            redact_secrets_in_sessions: true,
            allow_external_plugins: false,
            blocked_commands: default_blocked_commands(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            level: "info".to_string(),
            file_enabled: true,
            stdout_enabled: false,
            retention_days: 14,
            max_files: 30,
            include_payloads: false,
        }
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            session_memory_enabled: false,
            max_memory_entries: 3,
        }
    }
}

impl PartialEq for ModelConfig {
    fn eq(&self, other: &Self) -> bool {
        self.provider == other.provider && self.name == other.name
    }
}

/// Default context window size in tokens when the model's context window is unknown.
pub const DEFAULT_CONTEXT_WINDOW: u64 = 128_000;

fn default_blocked_commands() -> Vec<String> {
    [
        "rm", "rmdir", "shred", "mkfs", "dd", "sudo", "su", "doas", "chmod", "chown", "chgrp",
        "mount", "umount", "reboot", "shutdown", "poweroff",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}
