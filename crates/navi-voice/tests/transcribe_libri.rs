//! End-to-end Nemotron ONNX transcription (requires installed model).
//!
//! Runs when `NAVI_VOICE_MODEL_DIR` is set, or when the default install path
//! under `$XDG_DATA_HOME/navi` (or `~/.local/share/navi`) exists.

#![cfg(feature = "onnx")]

use std::path::{Path, PathBuf};

use navi_voice::NemotronOnnxEngine;

fn model_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NAVI_VOICE_MODEL_DIR") {
        let path = PathBuf::from(p);
        if path.join("onnx/encoder.onnx").is_file() {
            return Some(path);
        }
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
        })?;
    let path = data
        .join("navi/voice/models/nemotron-3.5-asr-streaming-0.6b-onnx");
    if path.join("onnx/encoder.onnx").is_file() {
        Some(path)
    } else {
        None
    }
}

fn sample_wav() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NAVI_VOICE_TEST_WAV") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    let candidates = [
        PathBuf::from("/tmp/libri16.wav"),
        PathBuf::from("crates/navi-voice/testdata/libri16.wav"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

#[test]
fn transcribe_libri_contains_expected_words() {
    let Some(model) = model_dir() else {
        eprintln!("skip: Nemotron model not installed");
        return;
    };
    let Some(wav) = sample_wav() else {
        eprintln!("skip: no sample WAV (set NAVI_VOICE_TEST_WAV or place /tmp/libri16.wav)");
        return;
    };

    let mut engine = NemotronOnnxEngine::load(&model, "en-US").expect("load engine");
    let result = engine.transcribe_wav(&wav).expect("transcribe");
    eprintln!("transcript: {}", result.text);
    assert!(
        !result.token_ids.is_empty(),
        "expected non-empty tokens, text={:?}",
        result.text
    );
    let lower = result.text.to_ascii_lowercase();
    // LibriSpeech sample: "going along slushy country roads..."
    assert!(
        lower.contains("going") || lower.contains("country") || lower.contains("roads"),
        "unexpected transcript: {}",
        result.text
    );
}

#[test]
fn load_rejects_missing_dir() {
    let err = NemotronOnnxEngine::load(Path::new("/tmp/navi-voice-missing-xyz"), "en-US");
    assert!(err.is_err());
}
