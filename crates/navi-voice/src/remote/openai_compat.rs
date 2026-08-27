//! OpenAI-compatible `POST /audio/transcriptions` (OpenAI Whisper, Groq, …).

use std::path::Path;

use anyhow::{Context, Result, bail};
use reqwest::multipart::{Form, Part};
use serde::Deserialize;

use super::{RemoteTranscribeResult, RemoteTranscriptionConfig, join_url};

#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    text: Option<String>,
    /// Some providers return language.
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    error: Option<ApiErrorBody>,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    message: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

pub struct OpenAiAudioTranscriptionsClient;

impl OpenAiAudioTranscriptionsClient {
    pub async fn transcribe(
        config: &RemoteTranscriptionConfig,
        path: &Path,
    ) -> Result<RemoteTranscribeResult> {
        let bytes =
            std::fs::read(path).with_context(|| format!("read audio file {}", path.display()))?;
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("audio.wav")
            .to_string();

        let mut form = Form::new()
            .text("model", config.model.clone())
            .text("response_format", "json".to_string());

        if let Some(lang) = config.language.as_deref() {
            let lang = lang.trim();
            if !lang.is_empty() && !lang.eq_ignore_ascii_case("auto") && lang != "und" {
                // OpenAI accepts ISO-639-1; strip region (en-US → en).
                let short = lang.split(['-', '_']).next().unwrap_or(lang);
                form = form.text("language", short.to_string());
            }
        }

        let part = Part::bytes(bytes)
            .file_name(filename)
            .mime_str("application/octet-stream")
            .context("set audio part mime type")?;
        form = form.part("file", part);

        let url = join_url(&config.base_url, &config.transcription_path);
        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .bearer_auth(&config.api_key)
            .multipart(form)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("read transcription response body")?;

        if !status.is_success() {
            bail!(
                "transcription request failed (HTTP {status}): {}",
                truncate(&body, 500)
            );
        }

        let parsed: TranscriptionResponse = serde_json::from_str(&body)
            .with_context(|| format!("parse transcription JSON: {}", truncate(&body, 300)))?;

        if let Some(err) = parsed.error {
            bail!(
                "transcription API error{}: {}",
                err.code.map(|c| format!(" ({c})")).unwrap_or_default(),
                err.message.unwrap_or_else(|| body.clone())
            );
        }

        let text = parsed
            .text
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("transcription response missing text field"))?;

        Ok(RemoteTranscribeResult {
            text: text.trim().to_string(),
            detected_language: parsed.language,
            provider_id: config.provider_id.clone(),
            model: config.model.clone(),
        })
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}…")
    }
}
