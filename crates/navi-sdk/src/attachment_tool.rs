use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use navi_core::{
    AttachmentKind, ContentPart, LoadedConfig, ModelConfig, ModelMessage, ModelRequest,
    ThinkingConfig, Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult,
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
}

#[derive(Debug, Deserialize)]
struct AttachmentAnalysisInput {
    kind: AttachmentKind,
    media_type: String,
    data: String,
    prompt: String,
    #[serde(default)]
    name: Option<String>,
}

#[async_trait]
impl Tool for AttachmentAnalysisTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "analyze_attachment",
            "Analyze an image, audio file, video, or document using the configured specialized attachment model. Use this when the chat model cannot inspect an attachment directly. Pass the attachment kind, MIME type, base64 data, and a focused prompt describing what to extract.",
            ToolKind::Read,
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["kind", "media_type", "data", "prompt"],
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["image", "audio", "video", "document"],
                        "description": "Attachment modality."
                    },
                    "media_type": {
                        "type": "string",
                        "description": "MIME type, for example image/png, audio/mpeg, video/mp4, or application/pdf."
                    },
                    "data": {
                        "type": "string",
                        "description": "Raw base64 attachment data with no data URL prefix."
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
        let input: AttachmentAnalysisInput =
            serde_json::from_value(invocation.input).context("invalid attachment analysis input")?;
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
                media_type: input.media_type.clone(),
                data: input.data,
            },
            AttachmentKind::Audio => ContentPart::Audio {
                media_type: input.media_type.clone(),
                data: input.data,
                name: input.name.clone(),
            },
            AttachmentKind::Video => ContentPart::Video {
                media_type: input.media_type.clone(),
                data: input.data,
                name: input.name.clone(),
            },
            AttachmentKind::Document => ContentPart::Document {
                media_type: input.media_type.clone(),
                data: input.data,
                name: input.name.clone(),
            },
        };

        let request = ModelRequest {
            model: model.name.clone(),
            messages: vec![
                ModelMessage::system("Analyze the attachment and return concise text only."),
                ModelMessage::user_multimodal(
                    input.prompt,
                    vec![ContentPart::Text {
                        text: "Analyze this attachment.".to_string(),
                    }, attachment],
                ),
            ],
            thinking: ThinkingConfig::Off,
            tools: Vec::new(),
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
