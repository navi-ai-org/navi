use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use base64::Engine;
use navi_core::config::types::ModelConfig;
use navi_core::{
    AttachmentKind, ContentPart, LoadedConfig, ModelMessage, ModelRequest, ThinkingConfig, Tool,
    ToolDefinition, ToolInvocation, ToolKind, ToolResult, resolve_model_thinking_level,
};
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

#[derive(Clone)]
pub(crate) struct AttachmentAnalysisTool {
    loaded_config: LoadedConfig,
    project_dir: PathBuf,
}

impl AttachmentAnalysisTool {
    pub(crate) fn new(loaded_config: LoadedConfig, project_dir: PathBuf) -> Self {
        Self {
            loaded_config,
            project_dir,
        }
    }

    fn model_for_kind(&self, kind: AttachmentKind) -> Option<ModelConfig> {
        let config = &self.loaded_config.config.attachment_models;
        match kind {
            AttachmentKind::Image => config.image.clone(),
            AttachmentKind::Audio => config.audio.clone(),
            AttachmentKind::Video => config.video.clone(),
            AttachmentKind::Document => config.document.clone(),
        }
    }

    /// Resolve the thinking level for the attachment model, clamped to its
    /// registry-declared reasoning support. Uses the user's configured thinking
    /// level as the starting preference so the attachment request shape matches
    /// the main turn request as closely as possible.
    fn resolve_thinking(&self, model: &ModelConfig) -> ThinkingConfig {
        let base = ThinkingConfig::from_config_str(&self.loaded_config.config.tui.thinking_level);
        let provider_config =
            navi_core::resolve_provider_config(&self.loaded_config.config, &model.provider);
        if let Some(provider) = provider_config
            && let Some(registry_model) = provider
                .models
                .iter()
                .find(|m| m.name == model.name || m.name.eq_ignore_ascii_case(&model.name))
        {
            return resolve_model_thinking_level(
                base,
                registry_model.supports_thinking,
                &registry_model.reasoning_levels,
                registry_model.default_reasoning_effort.as_deref(),
            );
        }
        base
    }
}

