//! Types shared between the registry store, fetcher, and catalog integration.

use serde::{Deserialize, Serialize};

use crate::config::types::ProviderRequestOptions;

/// A capability tag on a model (e.g. "tool_calling", "fast", "cheap").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapability {
    /// Composite key: `"provider_id:model_name"`.
    pub model_id: String,
    pub provider_id: String,
    pub capability: String,
    pub value: String,
}

/// Pricing metadata for a model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    /// Composite key: `"provider_id:model_name"`.
    pub model_id: String,
    pub provider_id: String,
    /// Price per 1M input tokens.
    pub input_price: Option<f64>,
    /// Price per 1M output tokens.
    pub output_price: Option<f64>,
    pub currency: String,
}

/// Associates a model with a routing profile and a quality score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfileEntry {
    /// Composite key: `"provider_id:model_name"`.
    pub model_id: String,
    pub provider_id: String,
    /// Profile identifier (e.g. "cheap_general", "repo_search").
    pub profile_id: String,
    /// Higher = better fit for this profile.
    pub score: f64,
}

/// A named routing profile that defines constraints for model selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub description: String,
    /// Minimum context window required (tokens).
    pub min_context: Option<u64>,
    /// Maximum input price per 1M tokens (cost ceiling).
    pub max_input_price: Option<f64>,
    /// Whether the model must support tool calling.
    pub requires_tools: bool,
}

/// A model ranked by a profile query, ready for selection.
#[derive(Debug, Clone)]
pub struct RankedModel {
    pub model_id: String,
    pub provider_id: String,
    pub model_name: String,
    pub score: f64,
    pub input_price: Option<f64>,
    pub output_price: Option<f64>,
    pub context_window_tokens: Option<u64>,
}

/// Attachment modalities a provider or model accepts directly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegistryAttachments {
    /// Whether image attachments can be sent directly to the model.
    #[serde(default, alias = "image", skip_serializing_if = "Option::is_none")]
    pub images: Option<bool>,
    /// Whether audio attachments can be sent directly to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<bool>,
    /// Whether video attachments can be sent directly to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<bool>,
    /// Whether document attachments can be sent directly to the model.
    #[serde(default, alias = "document", skip_serializing_if = "Option::is_none")]
    pub documents: Option<bool>,
}

impl RegistryAttachments {
    pub fn is_empty(&self) -> bool {
        self.images.is_none()
            && self.audio.is_none()
            && self.video.is_none()
            && self.documents.is_none()
    }
}

/// Provider-level defaults inherited by every model unless overridden.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegistryProviderDefaults {
    #[serde(default, skip_serializing_if = "RegistryAttachments::is_empty")]
    pub attachments: RegistryAttachments,
}

impl RegistryProviderDefaults {
    pub fn is_empty(&self) -> bool {
        self.attachments.is_empty()
    }
}

/// Pricing for a registry model (USD per 1M tokens by default).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RegistryModelPricing {
    /// Price per 1M input tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_per_1m: Option<f64>,
    /// Price per 1M output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_per_1m: Option<f64>,
    /// Currency code (defaults to USD when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

impl RegistryModelPricing {
    pub fn is_empty(&self) -> bool {
        self.input_per_1m.is_none() && self.output_per_1m.is_none()
    }
}

/// A single model entry in the remote registry JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryModel {
    pub name: String,
    /// Deprecated: task size is no longer part of the registry. Kept as optional
    /// for backward compatibility with older JSON snapshots — ignored at runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_size: Option<String>,
    pub context_window_tokens: Option<u64>,
    /// Maximum tokens the model can generate in a single response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    /// Recommended temperature for the model (0.0–2.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_temperature: Option<f64>,
    /// Whether the model supports extended thinking / reasoning mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_thinking: Option<bool>,
    /// Supported reasoning effort levels from the registry
    /// (e.g. `["none","low","medium","high","xhigh"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_levels: Vec<String>,
    /// Default reasoning effort for this model when the user has not picked one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<String>,
    /// Legacy coarse flag for models that support file/image attachments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_attachments: Option<bool>,
    /// Whether the model can consume image attachments directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_images: Option<bool>,
    /// Whether the model can consume audio attachments directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_audio: Option<bool>,
    /// Whether the model can consume video attachments directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_video: Option<bool>,
    /// Whether the model can consume document attachments directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_documents: Option<bool>,
    /// Preferred public registry shape for direct attachment support.
    #[serde(default, skip_serializing_if = "RegistryAttachments::is_empty")]
    pub attachments: RegistryAttachments,
    /// Free-form capability tags such as `vision`, `audio`, `video`, `documents`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    /// Optional list pricing used for session cost estimates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<RegistryModelPricing>,
}

