use crate::ProviderId;

/// Returns `true` if the model can run without an API key (opencode family free models).
pub fn model_can_run_publicly(provider_id: &str, model: &str) -> bool {
    ProviderId::from_config_id(provider_id).is_opencode_family() && is_free_model_name(model)
}

/// Returns the canonical model name to send in the API request, mapping free
/// model aliases to their opencode-zen id when applicable.
pub fn provider_request_model_name(provider_id: &str, model: &str) -> String {
    if ProviderId::from_config_id(provider_id).is_opencode_family() && is_free_model_name(model) {
        opencode_zen_model_id(model).unwrap_or_else(|| model.to_string())
    } else {
        model.to_string()
    }
}

/// Normalizes a free model name to its canonical opencode-zen model id.
/// Returns `None` if the model is not a recognized free model.
///
/// Only called for models where [`is_free_model_name`] returns `true`, so the
/// match arms below only need to cover free models from the registry.
pub(super) fn opencode_zen_model_id(model: &str) -> Option<String> {
    let normalized = model
        .trim()
        .trim_start_matches("opencode/")
        .to_ascii_lowercase()
        .replace([' ', '_'], "-");
    let collapsed = normalized
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    match collapsed.as_str() {
        // Free models from registry-snapshot/providers/opencode.json (pricing 0.0).
        "deepseek-v4-flash-free" => Some("deepseek-v4-flash-free".to_string()),
        "nemotron-3-ultra-free" => Some("nemotron-3-ultra-free".to_string()),
        "big-pickle" => Some("big-pickle".to_string()),
        "mimo-v2.5-free" => Some("mimo-v2.5-free".to_string()),
        "hy3-free" => Some("hy3-free".to_string()),
        "north-mini-code-free" => Some("north-mini-code-free".to_string()),
        _ => None,
    }
}

