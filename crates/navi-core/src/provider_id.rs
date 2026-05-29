/// Canonical provider identity. Eliminates string-match routing.
/// Canonical provider identity. Eliminates string-match routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderId {
    /// OpenAI (Responses API).
    OpenAi,
    /// Anthropic (Messages API).
    Anthropic,
    /// Google Gemini.
    GoogleGemini,
    /// OpenRouter.
    OpenRouter,
    /// GitHub Copilot.
    GitHubCopilot,
    /// Opencode.
    Opencode,
    /// Opencode Zen.
    OpencodeZen,
    /// Opencode Go.
    OpencodeGo,
    /// Groq.
    Groq,
    /// xAI (Grok).
    Xai,
    /// A custom provider not in the built-in set.
    Custom(String),
}

impl ProviderId {
    /// Parses a provider id from the config string form (e.g. `"openai"`,
    /// `"google-gemini"`). Unknown ids become [`ProviderId::Custom`].
    pub fn from_config_id(id: &str) -> Self {
        match id {
            "openai" => Self::OpenAi,
            "anthropic" => Self::Anthropic,
            "google-gemini" => Self::GoogleGemini,
            "openrouter" => Self::OpenRouter,
            "github-copilot" => Self::GitHubCopilot,
            "opencode" => Self::Opencode,
            "opencode-zen" => Self::OpencodeZen,
            "opencode-go" => Self::OpencodeGo,
            "groq" => Self::Groq,
            "xai" => Self::Xai,
            other => Self::Custom(other.to_string()),
        }
    }

    /// Returns the canonical string form of this provider id.
    pub fn as_str(&self) -> &str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::GoogleGemini => "google-gemini",
            Self::OpenRouter => "openrouter",
            Self::GitHubCopilot => "github-copilot",
            Self::Opencode => "opencode",
            Self::OpencodeZen => "opencode-zen",
            Self::OpencodeGo => "opencode-go",
            Self::Groq => "groq",
            Self::Xai => "xai",
            Self::Custom(s) => s,
        }
    }

    /// Returns `true` if this provider belongs to the opencode family
    /// (Opencode, OpencodeZen, or OpencodeGo).
    pub fn is_opencode_family(&self) -> bool {
        matches!(self, Self::Opencode | Self::OpencodeZen | Self::OpencodeGo)
    }
}
