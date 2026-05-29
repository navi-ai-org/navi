/// Canonical provider identity. Eliminates string-match routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderId {
    OpenAi,
    Anthropic,
    GoogleGemini,
    OpenRouter,
    GitHubCopilot,
    Opencode,
    OpencodeZen,
    OpencodeGo,
    Groq,
    Xai,
    Custom(String),
}

impl ProviderId {
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
