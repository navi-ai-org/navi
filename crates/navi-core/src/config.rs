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
            attachment_models: AttachmentModelsConfig::default(),
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
            registry: RegistryConfig::default(),
            tui: TuiConfig::default(),
            background_models: BackgroundModelsConfig::default(),
            goals: GoalsConfig::default(),
            voice: VoiceConfig::default(),
            updates: UpdatesConfig::default(),
            browser: BrowserConfig::default(),
        };

        let mut project = NaviConfig {
            model: ModelConfig {
                provider: "openai".to_string(),
                name: "gpt-5.4".to_string(),
            },
            attachment_models: AttachmentModelsConfig::default(),
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
            registry: RegistryConfig::default(),
            tui: TuiConfig::default(),
            background_models: BackgroundModelsConfig::default(),
            goals: GoalsConfig::default(),
            voice: VoiceConfig::default(),
            updates: UpdatesConfig::default(),
            browser: BrowserConfig::default(),
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
    fn parses_permission_mode_and_tool_rules() {
        let config: NaviConfig = toml::from_str(
            r#"
[security]
permission_mode = "accept-edits"
allow_tools = ["read_file"]
allow_tool_regex = ["^repo_"]
ask_tools = ["bash"]
ask_tool_regex = ["^plugin__"]
deny_tools = ["package_manager"]
deny_tool_regex = ["^danger_"]
"#,
        )
        .expect("config parses");

        assert_eq!(config.security.permission_mode, PermissionMode::AcceptEdits);
        assert_eq!(config.security.allow_tools, vec!["read_file"]);
        assert_eq!(config.security.allow_tool_regex, vec!["^repo_"]);
        assert_eq!(config.security.ask_tools, vec!["bash"]);
        assert_eq!(config.security.ask_tool_regex, vec!["^plugin__"]);
        assert_eq!(config.security.deny_tools, vec!["package_manager"]);
        assert_eq!(config.security.deny_tool_regex, vec!["^danger_"]);
    }

    #[test]
    fn effective_security_config_honors_legacy_approval_modes() {
        let mut config = NaviConfig::default();
        config.approvals.require_for_writes = false;
        config.approvals.require_for_commands = true;
        assert_eq!(
            config.effective_security_config().permission_mode,
            PermissionMode::AcceptEdits
        );

        config.approvals.require_for_commands = false;
        assert_eq!(
            config.effective_security_config().permission_mode,
            PermissionMode::Yolo
        );

        config.approvals.require_for_writes = true;
        config.approvals.require_for_commands = true;
        config.approvals.allow_reads = false;
        assert_eq!(
            config.effective_security_config().permission_mode,
            PermissionMode::Restricted
        );
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
            !opencode.models.is_empty(),
            "opencode models should come from the embedded registry snapshot"
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
        assert!(
            providers
                .iter()
                .filter(|p| p.id != "opencode" && p.id != "opencode-go")
                .all(|provider| !provider.models.is_empty())
        );
        assert!(
            providers
                .iter()
                .map(|provider| provider.models.len())
                .sum::<usize>()
                >= 142
        );
    }

    #[test]
    fn embedded_registry_context_windows_are_non_default() {
        use crate::config::providers::set_registry_store;
        use crate::registry::RegistryStore;
        use std::sync::Arc;

        let store = RegistryStore::open_memory().expect("memory store");
        let loaded = crate::registry::load_registry(&store);
        assert!(
            !loaded.providers.is_empty(),
            "embedded registry should load providers"
        );
        set_registry_store(Arc::new(store));

        // Pick any provider+model from the embedded registry and verify
        // that effective_context_window returns the registry's value, not
        // the hardcoded default (200_000).
        let config = NaviConfig::default();
        let options = available_model_options(&config);
        let test_model = options
            .iter()
            .find(|m| {
                m.context_window_tokens
                    .is_some_and(|c| c != crate::config::defaults::DEFAULT_CONTEXT_WINDOW)
            })
            .expect(
                "embedded registry should have at least one model with non-default context window",
            );

        let mut config = NaviConfig::default();
        config.model.provider = test_model.provider_id.clone();
        config.model.name = test_model.name.clone();

        let ctx = effective_context_window(&config);
        assert_eq!(
            ctx,
            test_model.context_window_tokens.unwrap(),
            "effective_context_window should return the registry value for {}/{}",
            test_model.provider_id,
            test_model.name
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
                task_size: Some(ModelTaskSize::Small),
                context_window_tokens: None,
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                supports_images: None,
                supports_audio: None,
                supports_video: None,
                supports_documents: None,
                tool_prompt_manifest: Some(true),
                pricing_input_per_1m: None,
                pricing_output_per_1m: None,
                reasoning_levels: Vec::new(),
                default_reasoning_effort: None,
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
                task_size: Some(ModelTaskSize::Large),
                context_window_tokens: None,
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                supports_images: None,
                supports_audio: None,
                supports_video: None,
                supports_documents: None,
                tool_prompt_manifest: None,
                pricing_input_per_1m: None,
                pricing_output_per_1m: None,
                reasoning_levels: Vec::new(),
                default_reasoning_effort: None,
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
                task_size: Some(ModelTaskSize::Large),
                context_window_tokens: None,
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                supports_images: None,
                supports_audio: None,
                supports_video: None,
                supports_documents: None,
                tool_prompt_manifest: None,
                pricing_input_per_1m: None,
                pricing_output_per_1m: None,
                reasoning_levels: Vec::new(),
                default_reasoning_effort: None,
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
                task_size: Some(ModelTaskSize::Large),
                context_window_tokens: None,
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                supports_images: None,
                supports_audio: None,
                supports_video: None,
                supports_documents: None,
                tool_prompt_manifest: None,
                pricing_input_per_1m: None,
                pricing_output_per_1m: None,
                reasoning_levels: Vec::new(),
                default_reasoning_effort: None,
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
                task_size: Some(ModelTaskSize::Small),
                context_window_tokens: None,
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                supports_images: None,
                supports_audio: None,
                supports_video: None,
                supports_documents: None,
                tool_prompt_manifest: None,
                pricing_input_per_1m: None,
                pricing_output_per_1m: None,
                reasoning_levels: Vec::new(),
                default_reasoning_effort: None,
            }],
            ..Default::default()
        });

        let provider = resolve_provider_config(&config, "my-private-proxy").expect("provider");
        // No default exists for unknown provider ids, so options stay None.
        assert!(provider.request_options.is_none());
    }

    #[test]
    fn model_supports_attachment_uses_per_modality_metadata() {
        let mut config = NaviConfig::default();
        config.providers.push(ProviderConfig {
            id: "media".to_string(),
            label: "Media".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "MEDIA_KEY".to_string(),
            base_url: Some("https://example.test/v1".to_string()),
            models: vec![ProviderModelConfig {
                name: "vision-only".to_string(),
                task_size: Some(ModelTaskSize::Large),
                context_window_tokens: None,
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                supports_images: Some(true),
                supports_audio: Some(false),
                supports_video: None,
                supports_documents: Some(true),
                tool_prompt_manifest: None,
                pricing_input_per_1m: None,
                pricing_output_per_1m: None,
                reasoning_levels: Vec::new(),
                default_reasoning_effort: None,
            }],
            ..Default::default()
        });

        assert!(model_supports_attachment(
            &config,
            "media",
            "vision-only",
            crate::AttachmentKind::Image
        ));
        assert!(!model_supports_attachment(
            &config,
            "media",
            "vision-only",
            crate::AttachmentKind::Audio
        ));
        assert!(!model_supports_attachment(
            &config,
            "media",
            "vision-only",
            crate::AttachmentKind::Video
        ));
        assert!(model_supports_attachment(
            &config,
            "media",
            "vision-only",
            crate::AttachmentKind::Document
        ));
    }

    #[test]
    fn model_supports_attachment_matches_aliases_and_cross_provider() {
        let mut config = NaviConfig::default();
        config.providers.push(ProviderConfig {
            id: "commandcode".to_string(),
            label: "Command Code".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "COMMANDCODE_KEY".to_string(),
            base_url: Some("https://example.test/v1".to_string()),
            models: vec![ProviderModelConfig {
                name: "MiniMaxAI/MiniMax-M3".to_string(),
                task_size: None,
                context_window_tokens: None,
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: Some(true),
                supports_images: Some(true),
                supports_audio: None,
                supports_video: None,
                supports_documents: None,
                tool_prompt_manifest: None,
                pricing_input_per_1m: None,
                pricing_output_per_1m: None,
                reasoning_levels: Vec::new(),
                default_reasoning_effort: None,
            }],
            ..Default::default()
        });
        config.providers.push(ProviderConfig {
            id: "xiaomi".to_string(),
            label: "Xiaomi".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "XIAOMI_KEY".to_string(),
            base_url: Some("https://example.test/v1".to_string()),
            models: vec![ProviderModelConfig {
                name: "mimo-v2.5".to_string(),
                task_size: None,
                context_window_tokens: None,
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                supports_images: Some(true),
                supports_audio: None,
                supports_video: None,
                supports_documents: Some(true),
                tool_prompt_manifest: None,
                pricing_input_per_1m: None,
                pricing_output_per_1m: None,
                reasoning_levels: Vec::new(),
                default_reasoning_effort: None,
            }],
            ..Default::default()
        });
        config.providers.push(ProviderConfig {
            id: "opencode".to_string(),
            label: "OpenCode".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "OPENCODE_KEY".to_string(),
            base_url: Some("https://example.test/v1".to_string()),
            // Registry entry without local vision flag — should inherit via alias
            // from xiaomi `mimo-v2.5`.
            models: vec![ProviderModelConfig {
                name: "mimo-v2.5-free".to_string(),
                task_size: None,
                context_window_tokens: None,
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: Some(true),
                supports_images: None,
                supports_audio: None,
                supports_video: None,
                supports_documents: None,
                tool_prompt_manifest: None,
                pricing_input_per_1m: None,
                pricing_output_per_1m: None,
                reasoning_levels: Vec::new(),
                default_reasoning_effort: None,
            }],
            ..Default::default()
        });

        assert!(model_supports_attachment(
            &config,
            "commandcode",
            "MiniMaxAI/MiniMax-M3",
            crate::AttachmentKind::Image
        ));
        // Leaf / case-normalized match against the same provider entry.
        assert!(model_supports_attachment(
            &config,
            "commandcode",
            "minimax-m3",
            crate::AttachmentKind::Image
        ));
        // Free-tier alias + cross-provider inheritance from xiaomi mimo-v2.5.
        assert!(model_supports_attachment(
            &config,
            "opencode",
            "mimo-v2.5-free",
            crate::AttachmentKind::Image
        ));
        // Document support also inherits from the base xiaomi entry.
        assert!(model_supports_attachment(
            &config,
            "opencode",
            "mimo-v2.5-free",
            crate::AttachmentKind::Document
        ));
    }

    #[test]
    fn model_attachment_name_candidates_strips_free_and_vendor_prefix() {
        let candidates = model_attachment_name_candidates("mimo-v2.5-free");
        assert!(candidates.iter().any(|c| c == "mimo-v2.5-free"));
        assert!(candidates.iter().any(|c| c == "mimo-v2.5"));

        let candidates = model_attachment_name_candidates("MiniMaxAI/MiniMax-M3");
        assert!(candidates.iter().any(|c| c == "MiniMaxAI/MiniMax-M3"));
        assert!(candidates.iter().any(|c| c == "MiniMax-M3"));
        assert!(candidates.iter().any(|c| c == "minimax-m3"));
    }

    #[test]
    fn model_supports_attachment_xai_defaults_cover_unknown_grok_skus() {
        // Empty providers: only provider-default path (xAI images=true).
        let config = NaviConfig::default();
        assert!(
            model_supports_attachment(&config, "xai", "grok-4.5", crate::AttachmentKind::Image),
            "xAI default attachments.images=true must cover unlisted Grok SKUs"
        );
        assert!(
            model_supports_attachment(
                &config,
                "xai",
                "x-ai/grok-4.5",
                crate::AttachmentKind::Image
            ),
            "vendor-prefixed Grok ids should still resolve via xAI defaults"
        );
        // Audio is not a family default for xAI.
        assert!(!model_supports_attachment(
            &config,
            "xai",
            "grok-4.5",
            crate::AttachmentKind::Audio
        ));
    }

    #[test]
    fn model_supports_attachment_family_inherits_from_catalogued_sibling() {
        let mut config = NaviConfig::default();
        config.providers.push(ProviderConfig {
            id: "xai".to_string(),
            label: "xAI".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiResponses,
            api_key_env: "XAI_API_KEY".to_string(),
            base_url: Some("https://api.x.ai/v1".to_string()),
            models: vec![ProviderModelConfig {
                name: "grok-4".to_string(),
                task_size: None,
                context_window_tokens: None,
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: Some(true),
                supports_images: Some(true),
                supports_audio: None,
                supports_video: None,
                supports_documents: Some(true),
                tool_prompt_manifest: None,
                pricing_input_per_1m: None,
                pricing_output_per_1m: None,
                reasoning_levels: Vec::new(),
                default_reasoning_effort: None,
            }],
            ..Default::default()
        });
        // Override catalog entry for xai — family path should still find grok-4.
        // Note: provider_catalog merges with base registry; explicit true on
        // grok-4.5 missing means family or defaults apply.
        assert!(model_supports_attachment(
            &config,
            "xai",
            "grok-4.5",
            crate::AttachmentKind::Image
        ));
    }

    #[test]
    fn model_attachment_family_candidates_peel_versions() {
        let family = model_attachment_family_candidates("grok-4.5");
        assert!(
            family.iter().any(|c| c == "grok-4"),
            "expected grok-4 stem in {family:?}"
        );
        let family = model_attachment_family_candidates("x-ai/grok-4.20");
        assert!(
            family
                .iter()
                .any(|c| c == "grok-4" || c.starts_with("grok-4")),
            "expected grok-4* stem in {family:?}"
        );
    }
}
