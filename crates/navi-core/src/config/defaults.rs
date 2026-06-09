use crate::compact::{
    AUTOCOMPACT_BUFFER_TOKENS, ERROR_THRESHOLD_BUFFER_TOKENS, MAX_CONSECUTIVE_FAILURES,
    MAX_OUTPUT_TOKENS_FOR_SUMMARY, WARNING_THRESHOLD_BUFFER_TOKENS,
};
use crate::config::types::{
    ApprovalConfig, HarnessConfig, HarnessProfile, LoggingConfig, MemoryConfig, ModelConfig,
    SecurityConfig, ToolPromptManifest,
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
            restrict_paths_to_project: true,
            protect_git_metadata: true,
            redact_secrets_in_sessions: true,
            allow_external_plugins: false,
            blocked_commands: default_blocked_commands(),
            deny_paths: default_deny_paths(),
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
pub(crate) const DEFAULT_CONTEXT_WINDOW: u64 = 128_000;

fn default_blocked_commands() -> Vec<String> {
    [
        "rm", "rmdir", "shred", "mkfs", "dd", "sudo", "su", "doas", "chmod", "chown", "chgrp",
        "mount", "umount", "reboot", "shutdown", "poweroff",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn default_deny_paths() -> Vec<String> {
    [
        // Dependencies (large, machine-generated)
        "node_modules",
        "vendor",
        ".venv",
        "venv",
        "__pycache__",
        ".tox",
        // Build artifacts
        "target",
        "dist",
        "build",
        "out",
        ".next",
        ".nuxt",
        ".output",
        // Cache directories
        ".cache",
        ".parcel-cache",
        ".turbo",
        ".eslintcache",
        // Large generated files
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "Cargo.lock",
        "composer.lock",
        "poetry.lock",
        // Coverage/test output
        "coverage",
        ".nyc_output",
        "htmlcov",
        // Log files
        "*.log",
        "npm-debug.log*",
        "yarn-debug.log*",
        "yarn-error.log*",
        // IDE/editor
        ".idea",
        ".vscode",
        // OS files
        ".DS_Store",
        "Thumbs.db",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}
