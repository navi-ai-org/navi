//! ASR engine backends.

#[cfg(feature = "onnx")]
mod nemotron_onnx;

#[cfg(feature = "onnx")]
pub use nemotron_onnx::{NemotronOnnxEngine, TranscribeResult};

/// Offline/streaming transcription result (always available).
#[cfg(not(feature = "onnx"))]
#[derive(Debug, Clone)]
pub struct TranscribeResult {
    pub text: String,
    pub token_ids: Vec<usize>,
}

/// Stub when built without the `onnx` feature (portable / musl releases).
#[cfg(not(feature = "onnx"))]
pub struct NemotronOnnxEngine {
    model_dir: std::path::PathBuf,
}

#[cfg(not(feature = "onnx"))]
impl NemotronOnnxEngine {
    pub fn load(model_dir: impl AsRef<std::path::Path>, _language: &str) -> anyhow::Result<Self> {
        let _ = model_dir;
        anyhow::bail!(
            "voice ONNX engine not compiled into this binary (build with --features voice-onnx)"
        )
    }

    pub fn model_dir(&self) -> &std::path::Path {
        &self.model_dir
    }

    pub fn sample_rate(&self) -> u32 {
        crate::mel::SAMPLE_RATE
    }

    pub fn chunk_samples(&self) -> usize {
        crate::mel::CHUNK_SAMPLES
    }

    pub fn set_language(&mut self, _language: &str) {}

    pub fn reset(&mut self) {}

    pub fn push_audio(&mut self, _samples: &[f32]) -> anyhow::Result<String> {
        anyhow::bail!("voice ONNX engine not compiled into this binary")
    }

    pub fn flush(&mut self) -> anyhow::Result<String> {
        anyhow::bail!("voice ONNX engine not compiled into this binary")
    }

    pub fn partial_text(&self) -> String {
        String::new()
    }

    pub fn transcribe_wav(&mut self, _path: &std::path::Path) -> anyhow::Result<TranscribeResult> {
        anyhow::bail!("voice ONNX engine not compiled into this binary")
    }
}
