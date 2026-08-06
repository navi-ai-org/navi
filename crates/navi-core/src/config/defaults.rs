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
            self_repair: true,
            self_repair_max_attempts: 1,
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
            guarded_commands: vec!["git".to_string()],
            deny_paths: Vec::new(),
            allowed_mcp_servers: Vec::new(),
            computer_use_deny_apps: default_computer_use_deny_apps(),
            computer_use_enabled: true,
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
            checkpoint_thresholds: vec![0.20, 0.45, 0.70],
            // Above auto-compact (80%) so model summarization runs first;
            // rebuild is a last-resort fallback near the hard ceiling.
            rebuild_threshold: 0.95,
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

/// Default deny-list of applications that computer-use tools must not target
/// (ADR 0016). Matched case-insensitively against the target process exe name
/// (without `.exe`) and the window title substring.
///
/// Enforced in Restricted / AcceptEdits / Auto; bypassed only in Yolo.
/// Project config may **extend** this list but cannot remove the
/// "protected" entries (OS security + NAVI self-protection) — see
/// `merge_deny_apps` in `loader.rs`.
pub(crate) fn default_computer_use_deny_apps() -> Vec<String> {
    [
        // ── Password managers ───────────────────────────────────────────
        "1password",
        "bitwarden",
        "keepass",
        "keepassxc",
        "lastpass",
        "dashlane",
        "enpass",
        "roboform",
        "applepasswords",
        // ── Banking / finance ───────────────────────────────────────────
        "banking",
        "quicken",
        "ynab",
        // ── OS security (Windows) ───────────────────────────────────────
        "securityhealthsystray",
        "windowsdefender",
        "msmpeng",
        "securitycenterhost",
        // ── OS security (macOS — future) ────────────────────────────────
        "systemsettings",
        "keychainaccess",
        // ── Credential stores ───────────────────────────────────────────
        "vaultsvc",
        "credentialmanager",
        "keychain",
        "seahorse",
        // ── NAVI self-protection ────────────────────────────────────────
        "navi",
        "navi-cli",
        "navi-tui",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// Entries in the default deny-list that project config cannot remove.
///
/// These are the "protected" categories from ADR 0016: OS security settings
/// and NAVI self-protection. Project `.navi/config.toml` may add entries but
/// cannot weaken these. (Yolo bypasses the entire deny-list at runtime, so
/// this only affects non-Yolo enforcement.)
pub(crate) fn protected_deny_apps() -> &'static [&'static str] {
    &[
        // OS security (Windows + macOS)
        "securityhealthsystray",
        "windowsdefender",
        "msmpeng",
        "securitycenterhost",
        "systemsettings",
        "keychainaccess",
        "vaultsvc",
        "credentialmanager",
        "keychain",
        "seahorse",
        // NAVI self-protection
        "navi",
        "navi-cli",
        "navi-tui",
    ]
}

/// Merges user-supplied deny-app entries with the defaults, ensuring the
/// protected entries are always present.
///
/// Semantics:
/// - The user's list **replaces** the non-protected defaults. If the user
///   provides `["mybankapp"]`, only `"mybankapp"` plus the protected entries
///   survive — the non-protected defaults (password managers, banking) are
///   dropped unless the user re-lists them.
/// - The protected entries (OS security + NAVI self-protection) are **always**
///   added, regardless of what the user provides.
///
/// This matches ADR 0016: project config may extend the list but cannot
/// weaken the protected categories. Yolo bypasses the entire deny-list at
/// runtime, so this only affects non-Yolo enforcement.
pub(crate) fn merge_deny_apps(user: &[String]) -> Vec<String> {
    let protected = protected_deny_apps();

    // Start from the user's list (it fully replaces non-protected defaults).
    let mut merged: Vec<String> = user.to_vec();

    // Ensure every protected entry is present (case-insensitive).
    for p in protected {
        let p_lower = p.to_ascii_lowercase();
        if !merged.iter().any(|e| e.to_ascii_lowercase() == p_lower) {
            merged.push(p.to_string());
        }
    }

    merged
}

/// Returns `true` if `target` (exe name or window title) matches any entry in
/// the deny-list. Case-insensitive substring match.
pub(crate) fn is_deny_listed(target: &str, deny_apps: &[String]) -> bool {
    let target_lower = target.to_ascii_lowercase();
    deny_apps
        .iter()
        .any(|entry| target_lower.contains(&entry.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_deny_list_includes_password_managers_and_os_security() {
        let apps = default_computer_use_deny_apps();
        assert!(apps.iter().any(|a| a == "1password"));
        assert!(apps.iter().any(|a| a == "bitwarden"));
        assert!(apps.iter().any(|a| a == "keepass"));
        assert!(apps.iter().any(|a| a == "windowsdefender"));
        assert!(apps.iter().any(|a| a == "navi"));
    }

    #[test]
    fn merge_deny_apps_keeps_protected_even_if_user_removes() {
        // User tries to remove all defaults.
        let user: Vec<String> = vec![];
        let merged = merge_deny_apps(&user);
        // Protected entries must still be present.
        assert!(merged.iter().any(|a| a == "navi"));
        assert!(merged.iter().any(|a| a == "windowsdefender"));
        assert!(merged.iter().any(|a| a == "keychainaccess"));
    }

    #[test]
    fn merge_deny_apps_keeps_user_additions() {
        let user = vec!["mybankapp".to_string()];
        let merged = merge_deny_apps(&user);
        assert!(merged.iter().any(|a| a == "mybankapp"));
        // Protected entries still present.
        assert!(merged.iter().any(|a| a == "navi"));
    }

    #[test]
    fn merge_deny_apps_allows_removal_of_non_protected_defaults() {
        // User provides a list that omits "1password" (non-protected).
        let user = vec!["navi".to_string()]; // keep protected, drop 1password
        let merged = merge_deny_apps(&user);
        assert!(
            !merged.iter().any(|a| a == "1password"),
            "user should be able to drop non-protected default '1password'"
        );
        assert!(merged.iter().any(|a| a == "navi"));
    }

    #[test]
    fn is_deny_listed_matches_case_insensitive_substring() {
        let apps = vec!["1Password".to_string(), "navi".to_string()];
        assert!(is_deny_listed("1Password 8.exe", &apps));
        assert!(is_deny_listed("NAVI-CLI", &apps));
        assert!(!is_deny_listed("notepad", &apps));
    }
}
