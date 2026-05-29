use crate::ProviderId;

pub fn model_can_run_publicly(provider_id: &str, model: &str) -> bool {
    ProviderId::from_config_id(provider_id).is_opencode_family() && is_free_model_name(model)
}

pub fn provider_request_model_name(provider_id: &str, model: &str) -> String {
    if ProviderId::from_config_id(provider_id).is_opencode_family() && is_free_model_name(model) {
        opencode_zen_model_id(model).unwrap_or_else(|| model.to_string())
    } else {
        model.to_string()
    }
}

pub fn opencode_zen_model_id(model: &str) -> Option<String> {
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
        "deepseek-v4-flash-free" => Some("deepseek-v4-flash-free".to_string()),
        "nemotron-3-super-free" => Some("nemotron-3-super-free".to_string()),
        "big-pickle" => Some("big-pickle".to_string()),
        "qwen3.6-plus" | "qwen-3.6-plus" => Some("qwen3.6-plus".to_string()),
        "qwen3.5-plus" | "qwen-3.5-plus" => Some("qwen3.5-plus".to_string()),
        "minimax-m2.7" | "mini-max-m2.7" => Some("minimax-m2.7".to_string()),
        "minimax-m2.5" | "mini-max-m2.5" => Some("minimax-m2.5".to_string()),
        "glm-5.1" => Some("glm-5.1".to_string()),
        "glm-5" => Some("glm-5".to_string()),
        "kimi-k2.6" => Some("kimi-k2.6".to_string()),
        "kimi-k2.5" => Some("kimi-k2.5".to_string()),
        "grok-build-0.1" => Some("grok-build-0.1".to_string()),
        _ => None,
    }
}

pub fn is_free_model_name(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.ends_with("-free") || model.contains(" free")
}
