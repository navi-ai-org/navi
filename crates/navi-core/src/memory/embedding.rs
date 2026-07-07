//! Local embedding generation for semantic memory search.
//!
//! When the `embeddings` feature is enabled, NAVI uses Qwen3-Embedding-0.6B
//! (GGUF format) via candle to generate 1024-dim embeddings, truncated to
//! 256 dims via Matryoshka representation.
//!
//! Without the feature, the module provides a no-op fallback and search
//! falls back to text matching (LIKE).

use anyhow::Result;
use std::path::PathBuf;

/// Target embedding dimension after Matryoshka truncation.
/// 256 dims × 4 bytes = 1KB per memory — negligible storage overhead.
pub const EMBED_DIM: usize = 256;

/// Full model embedding dimension (before truncation).
pub const FULL_EMBED_DIM: usize = 1024;

/// Default model repo on HuggingFace.
pub const DEFAULT_MODEL_REPO: &str = "Qwen/Qwen3-Embedding-0.6B-GGUF";
pub const DEFAULT_MODEL_FILE: &str = "qwen3-embedding-0.6b-q8_0.gguf";
pub const DEFAULT_TOKENIZER_REPO: &str = "Qwen/Qwen3-Embedding-0.6B";
pub const DEFAULT_TOKENIZER_FILE: &str = "tokenizer.json";

/// Trait for embedding generation — allows mocking in tests.
pub trait Embedder: Send + Sync {
    /// Generates an embedding for the given text.
    /// Returns a vector of `EMBED_DIM` f32 values.
    fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

/// No-op embedder used when the `embeddings` feature is disabled.
/// Always returns an error — callers should fall back to text search.
pub struct NoEmbedder;

impl Embedder for NoEmbedder {
    fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        anyhow::bail!("embeddings feature is not enabled")
    }
}

/// Configuration for the local embedding model.
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// Path to the GGUF model file on disk.
    pub model_path: PathBuf,
    /// Path to the tokenizer.json file on disk.
    pub tokenizer_path: PathBuf,
    /// Whether to normalize embeddings (L2 norm = 1.0).
    pub normalize: bool,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model_path: PathBuf::new(),
            tokenizer_path: PathBuf::new(),
            normalize: true,
        }
    }
}

#[cfg(feature = "embeddings")]
mod candle_embedder {
    use super::*;
    use anyhow::{Context, Result as AnyResult};
    use candle_core::{DType, Device, Tensor};
    use candle_core::quantized::gguf_file;
    use candle_transformers::models::quantized_qwen2::ModelWeights;
    use std::io::BufReader;
    use std::fs::File;
    use tokenizers::Tokenizer;

    /// Local embedding model using candle (pure Rust, no C++ dependency).
    /// Loads Qwen3-Embedding-0.6B in GGUF format and runs on CPU.
    pub struct CandleEmbedder {
        model: std::sync::Mutex<ModelWeights>,
        tokenizer: Tokenizer,
        device: Device,
        config: EmbeddingConfig,
    }

    impl CandleEmbedder {
        /// Loads the model from a GGUF file on disk.
        pub fn load(config: EmbeddingConfig) -> AnyResult<Self> {
            let device = Device::Cpu;

            // Open GGUF file
            let file = File::open(&config.model_path)
                .with_context(|| format!("Failed to open GGUF file: {:?}", config.model_path))?;
            let mut reader = BufReader::new(file);

            // Read GGUF content
            let ct = gguf_file::Content::read(&mut reader)
                .context("Failed to read GGUF content")?;

            // Build quantized model weights from GGUF
            let model = ModelWeights::from_gguf(ct, &mut reader, &device)
                .context("Failed to build quantized Qwen2 model from GGUF")?;

            // Load tokenizer
            let tokenizer = Tokenizer::from_file(&config.tokenizer_path)
                .map_err(|e| anyhow::anyhow!("Failed to load tokenizer from {:?}: {}", config.tokenizer_path, e))?;

            Ok(Self {
                model: std::sync::Mutex::new(model),
                tokenizer,
                device,
                config,
            })
        }

