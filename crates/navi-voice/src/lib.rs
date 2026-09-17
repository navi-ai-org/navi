//! Voice / dictation support for NAVI.
//!
//! - **Remote**: registry-backed cloud transcription (OpenAI / Groq Whisper).
//! - **Local capture**: recorder discovery, WAV helpers, recorder diagnostics.
//!
//! Local ONNX inference (`ort` / `ort-sys`, Nemotron streaming ASR) was
//! removed; there is no local STT/TTS engine in this crate anymore.

pub mod capture;
pub mod doctor;
pub mod remote;
pub mod types;
pub mod wav;

pub use capture::{RecorderKind, discover_recorder, list_available_recorders};
pub use doctor::{DoctorInput, DoctorReport, run_doctor};
pub use remote::{
    RemoteTranscribeResult, RemoteTranscriptionConfig, RemoteTranscriptionKind,
    transcribe_file_remote,
};
pub use types::{TranscribeResult, VoiceCaptureMode, VoiceEvent, VoiceRecorderInfo, VoiceStatus};
pub use wav::{
    SAMPLE_RATE, load_wav_16k_mono, load_wav_mono_f32, resample_linear, write_wav_16k_mono_bytes,
};
