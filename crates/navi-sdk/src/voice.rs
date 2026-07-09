//! Local voice / dictation API on [`NaviEngine`].
//!
//! Engine-scoped (not per-session). Desktop clients push 16 kHz mono PCM;
//! management APIs mirror CLI `navi voice status|doctor|init|transcribe`.

use std::path::{Path, PathBuf};

use navi_voice::{
    AsrEngineId, CHUNK_SAMPLES, DoctorInput, DoctorReport, NemotronOnnxEngine, SAMPLE_RATE,
    TranscribeResult, VoiceEvent, VoiceInstallOptions, VoiceRecorderInfo, VoiceStatus,
    download_engine, engine_installed, list_available_recorders, resolve_model_dir, run_doctor,
};
use tokio::sync::broadcast;

use crate::engine::NaviEngine;
use crate::types::NaviError;

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
        let engine = Self::parse_engine_id(Some(voice.engine.as_str()), "nemotron_streaming")
            .unwrap_or_default();
        let model_dir = resolve_model_dir(&loaded.data_dir, &options, engine);
        let installed = engine_installed(&loaded.data_dir, &options, engine);
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

    /// Mic tools, model files, checksums.
    pub fn voice_doctor(&self) -> Result<DoctorReport> {
        let loaded = self.loaded_config();
        let voice = &loaded.config.voice;
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

    /// Transcribe a WAV (any rate; resampled to 16 kHz mono). Blocking ONNX.
    pub fn voice_transcribe_file(
        &self,
        path: impl AsRef<Path>,
        language: Option<&str>,
    ) -> Result<TranscribeResult> {
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