        /// Mean pooling over token embeddings, then optional L2 normalization.
        fn mean_pool(&self, token_embeddings: &Tensor, attention_mask: &Tensor) -> AnyResult<Tensor> {
            let mask = attention_mask
                .to_dtype(DType::F32)?
                .unsqueeze(2)?; // [1, seq_len, 1]

            let masked = token_embeddings.broadcast_mul(&mask)?;
            let sum = masked.sum(1)?; // [1, hidden_size]

            let mask_sum = mask.sum(1)?; // [1, 1]
            let pooled = sum.broadcast_div(&mask_sum)?;

            if self.config.normalize {
                let norm = pooled.sqr()?.sum(1)?.sqrt()?;
                let pooled = pooled.broadcast_div(&norm.unsqueeze(1)?)?;
                Ok(pooled)
            } else {
                Ok(pooled)
            }
        }
    }

    impl Embedder for CandleEmbedder {
        fn embed(&self, text: &str) -> AnyResult<Vec<f32>> {
            let encoding = self
                .tokenizer
                .encode(text, true)
                .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

            let input_ids = encoding.get_ids();
            let attention_mask = encoding.get_attention_mask();

            // Create input_ids tensor [1, seq_len] as u32
            let input_ids_tensor = Tensor::from_slice(
                input_ids.iter().map(|&v| v as u32).collect::<Vec<_>>().as_slice(),
                (1, input_ids.len()),
                &self.device,
            )?;

            // Create attention mask tensor [1, seq_len] as u32
            let attention_mask_tensor = Tensor::from_slice(
                attention_mask.iter().map(|&v| v as u32).collect::<Vec<_>>().as_slice(),
                (1, attention_mask.len()),
                &self.device,
            )?;

            // Forward pass — quantized model returns hidden states
            let mut model = self.model.lock().map_err(|e| anyhow::anyhow!("model lock poisoned: {}", e))?;
            let embedded = model.forward(&input_ids_tensor, 0)?;
            // embedded: [1, seq_len, hidden_size]

            // Mean pooling with attention mask
            let pooled = self.mean_pool(&embedded, &attention_mask_tensor)?;
            // pooled: [1, hidden_size]

            // Extract to vec
            let full_embedding = pooled.to_vec2::<f32>()?.into_iter().next().unwrap_or_default();

            // Matryoshka truncation: take first EMBED_DIM dimensions
            let truncated: Vec<f32> = full_embedding
                .into_iter()
                .take(EMBED_DIM)
                .collect();

            // Re-normalize after truncation
            if self.config.normalize && !truncated.is_empty() {
                let norm: f32 = truncated.iter().map(|v| v * v).sum::<f32>().sqrt();
                if norm > 0.0 {
                    return Ok(truncated.iter().map(|v| v / norm).collect());
                }
            }

            Ok(truncated)
        }
    }
}

#[cfg(feature = "embeddings")]
pub use candle_embedder::CandleEmbedder;

/// Creates an embedder based on whether the feature is enabled and the model exists.
pub fn create_embedder(_config: EmbeddingConfig) -> Box<dyn Embedder> {
    #[cfg(feature = "embeddings")]
    {
        if _config.model_path.exists() && _config.tokenizer_path.exists() {
            match CandleEmbedder::load(_config) {
                Ok(embedder) => {
                    tracing::info!("Local embedding model loaded successfully");
                    return Box::new(embedder);
                }
                Err(e) => {
                    tracing::warn!("Failed to load embedding model: {}, falling back to text search", e);
                }
            }
        } else {
            if !_config.model_path.exists() {
                tracing::info!(
                    "Embedding model not found at {:?}. Download with: huggingface-cli download {} {}",
                    _config.model_path,
                    DEFAULT_MODEL_REPO,
                    DEFAULT_MODEL_FILE
                );
            }
            if !_config.tokenizer_path.exists() {
                tracing::info!(
                    "Tokenizer not found at {:?}. Download with: huggingface-cli download {} {}",
                    _config.tokenizer_path,
                    DEFAULT_TOKENIZER_REPO,
                    DEFAULT_TOKENIZER_FILE
                );
            }
        }
    }

    #[allow(unreachable_code)]
    Box::new(NoEmbedder)
}

/// Convenience: check if embeddings are available at runtime.
pub fn embeddings_available() -> bool {
    cfg!(feature = "embeddings")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_embedder_returns_error() {
        let embedder = NoEmbedder;
        assert!(embedder.embed("test").is_err());
    }

    #[test]
    fn test_embed_dim() {
        assert_eq!(EMBED_DIM, 256);
        assert_eq!(FULL_EMBED_DIM, 1024);
    }

    #[test]
    fn test_create_embedder_without_feature() {
        let config = EmbeddingConfig::default();
        let embedder = create_embedder(config);
        // Without feature flag, should be NoEmbedder
        assert!(embedder.embed("test").is_err());
    }
}
