//! Voice / dictation API on [`NaviEngine`].
//!
//! Engine-scoped (not per-session). Supports:
//! - **Local** ONNX Nemotron (feature `voice-onnx`)
//! - **Remote** registry transcription providers (OpenAI / Groq Whisper, Wispr Flow)
//!
//! Desktop clients push 16 kHz mono PCM for local streaming; remote path is
//! offline file transcription (WAV) via HTTP.

use std::path::{Path, PathBuf};

use navi_core::{
    CredentialStore, ProviderConfig, ProviderKind, VoiceConfig, find_transcription_provider,
    resolve_provider_api_key, resolve_transcription_model, save_global_config, save_project_config,
    transcription_provider_catalog,
};
use navi_voice::{
    AsrEngineId, CHUNK_SAMPLES, DoctorInput, DoctorReport, NemotronOnnxEngine,
    RemoteTranscriptionConfig, RemoteTranscriptionKind, SAMPLE_RATE, TranscribeResult, VoiceEvent,
    VoiceInstallOptions, VoiceRecorderInfo, VoiceStatus, download_engine, engine_installed,
    list_available_recorders, resolve_model_dir, run_doctor, transcribe_file_remote,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::engine::NaviEngine;
use crate::types::{NaviConfigSaveTarget, NaviError};

type Result<T> = std::result::Result<T, NaviError>;

const VOICE_EVENT_CAPACITY: usize = 128;

/// In-process voice runtime (lazy ONNX engine + event bus).
pub(crate) struct VoiceRuntime {
    engine: Option<NemotronOnnxEngine>,
    active: bool,
    event_tx: broadcast::Sender<VoiceEvent>,
}

impl VoiceRuntime {
    pub(crate) fn new() -> Self {
        let (event_tx, _) = broadcast::channel(VOICE_EVENT_CAPACITY);
        Self {
            engine: None,
            active: false,
            event_tx,
        }
    }

    fn emit(&self, event: VoiceEvent) {
        let _ = self.event_tx.send(event);
    }
}

impl NaviEngine {
    fn voice_install_options(&self) -> VoiceInstallOptions {
        let cfg = self.loaded_config();
        VoiceInstallOptions {
            model_dir: cfg.config.voice.model_dir.clone(),
            hf_repo_nemotron: cfg.config.voice.hf_repo_nemotron.clone(),
        }
    }

    fn parse_engine_id(engine: Option<&str>, fallback: &str) -> Result<AsrEngineId> {
        let raw = engine.unwrap_or(fallback);
        AsrEngineId::parse(raw).ok_or_else(|| {
            NaviError::Config(format!(
                "unknown voice engine '{raw}'. Use: nemotron_streaming | distil_whisper"
            ))
        })
    }

    /// Config + install + recorder discovery + streaming flag.
    pub fn voice_status(&self) -> Result<VoiceStatus> {
        let loaded = self.loaded_config();
        let voice = &loaded.config.voice;
        let options = self.voice_install_options();
        let provider = if voice.provider.trim().is_empty() {
            "local".to_string()
        } else {
            voice.provider.clone()
        };
        let remote = voice.uses_remote_transcription();
        let engine = Self::parse_engine_id(Some(voice.engine.as_str()), "nemotron_streaming")
            .unwrap_or_default();
        let model_dir = resolve_model_dir(&loaded.data_dir, &options, engine);
        let installed = if remote {
            find_transcription_provider(&provider).is_some()
        } else {
            engine_installed(&loaded.data_dir, &options, engine)
        };
        let model = if remote {
            find_transcription_provider(&provider)
                .map(|p| resolve_transcription_model(&p, &voice.model))
                .unwrap_or_else(|| voice.model.clone())
        } else {
            voice.model.clone()
        };
        let recorders = list_available_recorders()
            .into_iter()
            .map(|(kind, path)| VoiceRecorderInfo {
                id: kind.binary().to_string(),
                path: path.display().to_string(),
            })
            .collect();
        let streaming_active = self
            .inner
            .voice
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active;

        Ok(VoiceStatus {
            enabled: voice.enabled,
            provider,
            model,
            engine: engine.as_str().to_string(),
            language: voice.language.clone(),
            capture: voice.capture.clone(),
            recorder: voice.recorder.clone(),
            model_dir: model_dir.display().to_string(),
            installed,
            streaming_active,
            sample_rate: SAMPLE_RATE,
            chunk_samples: CHUNK_SAMPLES as u32,
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

    /// Mic tools, model files, checksums — or remote provider + credential checks.
    pub fn voice_doctor(&self) -> Result<DoctorReport> {
        let loaded = self.loaded_config();
        let voice = &loaded.config.voice;
        if voice.uses_remote_transcription() {
            return self.voice_doctor_remote(voice);
        }
        let engine = Self::parse_engine_id(Some(voice.engine.as_str()), "nemotron_streaming")
            .unwrap_or_default();
        run_doctor(
            &loaded.data_dir,
            &DoctorInput {
                enabled: voice.enabled,
                engine,
                language: voice.language.clone(),
                capture: voice.capture.clone(),
                recorder: voice.recorder.clone(),
                options: self.voice_install_options(),
            },
        )
        .map_err(|e| NaviError::Config(e.to_string()))
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

    /// Whether the given engine package is installed under data_dir.
    pub fn voice_engine_installed(&self, engine: Option<&str>) -> Result<bool> {
        let loaded = self.loaded_config();
        let fallback = loaded.config.voice.engine.as_str();
        let engine = Self::parse_engine_id(engine, fallback)?;
        Ok(engine_installed(
            &loaded.data_dir,
            &self.voice_install_options(),
            engine,
        ))
    }

    /// Download + verify a voice engine package (async).
    pub async fn voice_init(&self, engine: Option<&str>, force: bool) -> Result<PathBuf> {
        let loaded = self.loaded_config();
        let fallback = loaded.config.voice.engine.as_str();
        let engine = Self::parse_engine_id(engine, fallback)?;
        let options = self.voice_install_options();
        let data_dir = loaded.data_dir.clone();

        let progress = Box::new(move |downloaded: u64, total: Option<u64>, file: &str| {
            tracing::debug!(
                file,
                downloaded,
                total = total.unwrap_or(0),
                "voice model download progress"
            );
        });

        download_engine(&data_dir, &options, engine, force, Some(progress))
            .await
            .map_err(|e| NaviError::Config(e.to_string()))
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
    /// - **Remote** (`[voice].provider` = openai|groq|wispr-flow|…): HTTP call using
    ///   registry metadata + API key (same credential resolution as LLM providers).
    /// - **Local**: Blocking ONNX Nemotron (requires `voice-onnx` feature + installed model).
    pub fn voice_transcribe_file(
        &self,
        path: impl AsRef<Path>,
        language: Option<&str>,
    ) -> Result<TranscribeResult> {
        let loaded = self.loaded_config();
        let voice = &loaded.config.voice;
        if voice.uses_remote_transcription() {
            // Async remote call from sync API: use a small runtime if none is active.
            return self.voice_transcribe_file_remote(path.as_ref(), language);
        }
        let lang = self.resolve_voice_language(language);
        let mut rt = self.inner.voice.lock().unwrap_or_else(|e| e.into_inner());
        self.ensure_nemotron_locked(&mut rt, &lang)?;
        let engine = rt
            .engine
            .as_mut()
            .ok_or_else(|| NaviError::Config("voice engine not loaded".into()))?;
        engine.set_language(&lang);
        engine
            .transcribe_wav(path.as_ref())
            .map_err(|e| NaviError::Config(e.to_string()))
    }

    /// Async remote transcription (preferred from async contexts).
    pub async fn voice_transcribe_file_async(
        &self,
        path: impl AsRef<Path>,
        language: Option<&str>,
    ) -> Result<TranscribeResult> {
        let loaded = self.loaded_config();
        let voice = &loaded.config.voice;
        if !voice.uses_remote_transcription() {
            // Local path is sync/ONNX — run in blocking pool.
            let path = path.as_ref().to_path_buf();
            let language = language.map(|s| s.to_string());
            let this = self.clone();
            return tokio::task::spawn_blocking(move || {
                this.voice_transcribe_file(path, language.as_deref())
            })
            .await
            .map_err(|e| NaviError::Config(format!("voice transcribe join: {e}")))?;
        }
        let remote_cfg = self.resolve_remote_transcription_config(language)?;
        let result = transcribe_file_remote(&remote_cfg, path.as_ref())
            .await
            .map_err(|e| NaviError::Config(e.to_string()))?;
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
                    .map_err(|e| NaviError::Config(e.to_string()))?;
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
                .map_err(|e| NaviError::Config(e.to_string()))?;
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

    /// Start a streaming recognition session (client pushes PCM).
    pub fn voice_start_stream(&self, language: Option<&str>) -> Result<()> {
        let lang = self.resolve_voice_language(language);
        let mut rt = self.inner.voice.lock().unwrap_or_else(|e| e.into_inner());
        if rt.active {
            return Err(NaviError::Config(
                "voice stream already active; call voice_end_stream or voice_cancel_stream first"
                    .into(),
            ));
        }
        self.ensure_nemotron_locked(&mut rt, &lang)?;
        let engine = rt
            .engine
            .as_mut()
            .ok_or_else(|| NaviError::Config("voice engine not loaded".into()))?;
        engine.set_language(&lang);
        engine.reset();
        rt.active = true;
        rt.emit(VoiceEvent::Started {
            engine: AsrEngineId::NemotronStreaming.as_str().to_string(),
        });
        Ok(())
    }

    /// Push 16 kHz mono f32 samples. Returns text delta emitted this call (may be empty).
    pub fn voice_push_pcm(&self, samples: &[f32]) -> Result<String> {
        let mut rt = self.inner.voice.lock().unwrap_or_else(|e| e.into_inner());
        if !rt.active {
            return Err(NaviError::Config(
                "voice stream not active; call voice_start_stream first".into(),
            ));
        }
        let engine = rt
            .engine
            .as_mut()
            .ok_or_else(|| NaviError::Config("voice engine not loaded".into()))?;
        let delta = engine
            .push_audio(samples)
            .map_err(|e| NaviError::Config(e.to_string()))?;
        if !delta.is_empty() {
            let partial = engine.partial_text();
            rt.emit(VoiceEvent::Partial { text: partial });
        }
        Ok(delta)
    }

    /// Flush remaining audio, emit final text, stop stream.
    pub fn voice_end_stream(&self) -> Result<String> {
        let mut rt = self.inner.voice.lock().unwrap_or_else(|e| e.into_inner());
        if !rt.active {
            return Err(NaviError::Config(
                "voice stream not active; call voice_start_stream first".into(),
            ));
        }
        let engine = rt
            .engine
            .as_mut()
            .ok_or_else(|| NaviError::Config("voice engine not loaded".into()))?;
        let _ = engine
            .flush()
            .map_err(|e| NaviError::Config(e.to_string()))?;
        let text = engine.partial_text();
        rt.active = false;
        rt.emit(VoiceEvent::Final { text: text.clone() });
        rt.emit(VoiceEvent::Stopped);
        Ok(text)
    }

    /// Abort stream without committing final text.
    pub fn voice_cancel_stream(&self) -> Result<()> {
        let mut rt = self.inner.voice.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(engine) = rt.engine.as_mut() {
            engine.reset();
        }
        if rt.active {
            rt.active = false;
            rt.emit(VoiceEvent::Stopped);
        }
        Ok(())
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

    fn ensure_nemotron_locked(&self, rt: &mut VoiceRuntime, language: &str) -> Result<()> {
        let loaded = self.loaded_config();
        let options = self.voice_install_options();
        let engine_id = AsrEngineId::NemotronStreaming;
        if !engine_installed(&loaded.data_dir, &options, engine_id) {
            let model_dir = resolve_model_dir(&loaded.data_dir, &options, engine_id);
            let hint =
                "Run navi voice init --engine nemotron_streaming (or engine.voiceInit from N-API)"
                    .to_string();
            rt.emit(VoiceEvent::ModelMissing {
                engine: engine_id.as_str().to_string(),
                hint: hint.clone(),
            });
            return Err(NaviError::Config(format!(
                "Nemotron streaming model not installed at {} — {hint}",
                model_dir.display()
            )));
        }
        if rt.engine.is_none() {
            let model_dir = resolve_model_dir(&loaded.data_dir, &options, engine_id);
            let eng = NemotronOnnxEngine::load(&model_dir, language)
                .map_err(|e| NaviError::Config(format!("load Nemotron ONNX engine: {e:#}")))?;
            rt.engine = Some(eng);
        }
        Ok(())
    }
}

/// Partial update for `[voice]` settings (all fields optional).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceConfigUpdate {
    pub enabled: Option<bool>,
    /// `"local"` or registry transcription provider id.
    pub provider: Option<String>,
    pub model: Option<String>,
    pub engine: Option<String>,
    pub language: Option<String>,
    pub capture: Option<String>,
    pub recorder: Option<String>,
    pub model_dir: Option<String>,
    pub hf_repo_nemotron: Option<String>,
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
    if let Some(e) = update.engine {
        if AsrEngineId::parse(&e).is_none() {
            return Err(NaviError::Config(format!(
                "unknown voice engine '{e}'. Use: nemotron_streaming | distil_whisper"
            )));
        }
        voice.engine = e;
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
    if let Some(d) = update.model_dir {
        voice.model_dir = d;
    }
    if let Some(h) = update.hf_repo_nemotron {
        voice.hf_repo_nemotron = h;
    }
    Ok(())
}
