use crate::config::types::{
    ModelTaskSize, ProviderConfig, ProviderKind, ProviderModelConfig, ProviderRequestOptions,
    ToolCallingMode,
};

/// Returns the built-in default [`ProviderRequestOptions`] for a canonical
/// provider id, or `None` when the provider has no known defaults.
///
/// This is the single source of truth for the "out of the box" prompt
/// caching settings. The catalog layer merges these defaults into the resolved
/// [`ProviderConfig`] whenever the user has not explicitly configured the
/// options, so prompt caching stays enabled even when the local registry
/// cache is stale or when a user override replaces the provider wholesale.
pub fn default_request_options_for(provider_id: &str) -> Option<ProviderRequestOptions> {
    match provider_id {
        "openai" | "openai-responses" => Some(ProviderRequestOptions {
            prompt_cache_key: Some("openai".to_string()),
            prompt_cache_retention: Some("24h".to_string()),
            ..Default::default()
        }),
        "anthropic" => Some(ProviderRequestOptions {
            anthropic_cache_control: Some(serde_json::json!({ "type": "ephemeral" })),
            ..Default::default()
        }),
        _ => None,
    }
}

pub(super) fn model(name: &str, task_size: ModelTaskSize) -> ProviderModelConfig {
    ProviderModelConfig {
        name: name.to_string(),
        task_size,
        context_window_tokens: None,
        max_output_tokens: None,
        recommended_temperature: None,
        supports_thinking: None,
        tool_prompt_manifest: None,
    }
}

pub(super) fn model_ctx(name: &str, task_size: ModelTaskSize, ctx: u64) -> ProviderModelConfig {
    ProviderModelConfig {
        name: name.to_string(),
        task_size,
        context_window_tokens: Some(ctx),
        max_output_tokens: None,
        recommended_temperature: None,
        supports_thinking: None,
        tool_prompt_manifest: None,
    }
}

pub(super) fn determine_task_size(name: &str) -> ModelTaskSize {
    let name_lower = name.to_lowercase();
    if name_lower.contains("mini")
        || name_lower.contains("flash")
        || name_lower.contains("haiku")
        || name_lower.contains("nano")
        || name_lower.contains("instant")
        || name_lower.contains("lite")
        || name_lower.contains("scout")
        || name_lower.contains("small")
        || name_lower.contains("8b")
        || name_lower.contains("7b")
        || name_lower.contains("3b")
        || name_lower.contains("12b")
    {
        ModelTaskSize::Small
    } else {
        ModelTaskSize::Large
    }
}

