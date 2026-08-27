//! Attachment fallback models on [`NaviEngine`].

use std::path::PathBuf;

use anyhow::Context;
use navi_core::config::types::ModelConfig;
use navi_core::resolve_provider_config;

use crate::engine::NaviEngine;
use crate::types::{NaviConfigSaveTarget, NaviError};

type Result<T> = std::result::Result<T, NaviError>;

const ATTACHMENT_MODALITIES: &[&str] = &["image", "audio", "video", "document"];

fn normalize_modality(modality: &str) -> Result<&'static str> {
    let key = modality.trim().to_ascii_lowercase();
    ATTACHMENT_MODALITIES
        .iter()
        .copied()
        .find(|m| *m == key)
        .ok_or_else(|| {
            NaviError::Config(format!(
                "unknown attachment modality '{modality}' (expected image|audio|video|document)"
            ))
        })
}

impl NaviEngine {
    /// Set the specialized model used when the chat model cannot handle an attachment modality.
    ///
    /// `modality` is one of: `image`, `audio`, `video`, `document`.
    pub fn set_attachment_model(
        &self,
        modality: &str,
        provider: &str,
        model: &str,
        save_target: NaviConfigSaveTarget,
    ) -> Result<Option<PathBuf>> {
        let modality = normalize_modality(modality)?;
        let provider = provider.trim();
        let model = model.trim();
        if provider.is_empty() || model.is_empty() {
            return Err(NaviError::Config(
                "provider and model are required for attachment model override".into(),
            ));
        }

        let mut loaded = self.loaded_config();
        let provider_cfg = resolve_provider_config(&loaded.config, provider)
            .with_context(|| format!("unknown provider {provider}"))
            .map_err(NaviError::from)?;
        let entry = ModelConfig {
            provider: provider_cfg.id.clone(),
            name: model.to_string(),
        };
        match modality {
            "image" => loaded.config.attachment_models.image = Some(entry),
            "audio" => loaded.config.attachment_models.audio = Some(entry),
            "video" => loaded.config.attachment_models.video = Some(entry),
            "document" => loaded.config.attachment_models.document = Some(entry),
            // Invariant: `normalize_modality` only returns keys handled above.
            other => {
                return Err(NaviError::Config(format!(
                    "internal error: unexpected attachment modality '{other}'"
                )));
            }
        }
        let saved = self.save_loaded_config(&loaded, save_target)?;
        self.replace_loaded_config(loaded);
        Ok(saved)
    }

    /// Clear the attachment fallback for a modality (falls back to “none").
    pub fn clear_attachment_model(
        &self,
        modality: &str,
        save_target: NaviConfigSaveTarget,
    ) -> Result<Option<PathBuf>> {
        let modality = normalize_modality(modality)?;
        let mut loaded = self.loaded_config();
        match modality {
            "image" => loaded.config.attachment_models.image = None,
            "audio" => loaded.config.attachment_models.audio = None,
            "video" => loaded.config.attachment_models.video = None,
            "document" => loaded.config.attachment_models.document = None,
            // Invariant: `normalize_modality` only returns keys handled above.
            other => {
                return Err(NaviError::Config(format!(
                    "internal error: unexpected attachment modality '{other}'"
                )));
            }
        }
        let saved = self.save_loaded_config(&loaded, save_target)?;
        self.replace_loaded_config(loaded);
        Ok(saved)
    }
}
