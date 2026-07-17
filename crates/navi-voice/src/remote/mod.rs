//! Remote (cloud) speech-to-text providers.
//!
//! These mirror LLM providers: registry-backed id, base URL, API key env, and
//! model list. File transcription is the primary surface; streaming remains
//! local-engine only for now.

mod openai_compat;

use std::path::Path;

use anyhow::{Context, Result, bail};

pub use openai_compat::OpenAiAudioTranscriptionsClient;

/// Wire protocol for a remote transcription provider (matches registry `kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteTranscriptionKind {
    /// `POST {base}/audio/transcriptions` multipart (OpenAI / Groq).
    OpenaiAudioTranscriptions,
}

impl RemoteTranscriptionKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openai-audio-transcriptions" | "openai_audio_transcriptions" => {
                Some(Self::OpenaiAudioTranscriptions)
            }
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiAudioTranscriptions => "openai-audio-transcriptions",
        }
    }
}

/// Configuration resolved from the registry + voice config + credentials.
#[derive(Debug, Clone)]
pub struct RemoteTranscriptionConfig {
    pub provider_id: String,
    pub kind: RemoteTranscriptionKind,
    pub base_url: String,
    pub transcription_path: String,
    pub api_key: String,
    pub model: String,
    pub language: Option<String>,
}

/// Offline-style result (text + optional detected language).
#[derive(Debug, Clone)]
pub struct RemoteTranscribeResult {
    pub text: String,
    pub detected_language: Option<String>,
    pub provider_id: String,
    pub model: String,
}

/// Dispatch a file transcription to the configured remote provider.
pub async fn transcribe_file_remote(
    config: &RemoteTranscriptionConfig,
    path: &Path,
) -> Result<RemoteTranscribeResult> {
    if config.api_key.trim().is_empty() {
        bail!(
            "missing API key for transcription provider '{}'",
            config.provider_id
        );
    }
    if !path.is_file() {
        bail!("audio file not found: {}", path.display());
    }

    match config.kind {
        RemoteTranscriptionKind::OpenaiAudioTranscriptions => {
            OpenAiAudioTranscriptionsClient::transcribe(config, path)
                .await
                .with_context(|| {
                    format!(
                        "openai-compatible transcription via provider '{}'",
                        config.provider_id
                    )
                })
        }
    }
}

/// Join base_url + path without double slashes.
pub(crate) fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    if path.is_empty() {
        return base.to_string();
    }
    if path.starts_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_url_trims_slashes() {
        assert_eq!(
            join_url("https://api.openai.com/v1/", "/audio/transcriptions"),
            "https://api.openai.com/v1/audio/transcriptions"
        );
        assert_eq!(
            join_url("https://api.openai.com/v1", "audio/transcriptions"),
            "https://api.openai.com/v1/audio/transcriptions"
        );
    }

    #[test]
    fn kind_parse_aliases() {
        assert_eq!(
            RemoteTranscriptionKind::parse("openai-audio-transcriptions"),
            Some(RemoteTranscriptionKind::OpenaiAudioTranscriptions)
        );
        assert_eq!(
            RemoteTranscriptionKind::parse("openai_audio_transcriptions"),
            Some(RemoteTranscriptionKind::OpenaiAudioTranscriptions)
        );
        assert_eq!(
            RemoteTranscriptionKind::parse("whisperflow"),
            None
        );
    }
}