#[derive(Debug, Deserialize)]
struct AttachmentAnalysisInput {
    kind: AttachmentKind,
    prompt: String,
    /// Raw base64 attachment data. Optional when `attachment_id` is provided.
    #[serde(default)]
    data: Option<String>,
    /// MIME type. Optional when `attachment_id` is provided (inferred from extension).
    #[serde(default)]
    media_type: Option<String>,
    /// Stored attachment id (e.g. `{sha256}.png`) to load from the attachment store.
    /// Use this when the chat model does not have the raw base64 data (text-only models).
    #[serde(default)]
    attachment_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[async_trait]
impl Tool for AttachmentAnalysisTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "analyze_attachment",
            "Analyze an image, audio file, video, or document using the configured specialized attachment model. Use this when the chat model cannot inspect an attachment directly. Provide either `data` (raw base64) or `attachment_id` (from the placeholder text when the chat model is text-only), plus a focused prompt describing what to extract.",
            ToolKind::Read,
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["kind", "prompt"],
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["image", "audio", "video", "document"],
                        "description": "Attachment modality."
                    },
                    "media_type": {
                        "type": "string",
                        "description": "MIME type, for example image/png, audio/mpeg, video/mp4, or application/pdf. Optional when attachment_id is provided (inferred from the file extension)."
                    },
                    "data": {
                        "type": "string",
                        "description": "Raw base64 attachment data with no data URL prefix. Provide this OR attachment_id."
                    },
                    "attachment_id": {
                        "type": "string",
                        "description": "Stored attachment id (e.g. `{sha256}.png`) from the placeholder text. Use this when you do not have the raw base64 data (text-only chat models). The tool loads the bytes from NAVI's attachment store."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Focused analysis prompt for the specialized model."
                    },
                    "name": {
                        "type": "string",
                        "description": "Optional filename or label."
                    }
                }
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let input: AttachmentAnalysisInput = serde_json::from_value(invocation.input)
            .context("invalid attachment analysis input")?;

        // Resolve attachment data: either from raw base64 or from the attachment store.
        let (media_type, data) = match (&input.data, &input.attachment_id) {
            (Some(data), _) if !data.is_empty() => {
                let media_type = input
                    .media_type
                    .clone()
                    .unwrap_or_else(|| default_media_type_for_kind(input.kind));
                (media_type, data.clone())
            }
            (_, Some(id)) if !id.is_empty() => {
                let bytes =
                    navi_core::attachment_store::load_bytes(&self.loaded_config.data_dir, id)
                        .map_err(|err| anyhow!("failed to load attachment {id}: {err:#}"))?;
                let media_type = input
                    .media_type
                    .clone()
                    .unwrap_or_else(|| media_type_from_attachment_id(id, input.kind));
                let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                (media_type, data)
            }
            _ => {
                return Ok(ToolResult {
                    invocation_id: invocation.id,
                    ok: false,
                    output: json!({
                        "error": "either `data` (base64) or `attachment_id` must be provided",
                    }),
                });
            }
        };

        let Some(model) = self.model_for_kind(input.kind) else {
            return Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: json!({
                    "error": format!(
                        "no default {} attachment model configured",
                        input.kind.as_str()
                    ),
                    "config": format!("attachment_models.{}", input.kind.as_str()),
                }),
            });
        };

        let mut loaded_config = self.loaded_config.clone();
        loaded_config.config.model = model.clone();
        let provider =
            crate::tooling::build_provider_for_project_config(&loaded_config, &self.project_dir)
                .map_err(|err| anyhow!("failed to build attachment model provider: {err:#}"))?;

        let attachment = match input.kind {
            AttachmentKind::Image => ContentPart::Image {
                media_type: media_type.clone(),
                data,
            },
            AttachmentKind::Audio => ContentPart::Audio {
                media_type: media_type.clone(),
                data,
                name: input.name.clone(),
            },
            AttachmentKind::Video => ContentPart::Video {
                media_type: media_type.clone(),
                data,
                name: input.name.clone(),
            },
            AttachmentKind::Document => ContentPart::Document {
                media_type: media_type.clone(),
                data,
                name: input.name.clone(),
            },
        };

        let thinking = self.resolve_thinking(&model);

        let request = ModelRequest {
            model: model.name.clone(),
            instructions: None,
            messages: vec![
                ModelMessage::system("Analyze the attachment and return concise text only."),
                ModelMessage::user_multimodal(
                    input.prompt,
                    vec![
                        ContentPart::Text {
                            text: "Analyze this attachment.".to_string(),
                        },
                        attachment,
                    ],
                ),
            ],
            thinking,
            tools: Vec::new(),
            session_id: None,
        };

        let response = provider.complete(request).await?;
        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: true,
            output: json!({
                "kind": input.kind.as_str(),
                "provider": model.provider,
                "model": model.name,
                "analysis": response.text,
            }),
        })
    }
}

/// Default MIME type for an attachment kind when none is provided.
fn default_media_type_for_kind(kind: AttachmentKind) -> String {
    match kind {
        AttachmentKind::Image => "image/png".to_string(),
        AttachmentKind::Audio => "audio/mpeg".to_string(),
        AttachmentKind::Video => "video/mp4".to_string(),
        AttachmentKind::Document => "application/pdf".to_string(),
    }
}

