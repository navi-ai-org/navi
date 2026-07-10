//! Wispr Flow REST transcription (`POST /api` with base64 WAV).

use std::path::Path;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::Deserialize;
use serde_json::json;

use super::{RemoteTranscribeResult, RemoteTranscriptionConfig, join_url};
use crate::wav::{load_wav_16k_mono, write_wav_16k_mono_bytes};

#[derive(Debug, Deserialize)]
struct WisprResponse {
    text: Option<String>,
    #[serde(default)]
    detected_language: Option<String>,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

pub struct WisprFlowClient;

impl WisprFlowClient {
    pub async fn transcribe(
        config: &RemoteTranscriptionConfig,
        path: &Path,
    ) -> Result<RemoteTranscribeResult> {
        // Wispr requires base64 16 kHz WAV.
        let samples = load_wav_16k_mono(path)
            .with_context(|| format!("load/resample wav {}", path.display()))?;
        let wav_bytes = write_wav_16k_mono_bytes(&samples).context("encode 16kHz mono wav")?;
        let audio_b64 = B64.encode(&wav_bytes);

        let mut language: Vec<String> = Vec::new();
        if let Some(lang) = config.language.as_deref() {
            let lang = lang.trim();
            if !lang.is_empty() && !lang.eq_ignore_ascii_case("auto") {
                let short = lang.split(['-', '_']).next().unwrap_or(lang);
                language.push(short.to_string());
            }
        }

        let body = json!({
            "audio": audio_b64,
            "language": language,
            "context": {
                "app": { "type": "other" },
                "dictionary_context": [],
                "textbox_contents": {
                    "before_text": "",
                    "selected_text": "",
                    "after_text": ""
                }
            }
        });

        let url = join_url(&config.base_url, &config.transcription_path);
        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .bearer_auth(&config.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;

        let status = response.status();
        let body_text = response
            .text()
            .await
            .context("read wispr-flow response body")?;

        if !status.is_success() {
            bail!(
                "wispr-flow transcription failed (HTTP {status}): {}",
                truncate(&body_text, 500)
            );
        }

        let parsed: WisprResponse = serde_json::from_str(&body_text).with_context(|| {
            format!(
                "parse wispr-flow JSON: {}",
                truncate(&body_text, 300)
            )
        })?;

        if let Some(detail) = parsed.detail.or(parsed.message) {
            if parsed.text.as_ref().map(|t| t.trim().is_empty()).unwrap_or(true) {
                bail!("wispr-flow error: {detail}");
            }
        }

        let text = parsed
            .text
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("wispr-flow response missing text"))?;

        Ok(RemoteTranscribeResult {
            text: text.trim().to_string(),
            detected_language: parsed.detected_language,
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
