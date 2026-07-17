//! Voice / dictation support for NAVI.
//!
//! - Local: ONNX Nemotron streaming ASR (`feature = "onnx"`), download, doctor.
//! - Remote: registry-backed cloud transcription (OpenAI / Groq Whisper).

pub mod capture;
pub mod doctor;
pub mod download;
pub mod engine;
pub mod lang;
pub mod mel;
pub mod paths;
pub mod remote;
pub mod types;
pub mod vocab;
pub mod wav;

pub use capture::{RecorderKind, discover_recorder, list_available_recorders};
pub use doctor::{DoctorInput, DoctorReport, run_doctor};
pub use download::{DownloadProgress, download_engine, engine_installed};
pub use lang::resolve_lang_id;
pub use mel::{CHUNK_SAMPLES, SAMPLE_RATE};
pub use paths::{VoicePaths, default_hf_repo, engine_dir_name, resolve_model_dir, voice_root};
pub use remote::{
    RemoteTranscribeResult, RemoteTranscriptionConfig, RemoteTranscriptionKind,
    transcribe_file_remote,
};
pub use types::{
    AsrEngineId, DEFAULT_VOICE_HF_REPO, VoiceCaptureMode, VoiceEvent, VoiceInstallOptions,
    VoiceManifest, VoiceRecorderInfo, VoiceStatus,
};
pub use vocab::Vocab;
pub use wav::{load_wav_16k_mono, load_wav_mono_f32, resample_linear, write_wav_16k_mono_bytes};

pub use engine::{NemotronOnnxEngine, TranscribeResult};
