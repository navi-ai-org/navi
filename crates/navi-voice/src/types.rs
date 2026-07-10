use serde::{Deserialize, Serialize};

/// Default Hugging Face repo for the Nemotron streaming ONNX package.
pub const DEFAULT_VOICE_HF_REPO: &str = "navi-org/navi-voice-nemotron-3.5-asr-streaming-0.6b-onnx";

/// Selectable ASR engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AsrEngineId {
    /// Nemotron 3.5 ASR streaming 0.6B (ONNX INT4) — live dictation default.
    #[default]
    NemotronStreaming,
    /// Distil-Whisper large-v2 (candle) — chunked utterance (not wired yet).
    DistilWhisper,
}

impl AsrEngineId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NemotronStreaming => "nemotron_streaming",
            Self::DistilWhisper => "distil_whisper",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "nemotron_streaming" | "nemotron" | "streaming" => Some(Self::NemotronStreaming),
            "distil_whisper" | "distil-whisper" | "whisper" | "distil" => Some(Self::DistilWhisper),
            _ => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::NemotronStreaming => "Nemotron 3.5 ASR Streaming 0.6B (ONNX)",
            Self::DistilWhisper => "Distil-Whisper large-v2 (candle)",
        }
    }
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

/// Options for path resolution / download (mirrors navi-core `VoiceConfig` fields).
#[derive(Debug, Clone)]
pub struct VoiceInstallOptions {
    pub model_dir: String,
    pub hf_repo_nemotron: String,
}

impl Default for VoiceInstallOptions {
    fn default() -> Self {
        Self {
            model_dir: String::new(),
            hf_repo_nemotron: DEFAULT_VOICE_HF_REPO.to_string(),
        }
    }
}

/// Serializable install index (`navi-manifest.json` in model package).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceManifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub engine: String,
    pub runtime: String,
    pub sample_rate: u32,
    pub chunk_samples: u32,
    pub streaming: bool,
    pub languages: String,
    pub paths: VoiceManifestPaths,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceManifestPaths {
    pub encoder: String,
    pub decoder: String,
    pub joint: String,
    pub vad: String,
    pub tokenizer: String,
    pub vocab: String,
    pub genai_config: String,
    pub audio_processor_config: String,
    pub checksums: String,
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
    ModelMissing { engine: String, hint: String },
}

/// Serializable status snapshot for SDK / N-API / desktop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceStatus {
    pub enabled: bool,
    /// `local` or remote transcription provider id (`openai`, `groq`, `wispr-flow`, …).
    #[serde(default = "default_voice_provider")]
    pub provider: String,
    /// Remote model id when provider is not local.
    #[serde(default)]
    pub model: String,
    pub engine: String,
    pub language: String,
    pub capture: String,
    pub recorder: String,
    pub model_dir: String,
    pub installed: bool,
    pub streaming_active: bool,
    pub sample_rate: u32,
    pub chunk_samples: u32,
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
