use crate::compact::{
    AUTOCOMPACT_BUFFER_TOKENS, ERROR_THRESHOLD_BUFFER_TOKENS, MAX_CONSECUTIVE_FAILURES,
    MAX_OUTPUT_TOKENS_FOR_SUMMARY, WARNING_THRESHOLD_BUFFER_TOKENS,
};
use crate::config::types::{
    ApprovalConfig, HarnessConfig, HarnessProfile, HistoryConfig, LoggingConfig, MemoryConfig,
    ModelConfig, PermissionMode, SecurityConfig, ToolPromptManifest,
};

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
            max_turn_loops_small: 40,
            max_turn_loops_medium: 100,
            max_turn_loops_long_running: 80,
            turn_loop_limit: None,
            max_tool_calls_small: 40,
            max_tool_calls_medium: 100,
            max_parallel_tool_calls_small: 4,
            max_parallel_tool_calls_medium: 8,
            max_parallel_tool_calls_long_running: 4,
            max_consecutive_tool_errors: 4,
            max_consecutive_invalid_arguments: 3,
            max_consecutive_malformed_arguments: 2,
            max_consecutive_unknown_tools: 3,
            micro_compact_gap_minutes: 60,
            autocompact_buffer_tokens: AUTOCOMPACT_BUFFER_TOKENS,
            autocompact_warning_buffer_tokens: WARNING_THRESHOLD_BUFFER_TOKENS,
            autocompact_error_buffer_tokens: ERROR_THRESHOLD_BUFFER_TOKENS,
            autocompact_max_output_tokens: MAX_OUTPUT_TOKENS_FOR_SUMMARY,
            autocompact_max_consecutive_failures: MAX_CONSECUTIVE_FAILURES,
            autocompact_keep_ratio: 0.25,
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
            permission_mode: PermissionMode::Restricted,
            allow_tools: Vec::new(),
            allow_tool_regex: Vec::new(),
            ask_tools: Vec::new(),
            ask_tool_regex: Vec::new(),
            deny_tools: Vec::new(),
            deny_tool_regex: Vec::new(),
            restrict_paths_to_project: false,
            protect_git_metadata: true,
            redact_secrets_in_sessions: true,
            allow_external_plugins: false,
            blocked_commands: default_blocked_commands(),
            deny_paths: Vec::new(),
            allowed_mcp_servers: Vec::new(),
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
            enabled: true,
            root: "memory/projects".to_string(),
            global_memory_path: "~/.code-agent/global-memory.md".to_string(),
            checkpoint_thresholds: vec![0.20, 0.45, 0.70],
            rebuild_threshold: 0.85,
            injected_context_token_budget: 65000,
            dream_interval_days: 1,
            distill_interval_days: 30,
            embedding_model_path: String::new(),
            embedding_tokenizer_path: String::new(),
            history: HistoryConfig::default(),
        }
    }
}

impl PartialEq for ModelConfig {
    fn eq(&self, other: &Self) -> bool {
        self.provider == other.provider && self.name == other.name
    }
}

/// Default context window size in tokens when the model's context window is unknown.
pub(crate) const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;

fn default_blocked_commands() -> Vec<String> {
    [
        "rmdir", "shred", "mkfs", "dd", "sudo", "su", "doas", "chmod", "chown", "chgrp", "mount",
        "umount", "reboot", "shutdown", "poweroff",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}
