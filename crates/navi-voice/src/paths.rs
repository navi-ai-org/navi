use std::path::{Path, PathBuf};

use crate::types::{AsrEngineId, DEFAULT_VOICE_HF_REPO, VoiceInstallOptions};

/// `{data_dir}/voice`
pub fn voice_root(data_dir: &Path) -> PathBuf {
    data_dir.join("voice")
}

/// Directory name under `models/` for an engine.
pub fn engine_dir_name(engine: AsrEngineId) -> &'static str {
    match engine {
        AsrEngineId::NemotronStreaming => "nemotron-3.5-asr-streaming-0.6b-onnx",
        AsrEngineId::DistilWhisper => "distil-whisper-large-v2",
    }
}

/// Resolved model directory for an engine.
pub fn resolve_model_dir(
    data_dir: &Path,
    options: &VoiceInstallOptions,
    engine: AsrEngineId,
) -> PathBuf {
    if !options.model_dir.trim().is_empty() {
        return PathBuf::from(options.model_dir.trim()).join(engine_dir_name(engine));
    }
    voice_root(data_dir)
        .join("models")
        .join(engine_dir_name(engine))
}

pub fn default_hf_repo(options: &VoiceInstallOptions, engine: AsrEngineId) -> String {
    match engine {
        AsrEngineId::NemotronStreaming => {
            if options.hf_repo_nemotron.trim().is_empty() {
                DEFAULT_VOICE_HF_REPO.to_string()
            } else {
                options.hf_repo_nemotron.trim().to_string()
            }
        }
        AsrEngineId::DistilWhisper => {
            // Placeholder until Phase 2 packages a NAVI distil repo.
            "distil-whisper/distil-large-v2".to_string()
        }
    }
}

/// Convenience bundle of resolved paths for an engine install.
#[derive(Debug, Clone)]
pub struct VoicePaths {
    pub root: PathBuf,
    pub models_root: PathBuf,
    pub engine_dir: PathBuf,
    pub manifest: PathBuf,
    pub checksums: PathBuf,
}

impl VoicePaths {
    pub fn resolve(data_dir: &Path, options: &VoiceInstallOptions, engine: AsrEngineId) -> Self {
        let root = voice_root(data_dir);
        let engine_dir = resolve_model_dir(data_dir, options, engine);
        Self {
            models_root: root.join("models"),
            root,
            manifest: engine_dir.join("navi-manifest.json"),
            checksums: engine_dir.join("SHA256SUMS"),
            engine_dir,
        }
    }

    pub fn is_installed(&self) -> bool {
        self.manifest.is_file() && self.checksums.is_file()
    }
}
