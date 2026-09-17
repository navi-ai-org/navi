//! Voice / dictation API on [`NaviEngine`].
//!
//! Engine-scoped (not per-session). Transcription is **remote**: registry
//! transcription providers (OpenAI / Groq Whisper) over HTTP. Local capture
//! (recorder discovery, WAV helpers, recorder diagnostics) comes from
//! `navi-voice`; local ONNX inference was removed.
//!
//! Desktop clients push 16 kHz mono PCM for capture; remote transcription is
//! offline file transcription (WAV) via HTTP.

use std::path::{Path, PathBuf};

use navi_core::{
    CredentialStore, ProviderConfig, ProviderKind, VoiceConfig, find_transcription_provider,
    resolve_provider_api_key, resolve_transcription_model, save_global_config, save_project_config,
    transcription_provider_catalog,
};
use navi_voice::{
    DoctorInput, DoctorReport, RemoteTranscriptionConfig, RemoteTranscriptionKind, SAMPLE_RATE,
    TranscribeResult, VoiceEvent, VoiceRecorderInfo, VoiceStatus, list_available_recorders,
    run_doctor, transcribe_file_remote,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::engine::NaviEngine;
use crate::types::{NaviConfigSaveTarget, NaviError};

type Result<T> = std::result::Result<T, NaviError>;

const VOICE_EVENT_CAPACITY: usize = 128;

/// In-process voice event bus (engine-scoped).
pub(crate) struct VoiceRuntime {
    event_tx: broadcast::Sender<VoiceEvent>,
}

impl VoiceRuntime {
    pub(crate) fn new() -> Self {
        let (event_tx, _) = broadcast::channel(VOICE_EVENT_CAPACITY);
        Self { event_tx }
    }
}

impl NaviEngine {
    /// Config + remote provider status + recorder discovery.
    pub fn voice_status(&self) -> Result<VoiceStatus> {
        let loaded = self.loaded_config();
        let voice = &loaded.config.voice;
        let provider = if voice.provider.trim().is_empty() {
            "local".to_string()
        } else {
            voice.provider.clone()
        };
        let remote = voice.uses_remote_transcription();
        let registry = find_transcription_provider(&provider);
        let installed = remote && registry.is_some();
        let model = registry
            .map(|reg| resolve_transcription_model(&reg, &voice.model))
            .unwrap_or_else(|| voice.model.clone());
        let recorders = list_available_recorders()
            .into_iter()
            .map(|(kind, path)| VoiceRecorderInfo {
                id: kind.binary().to_string(),
                path: path.display().to_string(),
            })
            .collect();

        Ok(VoiceStatus {
            enabled: voice.enabled,
            provider,
            model,
            language: voice.language.clone(),
            capture: voice.capture.clone(),
            recorder: voice.recorder.clone(),
            installed,
            sample_rate: SAMPLE_RATE,
            recorders,
        })
    }

    /// List remote transcription providers from the embedded/cache registry catalog.
    pub fn voice_transcription_providers(&self) -> Vec<navi_core::RegistryTranscriptionProvider> {
        transcription_provider_catalog()
    }

    /// Update in-memory `[voice]` settings and optionally persist to disk.
    ///
    /// Only fields present in `update` are changed. Empty `provider` is treated as `"local"`.
    pub fn set_voice_config(
        &self,
        update: VoiceConfigUpdate,
        save_target: NaviConfigSaveTarget,
    ) -> Result<Option<PathBuf>> {
        let mut loaded = self.loaded_config();
        apply_voice_config_update(&mut loaded.config.voice, update)?;
        let saved = self.persist_loaded_config(&loaded, save_target)?;
        self.replace_loaded_config(loaded);
        Ok(saved)
    }

    /// Mic tools + checksums for recorders — or remote provider + credential checks.
    pub fn voice_doctor(&self) -> Result<DoctorReport> {
        let loaded = self.loaded_config();
        let voice = &loaded.config.voice;
        if voice.uses_remote_transcription() {
            return self.voice_doctor_remote(voice);
        }
        run_doctor(&DoctorInput {
            enabled: voice.enabled,
            language: voice.language.clone(),
            capture: voice.capture.clone(),
            recorder: voice.recorder.clone(),
        })
        .map_err(|e| NaviError::Config(format!("voice doctor diagnostics failed: {e}")))
    }

    fn voice_doctor_remote(&self, voice: &VoiceConfig) -> Result<DoctorReport> {
        let mut lines = Vec::new();
        let mut ok = true;
        let provider = voice.provider.trim();
        lines.push(format!("Voice doctor (remote provider: {provider})"));
        lines.push(format!("Config enabled: {}", voice.enabled));
        match find_transcription_provider(provider) {
            Some(reg) => {
                lines.push(format!("  [OK] Provider found in registry: {}", reg.label));
                lines.push(format!("  kind: {}", reg.kind));
                lines.push(format!("  models: {}", reg.models.len()));
                lines.push(format!(
                    "  default model: {}",
                    resolve_transcription_model(&reg, &voice.model)
                ));
                let store = CredentialStore::new(self.loaded_config().data_dir.clone());
                let synthetic = ProviderConfig {
                    id: reg.id.clone(),
                    label: reg.label.clone(),
                    description: reg.description.clone(),
                    kind: ProviderKind::OpenAiChatCompletions,
                    api_key_env: reg.api_key_env.clone(),
                    base_url: Some(reg.base_url.clone()),
                    ..Default::default()
                };
                if resolve_provider_api_key(&store, &synthetic, &reg.id).is_some() {
                    lines.push(format!("  [OK] API key resolved (${})", reg.api_key_env));
                } else {
                    ok = false;
                    lines.push(format!(
                        "  [FAIL] Missing API key — set ${}",
                        reg.api_key_env
                    ));
                }
            }
            None => {
                ok = false;
                lines.push(format!(
                    "  [FAIL] Unknown transcription provider '{provider}'"
                ));
                let known: Vec<_> = transcription_provider_catalog()
                    .into_iter()
                    .map(|p| p.id)
                    .collect();
                lines.push(format!("  known: {}", known.join(", ")));
            }
        }
        Ok(DoctorReport { ok, lines })
    }

    /// Subscribe to engine-global voice events (partials, final, errors).
    pub fn subscribe_voice_events(&self) -> broadcast::Receiver<VoiceEvent> {
        self.inner
            .voice
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .event_tx
            .subscribe()
    }

    /// Transcribe a WAV file.
    ///
    /// `[voice].provider` must be a remote registry transcription provider
    /// (openai | groq | …): HTTP call using registry metadata + API key (same
    /// credential resolution as LLM providers).
    pub fn voice_transcribe_file(
        &self,
        path: impl AsRef<Path>,
        language: Option<&str>,
    ) -> Result<TranscribeResult> {
        let loaded = self.loaded_config();
        if !loaded.config.voice.uses_remote_transcription() {
            return Err(local_transcription_removed_error());
        }
        self.voice_transcribe_file_remote(path.as_ref(), language)
    }

    /// Async remote transcription (preferred from async contexts).
    pub async fn voice_transcribe_file_async(
        &self,
        path: impl AsRef<Path>,
        language: Option<&str>,
    ) -> Result<TranscribeResult> {
        let loaded = self.loaded_config();
        if !loaded.config.voice.uses_remote_transcription() {
            return Err(local_transcription_removed_error());
        }
        let remote_cfg = self.resolve_remote_transcription_config(language)?;
        let result = transcribe_file_remote(&remote_cfg, path.as_ref())
            .await
            .map_err(|e| {
                NaviError::Config(format!(
                    "remote voice transcription of {} failed: {e}",
                    path.as_ref().display()
                ))
            })?;
        Ok(TranscribeResult {
            text: result.text,
            token_ids: Vec::new(),
        })
    }

    fn voice_transcribe_file_remote(
        &self,
        path: &Path,
        language: Option<&str>,
    ) -> Result<TranscribeResult> {
        let remote_cfg = self.resolve_remote_transcription_config(language)?;
        let path = path.to_path_buf();
        // Prefer existing tokio runtime (desktop / async CLI).
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            return handle.block_on(async move {
                let result = transcribe_file_remote(&remote_cfg, &path)
                    .await
                    .map_err(|e| {
                        NaviError::Config(format!(
                            "remote voice transcription of {} failed: {e}",
                            path.display()
                        ))
                    })?;
                Ok(TranscribeResult {
                    text: result.text,
                    token_ids: Vec::new(),
                })
            });
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| NaviError::Config(format!("tokio runtime for voice: {e}")))?;
        rt.block_on(async move {
            let result = transcribe_file_remote(&remote_cfg, &path)
                .await
                .map_err(|e| {
                    NaviError::Config(format!(
                        "remote voice transcription of {} failed: {e}",
                        path.display()
                    ))
                })?;
            Ok(TranscribeResult {
                text: result.text,
                token_ids: Vec::new(),
            })
        })
    }

    fn resolve_remote_transcription_config(
        &self,
        language: Option<&str>,
    ) -> Result<RemoteTranscriptionConfig> {
        let loaded = self.loaded_config();
        let voice = &loaded.config.voice;
        let provider_id = voice.provider.trim();
        let registry = find_transcription_provider(provider_id).ok_or_else(|| {
            NaviError::Config(format!(
                "unknown transcription provider '{provider_id}'. Known: {}",
                transcription_provider_catalog()
                    .iter()
                    .map(|p| p.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        let kind = RemoteTranscriptionKind::parse(&registry.kind).ok_or_else(|| {
            NaviError::Config(format!(
                "unsupported transcription kind '{}' for provider '{}'",
                registry.kind, registry.id
            ))
        })?;
        let model = resolve_transcription_model(&registry, &voice.model);
        let path = registry.resolved_path().to_string();

        // Reuse the same credential resolution as LLM providers (env → store).
        let synthetic = ProviderConfig {
            id: registry.id.clone(),
            label: registry.label.clone(),
            description: registry.description.clone(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: registry.api_key_env.clone(),
            base_url: Some(registry.base_url.clone()),
            ..Default::default()
        };
        let store = CredentialStore::new(loaded.data_dir.clone());
        let api_key = resolve_provider_api_key(&store, &synthetic, &registry.id).ok_or_else(|| {
            NaviError::Config(format!(
                "missing API key for transcription provider '{}'. Set ${} or save credentials in NAVI.",
                registry.id, registry.api_key_env
            ))
        })?;

        let lang = self.resolve_voice_language(language);
        let language = if lang.eq_ignore_ascii_case("auto") || lang.is_empty() {
            None
        } else {
            Some(lang)
        };

        Ok(RemoteTranscriptionConfig {
            provider_id: registry.id,
            kind,
            base_url: registry.base_url,
            transcription_path: path,
            api_key,
            model,
            language,
        })
    }

    fn resolve_voice_language(&self, language: Option<&str>) -> String {
        match language {
            Some(l) if !l.trim().is_empty() => l.trim().to_string(),
            _ => self.loaded_config().config.voice.language.clone(),
        }
    }

    fn persist_loaded_config(
        &self,
        loaded_config: &navi_core::LoadedConfig,
        target: NaviConfigSaveTarget,
    ) -> Result<Option<PathBuf>> {
        match target {
            NaviConfigSaveTarget::None => Ok(None),
            NaviConfigSaveTarget::Project => {
                let path = save_project_config(&self.inner.project_dir, &loaded_config.config)
                    .map_err(NaviError::from)?;
                Ok(Some(path))
            }
            NaviConfigSaveTarget::Global => {
                let global_path = loaded_config
                    .global_config_path
                    .as_ref()
                    .ok_or_else(|| NaviError::Config("global config path is unavailable".into()))?;
                let path = save_global_config(global_path, &loaded_config.config)
                    .map_err(NaviError::from)?;
                Ok(Some(path))
            }
            NaviConfigSaveTarget::Auto => {
                if loaded_config.project_config_path.is_some() {
                    let path = save_project_config(&self.inner.project_dir, &loaded_config.config)
                        .map_err(NaviError::from)?;
                    Ok(Some(path))
                } else {
                    let global_path =
                        loaded_config.global_config_path.as_ref().ok_or_else(|| {
                            NaviError::Config("global config path is unavailable".into())
                        })?;
                    let path = save_global_config(global_path, &loaded_config.config)
                        .map_err(NaviError::from)?;
                    Ok(Some(path))
                }
            }
        }
    }
}

fn local_transcription_removed_error() -> NaviError {
    NaviError::Config(
        "local voice transcription was removed; set [voice].provider to a remote transcription \
         provider (e.g. openai, groq)"
            .into(),
    )
}

/// Partial update for `[voice]` settings (all fields optional).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceConfigUpdate {
    pub enabled: Option<bool>,
    /// `"local"` or registry transcription provider id.
    pub provider: Option<String>,
    pub model: Option<String>,
    pub language: Option<String>,
    pub capture: Option<String>,
    pub recorder: Option<String>,
}

fn apply_voice_config_update(voice: &mut VoiceConfig, update: VoiceConfigUpdate) -> Result<()> {
    if let Some(v) = update.enabled {
        voice.enabled = v;
    }
    if let Some(p) = update.provider {
        let p = p.trim();
        if p.is_empty() {
            voice.provider = "local".to_string();
        } else if p.eq_ignore_ascii_case("local") {
            voice.provider = "local".to_string();
        } else if find_transcription_provider(p).is_none() {
            let known: Vec<_> = transcription_provider_catalog()
                .into_iter()
                .map(|x| x.id)
                .collect();
            return Err(NaviError::Config(format!(
                "unknown transcription provider '{p}'. Known: local, {}",
                known.join(", ")
            )));
        } else {
            voice.provider = p.to_string();
        }
    }
    if let Some(m) = update.model {
        voice.model = m;
    }
    if let Some(l) = update.language {
        voice.language = l;
    }
    if let Some(c) = update.capture {
        voice.capture = c;
    }
    if let Some(r) = update.recorder {
        voice.recorder = r;
    }
    Ok(())
}