/// Infer a MIME type from the attachment_id file extension.
fn media_type_from_attachment_id(id: &str, kind: AttachmentKind) -> String {
    let ext = id.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase());
    match (kind, ext.as_deref()) {
        (AttachmentKind::Image, Some("png")) => "image/png".to_string(),
        (AttachmentKind::Image, Some("jpg") | Some("jpeg")) => "image/jpeg".to_string(),
        (AttachmentKind::Image, Some("gif")) => "image/gif".to_string(),
        (AttachmentKind::Image, Some("webp")) => "image/webp".to_string(),
        (AttachmentKind::Image, Some("bmp")) => "image/bmp".to_string(),
        (AttachmentKind::Image, Some("svg")) => "image/svg+xml".to_string(),
        (AttachmentKind::Image, _) => "image/png".to_string(),
        (AttachmentKind::Audio, _) => "audio/mpeg".to_string(),
        (AttachmentKind::Video, _) => "video/mp4".to_string(),
        (AttachmentKind::Document, Some("pdf")) => "application/pdf".to_string(),
        (AttachmentKind::Document, _) => "application/pdf".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use navi_core::NaviConfig;

    #[tokio::test]
    async fn invoke_without_configured_model_returns_structured_error() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let tool = AttachmentAnalysisTool::new(
            LoadedConfig {
                config: NaviConfig::default(),
                global_config_path: None,
                project_config_path: None,
                data_dir: tempdir.path().to_path_buf(),
            },
            tempdir.path().to_path_buf(),
        );

        let result = tool
            .invoke(ToolInvocation {
                id: "call-1".to_string(),
                tool_name: "analyze_attachment".to_string(),
                input: json!({
                    "kind": "audio",
                    "media_type": "audio/mpeg",
                    "data": "abc123",
                    "prompt": "transcribe",
                }),
            })
            .await
            .expect("tool result");

        assert!(!result.ok);
        assert_eq!(result.output["config"], "attachment_models.audio");
    }

    #[tokio::test]
    async fn invoke_without_data_or_attachment_id_returns_error() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let tool = AttachmentAnalysisTool::new(
            LoadedConfig {
                config: NaviConfig::default(),
                global_config_path: None,
                project_config_path: None,
                data_dir: tempdir.path().to_path_buf(),
            },
            tempdir.path().to_path_buf(),
        );

        let result = tool
            .invoke(ToolInvocation {
                id: "call-2".to_string(),
                tool_name: "analyze_attachment".to_string(),
                input: json!({
                    "kind": "image",
                    "prompt": "describe",
                }),
            })
            .await
            .expect("tool result");

        assert!(!result.ok);
        assert!(
            result.output["error"]
                .as_str()
                .is_some_and(|s| s.contains("attachment_id")),
            "error should mention attachment_id: {:?}",
            result.output
        );
    }

    #[tokio::test]
    async fn invoke_with_attachment_id_loads_from_store() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let data_dir = tempdir.path().to_path_buf();

        // Store a fake image in the attachment store.
        let image_bytes = b"fake-png-bytes-for-test";
        let attachment_id = navi_core::attachment_store::store_bytes(&data_dir, image_bytes, "png")
            .expect("store bytes");

        let tool = AttachmentAnalysisTool::new(
            LoadedConfig {
                config: NaviConfig::default(),
                global_config_path: None,
                project_config_path: None,
                data_dir: data_dir,
            },
            tempdir.path().to_path_buf(),
        );

        // No attachment model configured → should get the "no model" error,
        // but only AFTER successfully loading the bytes (no load error).
        let result = tool
            .invoke(ToolInvocation {
                id: "call-3".to_string(),
                tool_name: "analyze_attachment".to_string(),
                input: json!({
                    "kind": "image",
                    "attachment_id": attachment_id,
                    "prompt": "describe",
                }),
            })
            .await
            .expect("tool result");

        // No model configured → structured error, not a load error.
        assert!(!result.ok);
        assert_eq!(result.output["config"], "attachment_models.image");
        // The error should NOT be about loading the attachment.
        assert!(
            !result.output["error"]
                .as_str()
                .is_some_and(|s| s.contains("failed to load")),
            "should not fail to load: {:?}",
            result.output
        );
    }

    #[tokio::test]
    async fn invoke_with_nonexistent_attachment_id_returns_load_error() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let fake_id = format!("{:0>64}.png", "a");

        let tool = AttachmentAnalysisTool::new(
            LoadedConfig {
                config: NaviConfig::default(),
                global_config_path: None,
                project_config_path: None,
                data_dir: tempdir.path().to_path_buf(),
            },
            tempdir.path().to_path_buf(),
        );

        let result = tool
            .invoke(ToolInvocation {
                id: "call-4".to_string(),
                tool_name: "analyze_attachment".to_string(),
                input: json!({
                    "kind": "image",
                    "attachment_id": fake_id,
                    "prompt": "describe",
                }),
            })
            .await;

        // Loading a nonexistent attachment should produce an error (Err, not Ok).
        assert!(result.is_err(), "expected error for nonexistent attachment");
    }

    #[test]
    fn media_type_inference_from_attachment_id() {
        assert_eq!(
            media_type_from_attachment_id("abc.png", AttachmentKind::Image),
            "image/png"
        );
        assert_eq!(
            media_type_from_attachment_id("abc.jpg", AttachmentKind::Image),
            "image/jpeg"
        );
        assert_eq!(
            media_type_from_attachment_id("abc.webp", AttachmentKind::Image),
            "image/webp"
        );
        assert_eq!(
            media_type_from_attachment_id("abc.bin", AttachmentKind::Image),
            "image/png"
        );
        assert_eq!(
            media_type_from_attachment_id("abc.pdf", AttachmentKind::Document),
            "application/pdf"
        );
    }
}