/// Returns `true` if the model name indicates a free model (ends with `-free`,
/// `_free`, ` free`, or contains `free` as a standalone word after normalizing
/// separators and case).
pub fn is_free_model_name(model: &str) -> bool {
    let normalized = model.to_ascii_lowercase().replace('_', "-");
    normalized.ends_with("-free") || normalized.contains(" free")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_free_model_name ───────────────────────────────────────────────

    #[test]
    fn free_model_suffix_detected() {
        assert!(is_free_model_name("deepseek-v4-flash-free"));
        assert!(is_free_model_name("hy3-free"));
        assert!(is_free_model_name("north-mini-code-free"));
    }

    #[test]
    fn free_model_substring_detected() {
        assert!(is_free_model_name("deepseek v4 flash free"));
        assert!(is_free_model_name("some model free tier"));
    }

    #[test]
    fn free_model_detection_is_case_insensitive() {
        assert!(is_free_model_name("DeepSeek-V4-Flash-FREE"));
        assert!(is_free_model_name("BIG-PICKLE FREE"));
    }

    #[test]
    fn free_model_underscore_suffix_detected() {
        assert!(is_free_model_name("deepseek_v4_flash_free"));
        assert!(is_free_model_name("hy3_Free"));
    }

    #[test]
    fn paid_models_not_detected_as_free() {
        assert!(!is_free_model_name("deepseek-v4-flash"));
        assert!(!is_free_model_name("gpt-5.1"));
        assert!(!is_free_model_name("claude-sonnet-4"));
        assert!(!is_free_model_name("qwen3.6-plus"));
        assert!(!is_free_model_name("big-pickle"));
        assert!(!is_free_model_name(""));
    }

    #[test]
    fn free_model_not_false_positive_on_substr() {
        // "free" in the middle should not match the suffix check,
        // but " free" as a substring should.
        assert!(!is_free_model_name("freedom-model"));
        assert!(!is_free_model_name("free-tier-gpt"));
    }

    // ── opencode_zen_model_id ────────────────────────────────────────────

    #[test]
    fn zen_id_all_registered_free_models() {
        // Every free model ref in registry-snapshot/providers/opencode.json
        // must resolve to its canonical id.
        assert_eq!(
            opencode_zen_model_id("deepseek-v4-flash-free").as_deref(),
            Some("deepseek-v4-flash-free")
        );
        assert_eq!(
            opencode_zen_model_id("nemotron-3-ultra-free").as_deref(),
            Some("nemotron-3-ultra-free")
        );
        assert_eq!(
            opencode_zen_model_id("big-pickle").as_deref(),
            Some("big-pickle")
        );
        assert_eq!(
            opencode_zen_model_id("mimo-v2.5-free").as_deref(),
            Some("mimo-v2.5-free")
        );
        assert_eq!(
            opencode_zen_model_id("hy3-free").as_deref(),
            Some("hy3-free")
        );
        assert_eq!(
            opencode_zen_model_id("north-mini-code-free").as_deref(),
            Some("north-mini-code-free")
        );
    }

    #[test]
    fn zen_id_strips_opencode_prefix() {
        assert_eq!(
            opencode_zen_model_id("opencode/deepseek-v4-flash-free").as_deref(),
            Some("deepseek-v4-flash-free")
        );
        assert_eq!(
            opencode_zen_model_id("opencode/big-pickle").as_deref(),
            Some("big-pickle")
        );
    }

    #[test]
    fn zen_id_normalizes_separators() {
        // Underscores → hyphens
        assert_eq!(
            opencode_zen_model_id("deepseek_v4_flash_free").as_deref(),
            Some("deepseek-v4-flash-free")
        );
        // Spaces → hyphens
        assert_eq!(
            opencode_zen_model_id("deepseek v4 flash free").as_deref(),
            Some("deepseek-v4-flash-free")
        );
        // Mixed case
        assert_eq!(
            opencode_zen_model_id("DeepSeek_V4_Flash_Free").as_deref(),
            Some("deepseek-v4-flash-free")
        );
    }

    #[test]
    fn zen_id_collapses_empty_segments() {
        assert_eq!(
            opencode_zen_model_id("deepseek--v4-flash-free").as_deref(),
            Some("deepseek-v4-flash-free")
        );
        assert_eq!(
            opencode_zen_model_id("big--pickle").as_deref(),
            Some("big-pickle")
        );
    }

    #[test]
    fn zen_id_trims_whitespace() {
        assert_eq!(
            opencode_zen_model_id("  deepseek-v4-flash-free  ").as_deref(),
            Some("deepseek-v4-flash-free")
        );
    }

    #[test]
    fn zen_id_returns_none_for_unrecognized() {
        assert!(opencode_zen_model_id("some-random-model").is_none());
        assert!(opencode_zen_model_id("gpt-5.1").is_none());
        assert!(opencode_zen_model_id("").is_none());
    }

    #[test]
    fn zen_id_returns_none_for_paid_models() {
        // Paid models should not resolve — they're not free model aliases.
        assert!(opencode_zen_model_id("qwen3.6-plus").is_none());
        assert!(opencode_zen_model_id("glm-5.1").is_none());
        assert!(opencode_zen_model_id("kimi-k2.6").is_none());
        assert!(opencode_zen_model_id("grok-build-0.1").is_none());
    }

    // ── model_can_run_publicly ───────────────────────────────────────────

    #[test]
    fn can_run_publicly_opencode_free_model() {
        assert!(model_can_run_publicly("opencode", "deepseek-v4-flash-free"));
        assert!(model_can_run_publicly("opencode-zen", "hy3-free"));
        assert!(model_can_run_publicly(
            "opencode-go",
            "north-mini-code-free"
        ));
    }

    #[test]
    fn cannot_run_publicly_opencode_paid_model() {
        assert!(!model_can_run_publicly("opencode", "deepseek-v4-flash"));
        assert!(!model_can_run_publicly("opencode", "gpt-5.1"));
        assert!(!model_can_run_publicly("opencode-zen", "claude-sonnet-4"));
    }

    #[test]
    fn cannot_run_publicly_non_opencode_free_model() {
        // Free-looking model on a non-opencode provider should not be public.
        assert!(!model_can_run_publicly("openai", "gpt-5-nano-free"));
        assert!(!model_can_run_publicly("anthropic", "claude-free"));
        assert!(!model_can_run_publicly(
            "openrouter",
            "deepseek-v4-flash-free"
        ));
    }

    // ── provider_request_model_name ──────────────────────────────────────

    #[test]
    fn request_model_name_maps_free_alias() {
        assert_eq!(
            provider_request_model_name("opencode", "opencode/deepseek-v4-flash-free"),
            "deepseek-v4-flash-free"
        );
        assert_eq!(
            provider_request_model_name("opencode-zen", "DeepSeek_V4_Flash_Free"),
            "deepseek-v4-flash-free"
        );
    }

    #[test]
    fn request_model_name_passes_through_paid() {
        assert_eq!(
            provider_request_model_name("opencode", "deepseek-v4-flash"),
            "deepseek-v4-flash"
        );
        assert_eq!(
            provider_request_model_name("opencode", "gpt-5.1"),
            "gpt-5.1"
        );
    }

    #[test]
    fn request_model_name_passes_through_non_opencode() {
        assert_eq!(provider_request_model_name("openai", "gpt-5.1"), "gpt-5.1");
        assert_eq!(
            provider_request_model_name("anthropic", "claude-sonnet-4"),
            "claude-sonnet-4"
        );
    }

    #[test]
    fn request_model_name_unrecognized_free_falls_back() {
        // A free-looking model that's not in the registry should fall back to
        // the trimmed name rather than None.
        assert_eq!(
            provider_request_model_name("opencode", "some-model-free"),
            "some-model-free"
        );
    }
}
