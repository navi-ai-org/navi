use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub provider: String,
    pub model: String,
    pub approval_mode: String,
}

impl Settings {
    pub fn defaults() -> Self {
        Self {
            provider: "openai".to_string(),
            model: "gpt-5.5".to_string(),
            approval_mode: "ask".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SettingsPatch {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub approval_mode: Option<String>,
}

pub fn resolve_config(
    defaults: Settings,
    global: Option<SettingsPatch>,
    project: Option<SettingsPatch>,
    env: &BTreeMap<String, String>,
) -> Settings {
    let mut settings = defaults;
    apply_env(&mut settings, env);
    if let Some(global) = global {
        apply_patch(&mut settings, global);
    }
    if let Some(project) = project {
        apply_patch(&mut settings, project);
    }
    settings
}

fn apply_patch(settings: &mut Settings, patch: SettingsPatch) {
    if let Some(provider) = patch.provider {
        settings.provider = provider;
    }
    if let Some(model) = patch.model {
        settings.model = model;
    }
    if let Some(approval_mode) = patch.approval_mode {
        settings.approval_mode = approval_mode;
    }
}

fn apply_env(settings: &mut Settings, env: &BTreeMap<String, String>) {
    if let Some(provider) = env.get("NAVI_PROVIDER") {
        settings.provider = provider.clone();
    }
    if let Some(model) = env.get("NAVI_MODEL") {
        settings.model = model.clone();
    }
    if let Some(approval_mode) = env.get("NAVI_APPROVAL_MODE") {
        settings.approval_mode = approval_mode.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_overrides_project_and_global_config() {
        let global = SettingsPatch {
            provider: Some("anthropic".to_string()),
            model: Some("claude-sonnet".to_string()),
            approval_mode: None,
        };
        let project = SettingsPatch {
            provider: Some("opencode".to_string()),
            model: Some("deepseek-v4-flash".to_string()),
            approval_mode: Some("auto".to_string()),
        };
        let env = BTreeMap::from([
            ("NAVI_MODEL".to_string(), "deepseek-v4-flash-free".to_string()),
            ("NAVI_APPROVAL_MODE".to_string(), "ask".to_string()),
        ]);

        let resolved = resolve_config(Settings::defaults(), Some(global), Some(project), &env);

        assert_eq!(resolved.provider, "opencode");
        assert_eq!(resolved.model, "deepseek-v4-flash-free");
        assert_eq!(resolved.approval_mode, "ask");
    }

    #[test]
    fn project_overrides_global_when_env_is_absent() {
        let global = SettingsPatch {
            provider: Some("anthropic".to_string()),
            model: Some("claude-sonnet".to_string()),
            approval_mode: Some("ask".to_string()),
        };
        let project = SettingsPatch {
            provider: Some("opencode".to_string()),
            model: None,
            approval_mode: Some("auto".to_string()),
        };

        let resolved = resolve_config(
            Settings::defaults(),
            Some(global),
            Some(project),
            &BTreeMap::new(),
        );

        assert_eq!(resolved.provider, "opencode");
        assert_eq!(resolved.model, "claude-sonnet");
        assert_eq!(resolved.approval_mode, "auto");
    }
}
