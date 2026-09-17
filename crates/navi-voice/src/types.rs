use serde::{Deserialize, Serialize};

/// Transcription result.
///
/// `token_ids` is a leftover from the removed local ONNX ASR path; remote
/// providers return text only, so it stays empty there.
#[derive(Debug, Clone)]
pub struct TranscribeResult {
    pub text: String,
    pub token_ids: Vec<usize>,
}

/// How the mic chord behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VoiceCaptureMode {
    #[default]
    Toggle,
    Hold,
}

impl VoiceCaptureMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::Hold => "hold",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "toggle" => Some(Self::Toggle),
            "hold" => Some(Self::Hold),
            _ => None,
        }
    }
}

/// Events emitted by a live dictation pipeline (TUI / SDK / N-API).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceEvent {
    Started { engine: String },
    Partial { text: String },
    Final { text: String },
    Error { message: String },
    Stopped,
}

/// Serializable status snapshot for SDK / N-API / desktop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceStatus {
    pub enabled: bool,
    /// `local` (legacy) or remote transcription provider id (`openai`, `groq`, …).
    #[serde(default = "default_voice_provider")]
    pub provider: String,
    /// Remote model id when provider is not local.
    #[serde(default)]
    pub model: String,
    pub language: String,
    pub capture: String,
    pub recorder: String,
    /// Whether the selected transcription backend is usable (remote provider
    /// present in the registry).
    pub installed: bool,
    /// Target capture rate for speech recognition (16 kHz).
    pub sample_rate: u32,
    pub recorders: Vec<VoiceRecorderInfo>,
}

fn default_voice_provider() -> String {
    "local".to_string()
}

/// A recorder binary discovered on PATH.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceRecorderInfo {
    pub id: String,
    pub path: String,
}