pub(super) fn built_in_providers() -> Vec<ProviderConfig> {
    vec![
        // ─── Tier 1: Major cloud providers ─────────────────────────────────────────
        ProviderConfig {
            id: "openai".to_string(),
            label: "OpenAI".to_string(),
            description: "ChatGPT Plus/Pro or API key".to_string(),
            kind: ProviderKind::OpenAiResponses,
            api_key_env: "OPENAI_API_KEY".to_string(),
            base_url: Some("https://api.openai.com/v1".to_string()),
            models: vec![
                model_ctx("gpt-5.5", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-5.4", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-5.4-codex", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-5.4-mini", ModelTaskSize::Small, 512_000),
                model_ctx("gpt-5.3-codex", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-5.2", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-5.1-codex", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-5.1-codex-mini", ModelTaskSize::Small, 512_000),
                model_ctx("gpt-5.1", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-5.1-mini", ModelTaskSize::Small, 512_000),
                model_ctx("gpt-5", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-5-mini", ModelTaskSize::Small, 512_000),
                model_ctx("gpt-5-nano", ModelTaskSize::Small, 256_000),
                model_ctx("gpt-4.1", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-4.1-mini", ModelTaskSize::Small, 512_000),
                model_ctx("gpt-4.1-nano", ModelTaskSize::Small, 256_000),
                model_ctx("gpt-4o", ModelTaskSize::Large, 128_000),
                model_ctx("gpt-4o-mini", ModelTaskSize::Small, 128_000),
                model_ctx("chatgpt-4o-latest", ModelTaskSize::Large, 128_000),
                model_ctx("gpt-4.5-preview", ModelTaskSize::Large, 128_000),
                model_ctx("o3", ModelTaskSize::Large, 200_000),
                model_ctx("o3-pro", ModelTaskSize::Large, 200_000),
                model_ctx("o3-mini", ModelTaskSize::Small, 200_000),
                model_ctx("o4-mini", ModelTaskSize::Small, 200_000),
                model_ctx("o1", ModelTaskSize::Large, 200_000),
                model_ctx("o1-pro", ModelTaskSize::Large, 200_000),
                model_ctx("o1-mini", ModelTaskSize::Small, 128_000),
                model_ctx("gpt-oss-120b", ModelTaskSize::Large, 128_000),
                model_ctx("gpt-oss-20b", ModelTaskSize::Small, 128_000),
            ],
            request_options: default_request_options_for("openai"),
            ..Default::default()
        },
        ProviderConfig {
            id: "anthropic".to_string(),
            label: "Anthropic".to_string(),
            description: "Claude models via API key".to_string(),
            kind: ProviderKind::AnthropicMessages,
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
            base_url: Some("https://api.anthropic.com/v1".to_string()),
            models: vec![
                model_ctx("claude-opus-4", ModelTaskSize::Large, 200_000),
                model_ctx("claude-opus-4-1-20250805", ModelTaskSize::Large, 200_000),
                model_ctx("claude-opus-4-20250514", ModelTaskSize::Large, 200_000),
                model_ctx("claude-sonnet-4", ModelTaskSize::Large, 200_000),
                model_ctx("claude-sonnet-4-20250514", ModelTaskSize::Large, 200_000),
                model_ctx("claude-haiku-4", ModelTaskSize::Small, 200_000),
                model_ctx("claude-3.7-sonnet", ModelTaskSize::Large, 200_000),
                model_ctx("claude-3-7-sonnet-20250219", ModelTaskSize::Large, 200_000),
                model_ctx("claude-3.5-sonnet", ModelTaskSize::Large, 200_000),
                model_ctx("claude-3-5-sonnet-20241022", ModelTaskSize::Large, 200_000),
                model_ctx("claude-3-5-sonnet-20240620", ModelTaskSize::Large, 200_000),
                model_ctx("claude-3.5-haiku", ModelTaskSize::Small, 200_000),
                model_ctx("claude-3-5-haiku-20241022", ModelTaskSize::Small, 200_000),
                model_ctx("claude-3-opus-20240229", ModelTaskSize::Large, 200_000),
                model_ctx("claude-3-sonnet-20240229", ModelTaskSize::Large, 200_000),
                model_ctx("claude-3-haiku-20240307", ModelTaskSize::Small, 200_000),
            ],
            request_options: default_request_options_for("anthropic"),
            ..Default::default()
        },
        ProviderConfig {
            id: "github-copilot".to_string(),
            label: "GitHub Copilot".to_string(),
            description: "GitHub Copilot OAuth device sign-in".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "GITHUB_COPILOT_TOKEN".to_string(),
            base_url: Some("https://api.githubcopilot.com".to_string()),
            models: vec![
                model_ctx("gpt-5.1-codex", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-5.1", ModelTaskSize::Large, 200_000),
                model_ctx("gpt-5-mini", ModelTaskSize::Small, 512_000),
                model_ctx("claude-sonnet-4.5", ModelTaskSize::Large, 200_000),
                model_ctx("claude-haiku-4.5", ModelTaskSize::Small, 200_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "google-gemini".to_string(),
            label: "Google Gemini".to_string(),
            description: "Gemini API key".to_string(),
            kind: ProviderKind::GeminiGenerateContent,
            api_key_env: "GEMINI_API_KEY".to_string(),
            base_url: Some("https://generativelanguage.googleapis.com/v1beta/openai/".to_string()),
            models: vec![
                model_ctx("gemini-2.5-pro", ModelTaskSize::Large, 1_000_000),
                model_ctx(
                    "gemini-2.5-pro-preview-06-05",
                    ModelTaskSize::Large,
                    1_000_000,
                ),
                model_ctx("gemini-2.5-flash", ModelTaskSize::Small, 1_000_000),
                model_ctx(
                    "gemini-2.5-flash-preview-05-20",
                    ModelTaskSize::Small,
                    1_000_000,
                ),
                model_ctx("gemini-2.5-flash-lite", ModelTaskSize::Small, 1_000_000),
                model_ctx("gemini-2.0-flash", ModelTaskSize::Small, 1_000_000),
                model_ctx("gemini-2.0-flash-001", ModelTaskSize::Small, 1_000_000),
                model_ctx("gemini-2.0-flash-lite", ModelTaskSize::Small, 1_000_000),
                model_ctx("gemini-1.5-pro", ModelTaskSize::Large, 2_000_000),
                model_ctx("gemini-1.5-pro-002", ModelTaskSize::Large, 2_000_000),
                model_ctx("gemini-1.5-flash", ModelTaskSize::Small, 1_000_000),
                model_ctx("gemini-1.5-flash-002", ModelTaskSize::Small, 1_000_000),
                model_ctx("gemini-1.5-flash-8b", ModelTaskSize::Small, 1_000_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "xai".to_string(),
            label: "xAI".to_string(),
            description: "Grok models via xAI API".to_string(),
            kind: ProviderKind::OpenAiResponses,
            api_key_env: "XAI_API_KEY".to_string(),
            base_url: Some("https://api.x.ai/v1".to_string()),
            models: vec![
                model_ctx("grok-4.3", ModelTaskSize::Large, 256_000),
                model_ctx("grok-4", ModelTaskSize::Large, 256_000),
                model_ctx("grok-4-fast", ModelTaskSize::Small, 131_072),
                model_ctx("grok-4-fast-reasoning", ModelTaskSize::Large, 256_000),
                model_ctx("grok-4-fast-non-reasoning", ModelTaskSize::Small, 131_072),
                model_ctx("grok-3", ModelTaskSize::Large, 131_072),
                model_ctx("grok-3-fast", ModelTaskSize::Small, 131_072),
                model_ctx("grok-3-mini", ModelTaskSize::Small, 131_072),
                model_ctx("grok-3-mini-fast", ModelTaskSize::Small, 131_072),
                model_ctx("grok-2-1212", ModelTaskSize::Large, 131_072),
                model_ctx("grok-2-vision-1212", ModelTaskSize::Large, 131_072),
                model_ctx("grok-build-0.1", ModelTaskSize::Large, 256_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "mistral".to_string(),
            label: "Mistral".to_string(),
            description: "Mistral AI API".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "MISTRAL_API_KEY".to_string(),
            base_url: Some("https://api.mistral.ai/v1".to_string()),
            models: vec![
                model_ctx("mistral-large-latest", ModelTaskSize::Large, 128_000),
                model_ctx("mistral-large-2411", ModelTaskSize::Large, 128_000),
                model_ctx("mistral-large-2407", ModelTaskSize::Large, 128_000),
                model_ctx("mistral-medium-latest", ModelTaskSize::Large, 64_000),
                model_ctx("mistral-medium-2508", ModelTaskSize::Large, 64_000),
                model_ctx("mistral-small-latest", ModelTaskSize::Small, 32_000),
                model_ctx("mistral-small-2506", ModelTaskSize::Small, 32_000),
                model_ctx("mistral-small-2503", ModelTaskSize::Small, 32_000),
                model_ctx("codestral-latest", ModelTaskSize::Large, 128_000),
                model_ctx("codestral-2508", ModelTaskSize::Large, 128_000),
                model_ctx("codestral-2501", ModelTaskSize::Large, 128_000),
                model_ctx("codestral-2405", ModelTaskSize::Large, 128_000),
                model_ctx("devstral-medium-latest", ModelTaskSize::Large, 256_000),
                model_ctx("devstral-medium-2507", ModelTaskSize::Large, 256_000),
                model_ctx("devstral-small-latest", ModelTaskSize::Small, 32_000),
                model_ctx("devstral-small-2507", ModelTaskSize::Small, 32_000),
                model_ctx("devstral-small-2505", ModelTaskSize::Small, 32_000),
                model_ctx("magistral-medium-latest", ModelTaskSize::Large, 128_000),
                model_ctx("magistral-small-latest", ModelTaskSize::Small, 32_000),
                model_ctx("pixtral-large-latest", ModelTaskSize::Large, 128_000),
                model_ctx("pixtral-12b-2409", ModelTaskSize::Small, 128_000),
                model_ctx("open-mistral-nemo", ModelTaskSize::Small, 128_000),
                model_ctx("open-mixtral-8x22b", ModelTaskSize::Large, 64_000),
                model_ctx("open-mixtral-8x7b", ModelTaskSize::Small, 32_000),
                model_ctx("open-mistral-7b", ModelTaskSize::Small, 32_000),
                model_ctx("ministral-8b-latest", ModelTaskSize::Small, 32_000),
                model_ctx("ministral-3b-latest", ModelTaskSize::Small, 32_000),
            ],
            ..Default::default()
        },
        // ─── Tier 2: High-quality specialized ─────────────────────────────────────
        ProviderConfig {
            id: "deepseek".to_string(),
            label: "DeepSeek".to_string(),
            description: "DeepSeek API".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            base_url: Some("https://api.deepseek.com".to_string()),
            models: vec![
                model_ctx("deepseek-v4-pro", ModelTaskSize::Large, 1_000_000),
                model_ctx("deepseek-v4-flash", ModelTaskSize::Small, 1_000_000),
                model_ctx("deepseek-chat", ModelTaskSize::Large, 1_000_000),
                model_ctx("deepseek-reasoner", ModelTaskSize::Large, 1_000_000),
                model_ctx("deepseek-coder", ModelTaskSize::Large, 1_000_000),
                model_ctx("deepseek-coder-v2", ModelTaskSize::Large, 1_000_000),
                model_ctx("deepseek-coder-v2-lite", ModelTaskSize::Small, 1_000_000),
                model_ctx("deepseek-v3", ModelTaskSize::Large, 1_000_000),
                model_ctx("deepseek-v3.1", ModelTaskSize::Large, 1_000_000),
                model_ctx("deepseek-v3.2", ModelTaskSize::Large, 1_000_000),
                model_ctx("deepseek-r1", ModelTaskSize::Large, 1_000_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "moonshot".to_string(),
            label: "Moonshot AI".to_string(),
            description: "Kimi models via API".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "MOONSHOT_API_KEY".to_string(),
            base_url: Some("https://api.moonshot.cn/v1".to_string()),
            models: vec![
                model_ctx("kimi-k2.6", ModelTaskSize::Large, 128_000),
                model_ctx("kimi-k2.5", ModelTaskSize::Large, 128_000),
                model_ctx("kimi-k2-thinking", ModelTaskSize::Large, 128_000),
                model_ctx("kimi-k2", ModelTaskSize::Large, 128_000),
                model_ctx("kimi-k2-0711-preview", ModelTaskSize::Large, 128_000),
                model_ctx("kimi-latest", ModelTaskSize::Large, 128_000),
                model_ctx("kimi-thinking-preview", ModelTaskSize::Large, 128_000),
                model_ctx("moonshot-v1-128k", ModelTaskSize::Large, 128_000),
                model_ctx("moonshot-v1-32k", ModelTaskSize::Small, 32_000),
                model_ctx("moonshot-v1-8k", ModelTaskSize::Small, 8_000),
                model_ctx("moonshot-v1-auto", ModelTaskSize::Large, 128_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "zai".to_string(),
            label: "Z.AI".to_string(),
            description: "GLM models by Zhipu AI".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "ZAI_API_KEY".to_string(),
            base_url: Some("https://api.z.ai/api/paas/v4/".to_string()),
            models: vec![
                model_ctx("glm-5.1", ModelTaskSize::Large, 128_000),
                model_ctx("glm-5", ModelTaskSize::Large, 128_000),
                model_ctx("glm-5-turbo", ModelTaskSize::Small, 128_000),
                model_ctx("glm-4.7", ModelTaskSize::Large, 128_000),
                model_ctx("glm-4.6", ModelTaskSize::Small, 128_000),
                model_ctx("glm-4.5", ModelTaskSize::Large, 128_000),
                model_ctx("glm-4.5-air", ModelTaskSize::Small, 128_000),
                model_ctx("glm-4.5-x", ModelTaskSize::Large, 128_000),
                model_ctx("glm-4.5-flash", ModelTaskSize::Small, 128_000),
                model_ctx("glm-4-plus", ModelTaskSize::Large, 128_000),
                model_ctx("glm-4-flash", ModelTaskSize::Small, 128_000),
                model_ctx("glm-4-long", ModelTaskSize::Large, 1_000_000),
                model_ctx("glm-4-air", ModelTaskSize::Small, 128_000),
                model_ctx("glm-4-airx", ModelTaskSize::Small, 128_000),
                model_ctx("glm-4-0520", ModelTaskSize::Large, 128_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "zai-coding".to_string(),
            label: "Z.AI Coding Plan".to_string(),
            description: "Dedicated coding endpoint".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "ZAI_API_KEY".to_string(),
            base_url: Some("https://api.z.ai/api/coding/paas/v4/".to_string()),
            models: vec![
                model_ctx("glm-5.1", ModelTaskSize::Large, 128_000),
                model_ctx("glm-5", ModelTaskSize::Large, 128_000),
                model("glm-5-turbo", ModelTaskSize::Small),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "minimax".to_string(),
            label: "MiniMax".to_string(),
            description: "MiniMax platform".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "MINIMAX_API_KEY".to_string(),
            base_url: Some("https://api.minimax.io/v1".to_string()),
            models: vec![
                model_ctx("MiniMax-M2.7", ModelTaskSize::Large, 1_000_000),
                model_ctx("MiniMax-M2.5", ModelTaskSize::Large, 1_000_000),
                model_ctx("MiniMax-M2.1", ModelTaskSize::Small, 128_000),
                model_ctx("MiniMax-Text-01", ModelTaskSize::Large, 256_000),
                model_ctx("MiniMax-Text-01-456B", ModelTaskSize::Large, 256_000),
                model_ctx("abab6.5-chat", ModelTaskSize::Large, 128_000),
                model_ctx("abab6.5g-chat", ModelTaskSize::Large, 128_000),
                model_ctx("abab6.5s-chat", ModelTaskSize::Small, 64_000),
                model_ctx("abab6.5t-chat", ModelTaskSize::Small, 32_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "groq".to_string(),
            label: "Groq".to_string(),
            description: "Ultra-fast inference".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "GROQ_API_KEY".to_string(),
            base_url: Some("https://api.groq.com/openai/v1".to_string()),
            models: vec![
                model_ctx("llama-3.3-70b-versatile", ModelTaskSize::Large, 128_000),
                model_ctx("llama-3.1-8b-instant", ModelTaskSize::Small, 32_000),
                model_ctx("openai/gpt-oss-120b", ModelTaskSize::Large, 128_000),
                model_ctx("openai/gpt-oss-20b", ModelTaskSize::Small, 128_000),
                model_ctx("qwen/qwen3-32b", ModelTaskSize::Small, 128_000),
                model_ctx(
                    "deepseek-r1-distill-llama-70b",
                    ModelTaskSize::Large,
                    1_000_000,
                ),
                model_ctx("moonshotai/kimi-k2-instruct", ModelTaskSize::Large, 128_000),
                model_ctx(
                    "meta-llama/llama-4-maverick-17b-128e-instruct",
                    ModelTaskSize::Large,
                    524_288,
                ),
                model_ctx(
                    "meta-llama/llama-4-scout-17b-16e-instruct",
                    ModelTaskSize::Small,
                    1_048_576,
                ),
                model_ctx("meta-llama/llama-guard-4-12b", ModelTaskSize::Small, 32_000),
                model_ctx("mistral-saba-24b", ModelTaskSize::Small, 32_000),
                model_ctx("gemma2-9b-it", ModelTaskSize::Small, 8_192),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "openrouter".to_string(),
            label: "OpenRouter".to_string(),
            description: "Unified API for 300+ models".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "OPENROUTER_API_KEY".to_string(),
            base_url: Some("https://openrouter.ai/api/v1".to_string()),
            models: vec![
                model_ctx("anthropic/claude-opus-4", ModelTaskSize::Large, 200_000),
                model_ctx("anthropic/claude-sonnet-4", ModelTaskSize::Large, 200_000),
                model_ctx("openai/gpt-5.5", ModelTaskSize::Large, 1_000_000),
                model_ctx("openai/gpt-5.4", ModelTaskSize::Large, 1_000_000),
                model_ctx("openai/gpt-4.1", ModelTaskSize::Large, 1_000_000),
                model_ctx("google/gemini-2.5-pro", ModelTaskSize::Large, 1_000_000),
                model_ctx("google/gemini-2.5-flash", ModelTaskSize::Small, 1_000_000),
                model_ctx("deepseek/deepseek-v4-pro", ModelTaskSize::Large, 1_000_000),
                model_ctx("deepseek/deepseek-chat", ModelTaskSize::Large, 1_000_000),
                model_ctx("x-ai/grok-4", ModelTaskSize::Large, 256_000),
                model_ctx("x-ai/grok-3", ModelTaskSize::Large, 131_072),
                model_ctx("meta-llama/llama-3.3-70b", ModelTaskSize::Large, 128_000),
                model_ctx("meta-llama/llama-4-maverick", ModelTaskSize::Large, 524_288),
                model_ctx("meta-llama/llama-4-scout", ModelTaskSize::Small, 1_048_576),
                model_ctx("mistralai/mistral-large", ModelTaskSize::Large, 128_000),
                model_ctx("mistralai/codestral", ModelTaskSize::Large, 128_000),
                model_ctx("qwen/qwen3-coder", ModelTaskSize::Large, 128_000),
                model_ctx("qwen/qwen3-235b-a22b", ModelTaskSize::Large, 128_000),
                model_ctx("qwen/qwen3-32b", ModelTaskSize::Small, 128_000),
                model_ctx("z-ai/glm-4.5", ModelTaskSize::Large, 128_000),
                model_ctx("moonshotai/kimi-k2", ModelTaskSize::Large, 128_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "stepfun".to_string(),
            label: "StepFun".to_string(),
            description: "StepFun AI models".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "STEP_API_KEY".to_string(),
            base_url: Some("https://api.stepfun.ai/v1".to_string()),
            models: vec![
                model_ctx("step-3.5-flash", ModelTaskSize::Large, 256_000),
                model_ctx("step-3", ModelTaskSize::Large, 128_000),
                model_ctx("step-2-16k", ModelTaskSize::Large, 16_000),
                model_ctx("step-2", ModelTaskSize::Large, 256_000),
                model_ctx("step-1-256k", ModelTaskSize::Large, 256_000),
                model_ctx("step-1-128k", ModelTaskSize::Large, 128_000),
                model_ctx("step-1-32k", ModelTaskSize::Small, 32_000),
                model_ctx("step-1-8k", ModelTaskSize::Small, 8_000),
                model_ctx("step-1v", ModelTaskSize::Small, 32_000),
                model_ctx("step-1", ModelTaskSize::Small, 32_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "xiaomi".to_string(),
            label: "MiMo".to_string(),
            description: "MiMo models via Tokens Plan".to_string(),
            kind: ProviderKind::AnthropicMessages,
            api_key_env: "XIAOMI_API_KEY".to_string(),
            base_url: Some("https://token-plan-sgp.xiaomimimo.com/anthropic".to_string()),
            models: vec![
                model_ctx("mimo-v2.5-pro", ModelTaskSize::Large, 1_048_576),
                model_ctx("mimo-v2.5", ModelTaskSize::Small, 1_048_576),
                model_ctx("mimo-v2-pro", ModelTaskSize::Large, 1_048_576),
                model_ctx("mimo-v2-omni", ModelTaskSize::Small, 262_144),
                model_ctx("mimo-v2-flash", ModelTaskSize::Small, 262_144),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "mimo-anthropic-cn".to_string(),
            label: "MiMo (China)".to_string(),
            description: "".to_string(),
            kind: ProviderKind::AnthropicMessages,
            api_key_env: "MIMO_API_KEY".to_string(),
            base_url: Some("https://token-plan-cn.xiaomimimo.com/anthropic".to_string()),
            models: vec![
                model_ctx("mimo-v2.5-pro", ModelTaskSize::Large, 1_048_576),
                model_ctx("mimo-v2.5", ModelTaskSize::Small, 1_048_576),
                model_ctx("mimo-v2-pro", ModelTaskSize::Large, 1_048_576),
                model_ctx("mimo-v2-omni", ModelTaskSize::Small, 262_144),
                model_ctx("mimo-v2-flash", ModelTaskSize::Small, 262_144),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "mimo-anthropic-sgp".to_string(),
            label: "MiMo Singapore (Tokens Plan)".to_string(),
            description: "".to_string(),
            kind: ProviderKind::AnthropicMessages,
            api_key_env: "MIMO_API_KEY".to_string(),
            base_url: Some("https://token-plan-sgp.xiaomimimo.com/anthropic".to_string()),
            models: vec![
                model_ctx("mimo-v2.5-pro", ModelTaskSize::Large, 1_048_576),
                model_ctx("mimo-v2.5", ModelTaskSize::Small, 1_048_576),
                model_ctx("mimo-v2-pro", ModelTaskSize::Large, 1_048_576),
                model_ctx("mimo-v2-omni", ModelTaskSize::Small, 262_144),
                model_ctx("mimo-v2-flash", ModelTaskSize::Small, 262_144),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "mimo-anthropic-ams".to_string(),
            label: "MiMo (Europe)".to_string(),
            description: "".to_string(),
            kind: ProviderKind::AnthropicMessages,
            api_key_env: "MIMO_API_KEY".to_string(),
            base_url: Some("https://token-plan-ams.xiaomimimo.com/anthropic".to_string()),
            models: vec![
                model_ctx("mimo-v2.5-pro", ModelTaskSize::Large, 1_048_576),
                model_ctx("mimo-v2.5", ModelTaskSize::Small, 1_048_576),
                model_ctx("mimo-v2-pro", ModelTaskSize::Large, 1_048_576),
                model_ctx("mimo-v2-omni", ModelTaskSize::Small, 262_144),
                model_ctx("mimo-v2-flash", ModelTaskSize::Small, 262_144),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "nvidia".to_string(),
            label: "Nvidia".to_string(),
            description: "NIM inference microservices".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "NVIDIA_API_KEY".to_string(),
            base_url: Some("https://integrate.api.nvidia.com/v1".to_string()),
            models: vec![
                model_ctx("meta/llama-3.3-70b-instruct", ModelTaskSize::Large, 128_000),
                model_ctx("meta/llama-3.1-8b-instruct", ModelTaskSize::Small, 128_000),
                model_ctx(
                    "mistralai/mistral-7b-instruct",
                    ModelTaskSize::Small,
                    32_000,
                ),
                model_ctx(
                    "nvidia/llama-3.1-nemotron-70b-instruct",
                    ModelTaskSize::Large,
                    128_000,
                ),
                model_ctx(
                    "qwen/qwen2.5-coder-32b-instruct",
                    ModelTaskSize::Large,
                    128_000,
                ),
                model_ctx(
                    "mistralai/mixtral-8x7b-instruct",
                    ModelTaskSize::Small,
                    32_000,
                ),
                model_ctx("mistralai/mistral-large", ModelTaskSize::Large, 128_000),
                model_ctx("deepseek-ai/deepseek-r1", ModelTaskSize::Large, 1_000_000),
                model_ctx("microsoft/phi-4", ModelTaskSize::Small, 16_000),
                model_ctx("minimaxai/minimax-m3", ModelTaskSize::Large, 1_000_000),
            ],
            ..Default::default()
        },
        // ─── Tier 3: Local / self-hosted ──────────────────────────────────────────
        ProviderConfig {
            id: "ollama".to_string(),
            label: "Ollama".to_string(),
            description: "Local + cloud inference".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "OLLAMA_API_KEY".to_string(),
            base_url: Some("http://localhost:11434/v1".to_string()),
            models: vec![
                model_ctx("llama3.1", ModelTaskSize::Large, 128_000),
                model_ctx("llama3.2", ModelTaskSize::Small, 128_000),
                model_ctx("llama3.3", ModelTaskSize::Large, 128_000),
                model_ctx("deepseek-r1", ModelTaskSize::Large, 1_000_000),
                model_ctx("qwen3", ModelTaskSize::Large, 128_000),
                model_ctx("qwen2.5-coder", ModelTaskSize::Large, 128_000),
                model_ctx("qwen2.5-coder:32b", ModelTaskSize::Large, 128_000),
                model_ctx("qwen2.5-coder:14b", ModelTaskSize::Small, 32_000),
                model_ctx("qwen2.5-coder:7b", ModelTaskSize::Small, 32_000),
                model_ctx("codellama", ModelTaskSize::Large, 16_000),
                model_ctx("starcoder2", ModelTaskSize::Small, 16_000),
                model_ctx("granite-code", ModelTaskSize::Small, 128_000),
                model_ctx("gemma3", ModelTaskSize::Small, 128_000),
                model_ctx("gemma3:27b", ModelTaskSize::Large, 128_000),
                model_ctx("gemma3:12b", ModelTaskSize::Small, 128_000),
                model_ctx("mistral", ModelTaskSize::Small, 32_000),
                model_ctx("devstral", ModelTaskSize::Small, 32_000),
                model_ctx("phi4", ModelTaskSize::Small, 16_000),
                model_ctx("phi4-mini", ModelTaskSize::Small, 16_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "lmstudio".to_string(),
            label: "LMStudio".to_string(),
            description: "Local inference server".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "LMSTUDIO_API_KEY".to_string(),
            base_url: Some("http://localhost:1234/v1".to_string()),
            models: vec![
                model_ctx("local-model", ModelTaskSize::Large, 128_000),
                model_ctx("qwen2.5-coder-14b", ModelTaskSize::Small, 32_000),
                model_ctx("qwen2.5-coder-7b", ModelTaskSize::Small, 32_000),
                model_ctx("qwen2.5-coder-32b", ModelTaskSize::Large, 128_000),
                model_ctx(
                    "deepseek-r1-distill-qwen-32b",
                    ModelTaskSize::Large,
                    128_000,
                ),
                model_ctx("deepseek-r1-distill-llama-8b", ModelTaskSize::Small, 32_000),
                model_ctx("mistral-small-instruct", ModelTaskSize::Small, 32_000),
                model_ctx("gemma-3-27b-it", ModelTaskSize::Large, 128_000),
                model_ctx("llama-3.2-3b-instruct", ModelTaskSize::Small, 128_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "llamacpp".to_string(),
            label: "Llama.cpp".to_string(),
            description: "Self-hosted GGUF inference".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "LLAMACPP_API_KEY".to_string(),
            base_url: Some("http://localhost:8080/v1".to_string()),
            models: vec![
                model_ctx("local-model", ModelTaskSize::Large, 128_000),
                model_ctx("qwen2.5-coder", ModelTaskSize::Large, 128_000),
                model_ctx("deepseek-coder", ModelTaskSize::Large, 1_000_000),
                model_ctx("starcoder2", ModelTaskSize::Small, 16_000),
                model_ctx("granite-code", ModelTaskSize::Small, 128_000),
                model_ctx("llama3", ModelTaskSize::Large, 128_000),
                model_ctx("mistral", ModelTaskSize::Small, 32_000),
                model_ctx("tinyllama", ModelTaskSize::Small, 2_048),
            ],
            ..Default::default()
        },
        // ─── Tier 4: Aggregators / value ──────────────────────────────────────────
        ProviderConfig {
            id: "charm-hyper".to_string(),
            label: "Charm Hyper".to_string(),
            description: "Hyper provider".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "CHARM_HYPER_API_KEY".to_string(),
            base_url: None,
            models: vec![
                model_ctx("Kimi K2.6", ModelTaskSize::Large, 128_000),
                model_ctx("Kimi K2.5", ModelTaskSize::Large, 128_000),
                model_ctx("DeepSeek V4 Pro", ModelTaskSize::Large, 1_000_000),
                model_ctx("DeepSeek V4 Flash", ModelTaskSize::Small, 1_000_000),
                model_ctx("Gemma 4 26B A4B", ModelTaskSize::Small, 128_000),
                model_ctx("GLM-5.1", ModelTaskSize::Large, 128_000),
                model_ctx("GLM-5", ModelTaskSize::Large, 128_000),
                model_ctx("Qwen 3 32B", ModelTaskSize::Small, 128_000),
                model_ctx("MiniMax M2.1", ModelTaskSize::Small, 128_000),
                model_ctx("gpt-oss-120b", ModelTaskSize::Large, 128_000),
                model_ctx("gpt-oss-20b", ModelTaskSize::Small, 128_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "opencode".to_string(),
            label: "OpenCode Zen".to_string(),
            description: "Recommended".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "OPENCODE_API_KEY".to_string(),
            base_url: None,
            models: vec![
                model_ctx("big-pickle", ModelTaskSize::Large, 200_000),
                model_ctx("deepseek-v4-flash-free", ModelTaskSize::Small, 1_000_000),
                model_ctx("nemotron-3-super-free", ModelTaskSize::Small, 1_000_000),
                model_ctx("qwen3.6-plus", ModelTaskSize::Large, 1_000_000),
                model_ctx("qwen3.5-plus", ModelTaskSize::Large, 1_000_000),
                model_ctx("kimi-k2.6", ModelTaskSize::Large, 262_144),
                model_ctx("kimi-k2.5", ModelTaskSize::Large, 262_144),
                model_ctx("glm-5.1", ModelTaskSize::Large, 200_000),
                model_ctx("glm-5", ModelTaskSize::Large, 200_000),
                model_ctx("minimax-m2.7", ModelTaskSize::Small, 204_800),
                model_ctx("minimax-m2.5", ModelTaskSize::Small, 204_800),
                model_ctx("grok-build-0.1", ModelTaskSize::Small, 256_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "opencode-go".to_string(),
            label: "OpenCode Go".to_string(),
            description: "Low cost subscription".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "OPENCODE_GO_API_KEY".to_string(),
            base_url: None,
            models: vec![
                model_ctx("deepseek-v4-flash", ModelTaskSize::Small, 1_000_000),
                model_ctx("deepseek-v4-pro", ModelTaskSize::Large, 1_000_000),
                model_ctx("qwen3.6-plus", ModelTaskSize::Large, 1_000_000),
                model_ctx("glm-5", ModelTaskSize::Large, 200_000),
                model_ctx("kimi-k2.5", ModelTaskSize::Large, 262_144),
                model_ctx("minimax-m2.5", ModelTaskSize::Small, 204_800),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "commandcode".to_string(),
            label: "Command Code".to_string(),
            description: "Command Code CLI API (alpha/generate)".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "CMD_API_KEY".to_string(),
            base_url: Some("https://api.commandcode.ai".to_string()),
            tool_calling_mode: Some(ToolCallingMode::Disabled),
            models: vec![
                model_ctx("claude-sonnet-4-6", ModelTaskSize::Large, 1_000_000),
                model_ctx("claude-fable-5", ModelTaskSize::Large, 1_000_000),
                model_ctx("claude-opus-4-8", ModelTaskSize::Large, 1_000_000),
                model_ctx("claude-opus-4-7", ModelTaskSize::Large, 1_000_000),
                model_ctx("claude-haiku-4-5-20251001", ModelTaskSize::Small, 200_000),
                model_ctx("gpt-5.5", ModelTaskSize::Large, 200_000),
                model_ctx("gpt-5.4", ModelTaskSize::Large, 400_000),
                model_ctx("gpt-5.3-codex", ModelTaskSize::Large, 400_000),
                model_ctx("gpt-5.4-mini", ModelTaskSize::Small, 400_000),
                model_ctx("MiniMaxAI/MiniMax-M3", ModelTaskSize::Large, 1_000_000),
                model_ctx("deepseek/deepseek-v4-pro", ModelTaskSize::Large, 1_000_000),
                model_ctx(
                    "deepseek/deepseek-v4-flash",
                    ModelTaskSize::Small,
                    1_000_000,
                ),
                model_ctx("moonshotai/Kimi-K2.7-Code", ModelTaskSize::Large, 256_000),
                model_ctx("moonshotai/Kimi-K2.6", ModelTaskSize::Large, 256_000),
                model_ctx("moonshotai/Kimi-K2.5", ModelTaskSize::Large, 256_000),
                model_ctx("zai-org/GLM-5.1", ModelTaskSize::Large, 200_000),
                model_ctx("zai-org/GLM-5", ModelTaskSize::Large, 200_000),
                model_ctx("MiniMaxAI/MiniMax-M2.7", ModelTaskSize::Small, 200_000),
                model_ctx("MiniMaxAI/MiniMax-M2.5", ModelTaskSize::Small, 200_000),
                model_ctx("xiaomi/mimo-v2.5-pro", ModelTaskSize::Large, 1_000_000),
                model_ctx("xiaomi/mimo-v2.5", ModelTaskSize::Small, 1_000_000),
                model_ctx("Qwen/Qwen3.6-Max-Preview", ModelTaskSize::Large, 200_000),
                model_ctx("Qwen/Qwen3.6-Plus", ModelTaskSize::Large, 200_000),
                model_ctx("Qwen/Qwen3.7-Max", ModelTaskSize::Large, 1_000_000),
                model_ctx("Qwen/Qwen3.7-Plus", ModelTaskSize::Large, 1_000_000),
                model_ctx("stepfun/Step-3.7-Flash", ModelTaskSize::Small, 256_000),
                model_ctx("stepfun/Step-3.5-Flash", ModelTaskSize::Small, 1_000_000),
                model_ctx("google/gemini-3.5-flash", ModelTaskSize::Small, 1_000_000),
                model_ctx(
                    "google/gemini-3.1-flash-lite",
                    ModelTaskSize::Small,
                    1_000_000,
                ),
                model_ctx(
                    "nvidia/nemotron-3-ultra-550b-a55b",
                    ModelTaskSize::Large,
                    1_000_000,
                ),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "gitlawb".to_string(),
            label: "Gitlawb".to_string(),
            description: "Free Opengateway (MiMo, Gemini)".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "GITLAWB_API_KEY".to_string(),
            base_url: Some("https://opengateway.gitlawb.com/v1".to_string()),
            models: vec![
                model_ctx("mimo-v2.5-pro", ModelTaskSize::Large, 200_000),
                model_ctx("mimo-v2.5", ModelTaskSize::Large, 200_000),
                model_ctx(
                    "google/gemini-3.1-flash-lite-preview",
                    ModelTaskSize::Small,
                    1_000_000,
                ),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "custom".to_string(),
            label: "Custom".to_string(),
            description: "User-configured endpoint".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "CUSTOM_API_KEY".to_string(),
            base_url: None,
            models: vec![model("custom-model", ModelTaskSize::Large)],
            ..Default::default()
        },
    ]
}
