/// Canonical provider identity. Eliminates string-match routing.
///
/// Known providers have predefined constants. Any string is accepted.
/// New built-in providers only need a constant and a `behavior_for_provider` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderId(String);

impl ProviderId {
    // ── Known provider constants ─────────────────────────────────────────────
    pub const OPENAI: &'static str = "openai";
    pub const ANTHROPIC: &'static str = "anthropic";
    pub const GOOGLE_GEMINI: &'static str = "google-gemini";
    pub const OPENROUTER: &'static str = "openrouter";
    pub const GITHUB_COPILOT: &'static str = "github-copilot";
    pub const OPENCODE: &'static str = "opencode";
    pub const OPENCODE_ZEN: &'static str = "opencode-zen";
    pub const OPENCODE_GO: &'static str = "opencode-go";
    pub const COMMANDCODE: &'static str = "commandcode";
    pub const GROQ: &'static str = "groq";
    pub const XAI: &'static str = "xai";
    pub const MIMO_ANTHROPIC_CN: &'static str = "mimo-anthropic-cn";
    pub const MIMO_ANTHROPIC_SGP: &'static str = "mimo-anthropic-sgp";
    pub const MIMO_ANTHROPIC_AMS: &'static str = "mimo-anthropic-ams";
    pub const CLAUDINIO: &'static str = "claudinio";

    // ── Constructors ─────────────────────────────────────────────────────────

    /// Parses a provider id from the config string form (e.g. `"openai"`,
    /// `"google-gemini"`). Any string is accepted.
    pub fn from_config_id(id: &str) -> Self {
        Self(id.to_string())
    }

    /// Creates a `ProviderId` from a known constant. Panics in debug if the
    /// constant is not one of the predefined values (for safety).
    pub fn known(constant: &str) -> Self {
        debug_assert!(
            [
                Self::OPENAI,
                Self::ANTHROPIC,
                Self::GOOGLE_GEMINI,
                Self::OPENROUTER,
                Self::GITHUB_COPILOT,
                Self::OPENCODE,
                Self::OPENCODE_ZEN,
                Self::OPENCODE_GO,
                Self::COMMANDCODE,
                Self::GROQ,
                Self::XAI,
                Self::MIMO_ANTHROPIC_CN,
                Self::MIMO_ANTHROPIC_SGP,
                Self::MIMO_ANTHROPIC_AMS,
                Self::CLAUDINIO,
            ]
            .contains(&constant),
            "not a known provider constant: {constant}"
        );
        Self(constant.to_string())
    }

    // ── Accessors ────────────────────────────────────────────────────────────

    /// Returns the canonical string form of this provider id.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns `true` if this provider belongs to the opencode family
    /// (Opencode, OpencodeZen, or OpencodeGo).
    pub fn is_opencode_family(&self) -> bool {
        matches!(
            self.0.as_str(),
            Self::OPENCODE | Self::OPENCODE_ZEN | Self::OPENCODE_GO
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_config_id_stores_string_verbatim() {
        assert_eq!(ProviderId::from_config_id("openai").as_str(), "openai");
        assert_eq!(
            ProviderId::from_config_id("google-gemini").as_str(),
            "google-gemini"
        );
        assert_eq!(
            ProviderId::from_config_id("custom-provider").as_str(),
            "custom-provider"
        );
    }

    #[test]
    fn from_config_id_accepts_empty_string() {
        assert_eq!(ProviderId::from_config_id("").as_str(), "");
    }

    #[test]
    fn as_str_returns_inner_string() {
        let id = ProviderId::from_config_id("anthropic");
        assert_eq!(id.as_str(), "anthropic");
    }

    #[test]
    fn known_accepts_all_predefined_constants() {
        let constants = [
            ProviderId::OPENAI,
            ProviderId::ANTHROPIC,
            ProviderId::GOOGLE_GEMINI,
            ProviderId::OPENROUTER,
            ProviderId::GITHUB_COPILOT,
            ProviderId::OPENCODE,
            ProviderId::OPENCODE_ZEN,
            ProviderId::OPENCODE_GO,
            ProviderId::COMMANDCODE,
            ProviderId::GROQ,
            ProviderId::XAI,
            ProviderId::MIMO_ANTHROPIC_CN,
            ProviderId::MIMO_ANTHROPIC_SGP,
            ProviderId::MIMO_ANTHROPIC_AMS,
            ProviderId::CLAUDINIO,
        ];
        for c in constants {
            let id = ProviderId::known(c);
            assert_eq!(id.as_str(), c);
        }
    }

    #[test]
    fn is_opencode_family_returns_true_for_opencode_variants() {
        assert!(ProviderId::from_config_id("opencode").is_opencode_family());
        assert!(ProviderId::from_config_id("opencode-zen").is_opencode_family());
        assert!(ProviderId::from_config_id("opencode-go").is_opencode_family());
    }

    #[test]
    fn is_opencode_family_returns_false_for_others() {
        assert!(!ProviderId::from_config_id("openai").is_opencode_family());
        assert!(!ProviderId::from_config_id("anthropic").is_opencode_family());
        assert!(!ProviderId::from_config_id("commandcode").is_opencode_family());
        assert!(!ProviderId::from_config_id("google-gemini").is_opencode_family());
        assert!(!ProviderId::from_config_id("custom").is_opencode_family());
    }

    #[test]
    fn known_equals_from_config_id_for_same_string() {
        assert_eq!(
            ProviderId::known(ProviderId::OPENAI),
            ProviderId::from_config_id("openai")
        );
        assert_eq!(
            ProviderId::known(ProviderId::ANTHROPIC),
            ProviderId::from_config_id("anthropic")
        );
    }
}
