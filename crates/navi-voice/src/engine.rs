//! ASR engine backends.

#[cfg(feature = "onnx")]
mod nemotron_onnx;

#[cfg(feature = "onnx")]
pub use nemotron_onnx::{NemotronOnnxEngine, TranscribeResult};
