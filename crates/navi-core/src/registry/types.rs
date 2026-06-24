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

/// A single model entry in the remote registry JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryModel {
    pub name: String,
    pub task_size: String,
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
}

/// Per-provider entry inside the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestProviderEntry {
    pub file: String,
    pub sha256: String,
    pub model_count: usize,
}
