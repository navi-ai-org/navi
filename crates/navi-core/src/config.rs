pub mod defaults;
pub mod persistence;
pub mod providers;
pub mod types;

pub use persistence::*;
pub use providers::*;
pub use types::*;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn project_config_overrides_safe_settings_but_not_execution_hooks() {
        let mut global = NaviConfig {
            model: ModelConfig {
                provider: "openai".to_string(),
                name: "gpt-5.5".to_string(),
            },
            harness: HarnessConfig::default(),
            approvals: ApprovalConfig {
                allow_reads: true,
                require_for_writes: true,
                require_for_commands: true,
            },
            security: SecurityConfig::default(),
            logging: LoggingConfig::default(),
            providers: Vec::new(),
            plugins: vec![PluginConfig {
                path: PathBuf::from("/global/plugin.so"),
                enabled: true,
            }],
            memory: MemoryConfig::default(),
            skills: SkillsConfig::default(),
            mcp: McpConfig::default(),
            wasm_plugins: Vec::new(),
            plugin_marketplace: PluginMarketplaceConfig::default(),
            tui: TuiConfig::default(),
        };

        let mut project = NaviConfig {
            model: ModelConfig {
                provider: "openai".to_string(),
                name: "gpt-5.4".to_string(),
            },
            harness: HarnessConfig::default(),
            approvals: ApprovalConfig {
                allow_reads: true,
                require_for_writes: false,
                require_for_commands: true,
            },
            security: SecurityConfig::default(),
            logging: LoggingConfig {
                level: "debug".to_string(),
                ..LoggingConfig::default()
            },
            providers: Vec::new(),
            plugins: vec![PluginConfig {
                path: PathBuf::from("./project-plugin.so"),
                enabled: true,
            }],
            memory: MemoryConfig::default(),
            skills: SkillsConfig::default(),
            mcp: McpConfig {
                enabled: true,
                servers: vec![McpServerConfig {
                    id: "project-mcp".to_string(),
                    command: Some("malicious".to_string()),
                    url: None,
                    args: Vec::new(),
                    env: Default::default(),
                    cwd: None,
                    enabled: true,
                    tool_prefix: None,
                    timeout_ms: None,
                }],
            },
            wasm_plugins: Vec::new(),
            plugin_marketplace: PluginMarketplaceConfig::default(),
            tui: TuiConfig::default(),
        };
        project.plugins.clear();
        project.mcp = McpConfig::default();
        global.merge(project);

        assert_eq!(global.model.name, "gpt-5.4");
        assert!(!global.approvals.require_for_writes);
        assert_eq!(global.logging.level, "debug");
        assert_eq!(global.plugins.len(), 1);
        assert!(!global.mcp.enabled);
    }

    #[test]
    fn parses_skills_and_mcp_config() {
        let config: NaviConfig = toml::from_str(
            r#"
[skills]
enabled = true
dirs = [".navi/skills"]
active = ["socratic"]

[mcp]
enabled = true

[[mcp.servers]]
id = "memory"
command = "memory-mcp-server"
args = ["--stdio"]
enabled = true
tool_prefix = "mem"
timeout_ms = 1000
"#,
        )
        .expect("config parses");

        assert!(config.skills.enabled);
        assert_eq!(config.skills.active, vec!["socratic"]);
        assert!(config.mcp.enabled);
        assert_eq!(config.mcp.servers[0].id, "memory");
        assert_eq!(config.mcp.servers[0].tool_prefix.as_deref(), Some("mem"));
    }

    #[test]
    fn logging_defaults_are_compact_and_file_backed() {
        let config = NaviConfig::default();

        assert!(config.logging.enabled);
        assert_eq!(config.logging.level, "info");
        assert!(config.logging.file_enabled);
        assert!(!config.logging.stdout_enabled);
        assert!(!config.logging.include_payloads);
    }

    #[test]
    fn memory_defaults_are_disabled_with_max_3_entries() {
        let config = NaviConfig::default();

        assert!(!config.memory.session_memory_enabled);
        assert_eq!(config.memory.max_memory_entries, 3);
    }

    #[test]
    fn built_in_provider_catalog_includes_starting_providers() {
        let config = NaviConfig::default();
        let providers = provider_catalog(&config);

        assert!(providers.iter().any(|provider| provider.id == "openai"));
        assert!(
            providers
                .iter()
                .any(|provider| provider.id == "charm-hyper")
        );
        assert!(providers.iter().any(|provider| provider.id == "opencode"));
        assert!(
            providers
                .iter()
                .any(|provider| provider.id == "commandcode")
        );
        assert_eq!(canonical_provider_id("opencode-zen"), "opencode");
        assert_eq!(
            resolve_provider_config(&config, "opencode-zen")
                .expect("opencode alias")
                .id,
            "opencode"
        );
        let opencode = providers
            .iter()
            .find(|provider| provider.id == "opencode")
            .expect("opencode provider");
        assert_eq!(opencode.api_key_env, "OPENCODE_API_KEY");
        assert!(
            opencode
                .models
                .iter()
                .any(|model| model.name == "big-pickle")
        );
        assert!(
            opencode
                .models
                .iter()
                .any(|model| model.name == "nemotron-3-super-free")
        );
        let commandcode = providers
            .iter()
            .find(|provider| provider.id == "commandcode")
            .expect("commandcode provider");
        assert_eq!(commandcode.api_key_env, "CMD_API_KEY");
        assert_eq!(
            commandcode.base_url.as_deref(),
            Some("https://api.commandcode.ai")
        );
        assert!(
            commandcode
                .models
                .iter()
                .any(|model| model.name == "deepseek/deepseek-v4-flash")
        );
        assert!(
            commandcode
                .models
                .iter()
                .any(|model| model.name == "claude-sonnet-4-6")
        );
        let nvidia = providers
            .iter()
            .find(|provider| provider.id == "nvidia")
            .expect("nvidia provider");
        assert_eq!(
            nvidia.base_url.as_deref(),
            Some("https://integrate.api.nvidia.com/v1")
        );
        assert!(providers.iter().all(|provider| !provider.models.is_empty()));
        assert!(
            providers
                .iter()
                .map(|provider| provider.models.len())
                .sum::<usize>()
                >= 160
        );
    }

    #[test]
    fn tool_prompt_manifest_defaults_to_disabled_for_native_models() {
        let config = NaviConfig::default();
        assert!(!effective_tool_prompt_manifest(&config));
    }

    #[test]
    fn tool_prompt_manifest_can_be_forced_or_model_enabled() {
        let mut config = NaviConfig::default();
        config.harness.tool_prompt_manifest = ToolPromptManifest::Always;
        assert!(effective_tool_prompt_manifest(&config));

        config.harness.tool_prompt_manifest = ToolPromptManifest::Never;
        assert!(!effective_tool_prompt_manifest(&config));

        config.harness.tool_prompt_manifest = ToolPromptManifest::Auto;
        config.model.provider = "compat".to_string();
        config.model.name = "compat-model".to_string();
        config.providers.push(ProviderConfig {
            id: "compat".to_string(),
            label: "Compat".to_string(),
            description: "compat provider".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "COMPAT_KEY".to_string(),
            base_url: Some("https://example.com".to_string()),
            models: vec![ProviderModelConfig {
                name: "compat-model".to_string(),
                task_size: ModelTaskSize::Small,
                context_window_tokens: None,
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                tool_prompt_manifest: Some(true),
            }],
            ..Default::default()
        });
        assert!(effective_tool_prompt_manifest(&config));
    }

    #[test]
    fn custom_provider_config_overrides_built_in_provider() {
        let mut config = NaviConfig::default();
        config.providers.push(ProviderConfig {
            id: "charm-hyper".to_string(),
            label: "Charm Hyper".to_string(),
            description: "Custom override".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "CUSTOM_CHARM_KEY".to_string(),
            base_url: Some("https://example.test/v1".to_string()),
            models: vec![ProviderModelConfig {
                name: "Custom Model".to_string(),
                task_size: ModelTaskSize::Large,
                context_window_tokens: None,
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                tool_prompt_manifest: None,
            }],
            ..Default::default()
        });

        let provider = resolve_provider_config(&config, "charm-hyper").expect("provider");
        assert_eq!(provider.api_key_env, "CUSTOM_CHARM_KEY");
        assert_eq!(provider.models[0].name, "Custom Model");
    }

    #[test]
    fn default_request_options_for_known_providers() {
        let openai = default_request_options_for("openai").expect("openai defaults");
        assert_eq!(openai.prompt_cache_key.as_deref(), Some("openai"));
        assert_eq!(openai.prompt_cache_retention.as_deref(), Some("24h"));
        assert!(openai.anthropic_cache_control.is_none());

        let anthropic = default_request_options_for("anthropic").expect("anthropic defaults");
        assert_eq!(
            anthropic
                .anthropic_cache_control
                .as_ref()
                .and_then(|value| value.get("type"))
                .and_then(serde_json::Value::as_str),
            Some("ephemeral")
        );
        assert!(anthropic.prompt_cache_key.is_none());

        assert!(default_request_options_for("nvidia").is_none());
    }

    #[test]
    fn catalog_fills_default_request_options_for_known_providers() {
        // Built-in providers always come with the right defaults set.
        let config = NaviConfig::default();
        let openai = resolve_provider_config(&config, "openai").expect("openai");
        let openai_opts = openai
            .request_options
            .as_ref()
            .expect("openai request_options filled");
        assert_eq!(openai_opts.prompt_cache_key.as_deref(), Some("openai"));
        assert_eq!(openai_opts.prompt_cache_retention.as_deref(), Some("24h"));

        let anthropic = resolve_provider_config(&config, "anthropic").expect("anthropic");
        let anthropic_opts = anthropic
            .request_options
            .as_ref()
            .expect("anthropic request_options filled");
        assert!(anthropic_opts.anthropic_cache_control.is_some());
    }

    #[test]
    fn catalog_fills_defaults_when_override_replaces_provider_wholesale() {
        // Simulates the user writing a [[providers]] block in config.toml
        // without setting request_options. The merge in merge_provider_configs
        // replaces the cached provider, so defaults would be lost without
        // the post-merge fill step.
        let mut config = NaviConfig::default();
        config.providers.push(ProviderConfig {
            id: "openai".to_string(),
            label: "OpenAI (custom url)".to_string(),
            description: "OpenAI with a proxied base URL".to_string(),
            kind: ProviderKind::OpenAiResponses,
            api_key_env: "OPENAI_API_KEY".to_string(),
            base_url: Some("https://proxy.example/v1".to_string()),
            models: vec![ProviderModelConfig {
                name: "gpt-5".to_string(),
                task_size: ModelTaskSize::Large,
                context_window_tokens: None,
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                tool_prompt_manifest: None,
            }],
            ..Default::default()
        });

        let openai = resolve_provider_config(&config, "openai").expect("openai");
        assert_eq!(openai.base_url.as_deref(), Some("https://proxy.example/v1"));
        // The override did not set request_options, so the catalog fills the
        // canonical OpenAI defaults — otherwise prompt caching would silently
        // stop working.
        let opts = openai
            .request_options
            .as_ref()
            .expect("openai defaults filled even with override");
        assert_eq!(opts.prompt_cache_key.as_deref(), Some("openai"));
        assert_eq!(opts.prompt_cache_retention.as_deref(), Some("24h"));
    }

    #[test]
    fn catalog_respects_explicit_request_options_in_override() {
        // If the user explicitly disables prompt caching, the catalog must
        // honor that — the fill step only applies when request_options is
        // None, not when the user opted out with an explicit Some(empty).
        let mut config = NaviConfig::default();
        config.providers.push(ProviderConfig {
            id: "openai".to_string(),
            label: "OpenAI".to_string(),
            description: "OpenAI with prompt caching disabled".to_string(),
            kind: ProviderKind::OpenAiResponses,
            api_key_env: "OPENAI_API_KEY".to_string(),
            base_url: Some("https://api.openai.com/v1".to_string()),
            models: vec![ProviderModelConfig {
                name: "gpt-5".to_string(),
                task_size: ModelTaskSize::Large,
                context_window_tokens: None,
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                tool_prompt_manifest: None,
            }],
            request_options: Some(ProviderRequestOptions::default()), // explicit opt-out
            ..Default::default()
        });

        let openai = resolve_provider_config(&config, "openai").expect("openai");
        let opts = openai
            .request_options
            .as_ref()
            .expect("explicit value preserved");
        assert!(opts.prompt_cache_key.is_none());
        assert!(opts.prompt_cache_retention.is_none());
    }

    #[test]
    fn catalog_does_not_fill_defaults_for_unknown_providers() {
        let mut config = NaviConfig::default();
        config.providers.push(ProviderConfig {
            id: "my-private-proxy".to_string(),
            label: "Private proxy".to_string(),
            description: "user-supplied".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "PROXY_KEY".to_string(),
            base_url: Some("https://proxy.test/v1".to_string()),
            models: vec![ProviderModelConfig {
                name: "some-model".to_string(),
                task_size: ModelTaskSize::Small,
                context_window_tokens: None,
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                tool_prompt_manifest: None,
            }],
            ..Default::default()
        });

        let provider = resolve_provider_config(&config, "my-private-proxy").expect("provider");
        // No default exists for unknown provider ids, so options stay None.
        assert!(provider.request_options.is_none());
    }
}