/// A full provider entry as stored in `registry/providers/<id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryProvider {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    pub kind: String,
    pub api_key_env: String,
    pub base_url: Option<String>,
    #[serde(default)]
    pub tool_calling_mode: Option<String>,
    /// When `true`, this provider is an aggregator whose model list is fetched
    /// dynamically from the provider's `/models` API endpoint at sync time
    /// instead of being hardcoded in the registry JSON.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub aggregator: bool,
    /// Defaults applied to all models, overridden by per-model fields.
    #[serde(default, skip_serializing_if = "RegistryProviderDefaults::is_empty")]
    pub defaults: RegistryProviderDefaults,
    #[serde(default, skip_serializing_if = "ProviderRequestOptions::is_empty")]
    pub request_options: ProviderRequestOptions,
    #[serde(default)]
    pub models: Vec<RegistryModel>,
}

/// Top-level manifest file at `registry/manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryManifest {
    pub version: u32,
    pub updated_at: String,
    pub providers: std::collections::HashMap<String, ManifestProviderEntry>,
    /// Remote speech-to-text / dictation providers (Whisper, Wispr Flow, …).
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub transcription_providers: std::collections::HashMap<String, ManifestProviderEntry>,
}

/// Per-provider entry inside the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestProviderEntry {
    pub file: String,
    pub sha256: String,
    pub model_count: usize,
}

/// Protocol kind for a remote transcription provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TranscriptionProviderKind {
    /// OpenAI-compatible `POST /audio/transcriptions` (multipart).
    OpenaiAudioTranscriptions,
    /// Wispr Flow `POST /api` (JSON + base64 audio).
    WisprFlow,
}

impl TranscriptionProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiAudioTranscriptions => "openai-audio-transcriptions",
            Self::WisprFlow => "wispr-flow",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openai-audio-transcriptions" | "openai_audio_transcriptions" => {
                Some(Self::OpenaiAudioTranscriptions)
            }
            "wispr-flow" | "wispr_flow" | "whisperflow" | "whisper-flow" => Some(Self::WisprFlow),
            _ => None,
        }
    }
}

/// Pricing for a transcription model (USD per audio minute by default).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TranscriptionModelPricing {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_minute: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

/// A single speech-to-text model entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryTranscriptionModel {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate_hz: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<TranscriptionModelPricing>,
}

/// A remote transcription / dictation provider in the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryTranscriptionProvider {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    /// Wire kind string (`openai-audio-transcriptions` | `wispr-flow`).
    pub kind: String,
    pub api_key_env: String,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcription_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default)]
    pub supports_streaming: bool,
    #[serde(default)]
    pub models: Vec<RegistryTranscriptionModel>,
}

impl RegistryTranscriptionProvider {
    pub fn kind_enum(&self) -> Option<TranscriptionProviderKind> {
        TranscriptionProviderKind::parse(&self.kind)
    }

    pub fn resolved_path(&self) -> &str {
        if let Some(p) = self.transcription_path.as_deref() {
            if !p.is_empty() {
                return p;
            }
        }
        match self.kind_enum() {
            Some(TranscriptionProviderKind::OpenaiAudioTranscriptions) => "/audio/transcriptions",
            Some(TranscriptionProviderKind::WisprFlow) => "/api",
            None => "/audio/transcriptions",
        }
    }

    pub fn resolved_default_model(&self) -> Option<&str> {
        self.default_model
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| self.models.first().map(|m| m.name.as_str()))
    }
}
