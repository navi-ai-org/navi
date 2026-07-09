//! Nemotron 3.5 ASR streaming via ONNX Runtime (onnx-community INT4 layout).
//!
//! Graph I/O (fixed shapes for 560 ms chunk export):
//! - encoder `audio_signal` `[1, 65, 128]` (time × mels), caches, `lang_id`
//! - decoder LSTM + joint RNNT greedy decode

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ndarray::{Array1, Array2, Array3, Array4, s};
use ort::session::Session;
use ort::value::Tensor;

use crate::lang::resolve_lang_id;
use crate::mel::{
    CHUNK_MEL_FRAMES, CHUNK_SAMPLES, ENCODER_MEL_FRAMES, MelFrontend, N_MELS, PRE_ENCODE_CACHE,
    SAMPLE_RATE,
};
use crate::vocab::Vocab;
use crate::wav::load_wav_16k_mono;

const HIDDEN: usize = 1024;
const NUM_LAYERS: usize = 24;
const LEFT_CTX: usize = 56;
const CONV_CTX: usize = 8;
const LSTM_DIM: usize = 640;
const LSTM_LAYERS: usize = 2;
const MAX_SYMBOLS: usize = 10;

/// Result of a full-utterance transcription.
#[derive(Debug, Clone)]
pub struct TranscribeResult {
    pub text: String,
    pub token_ids: Vec<usize>,
}

/// Streaming Nemotron ONNX engine (cache-aware FastConformer-RNNT).
pub struct NemotronOnnxEngine {
    model_dir: PathBuf,
    encoder: Session,
    decoder: Session,
    joint: Session,
    vocab: Vocab,
    mel: MelFrontend,
    lang_id: i64,
    cache_last_channel: Array4<f32>,
    cache_last_time: Array4<f32>,
    cache_last_channel_len: Array1<i64>,
    h: Array3<f32>,
    c: Array3<f32>,
    last_token: i64,
    /// Accumulated 16 kHz mono PCM for continuous mel (streaming + offline).
    raw_audio: Vec<f32>,
    /// Mel frames already fed to the encoder.
    processed_mel_frames: usize,
    tokens: Vec<usize>,
}

impl NemotronOnnxEngine {
    /// Load encoder/decoder/joint sessions and vocab from a NAVI model package dir.
    pub fn load(model_dir: impl AsRef<Path>, language: &str) -> Result<Self> {
        let model_dir = model_dir.as_ref().to_path_buf();
        let onnx = model_dir.join("onnx");
        let enc_path = onnx.join("encoder.onnx");
        let dec_path = onnx.join("decoder.onnx");
        let jnt_path = onnx.join("joint.onnx");
        let vocab_path = model_dir.join("vocab.txt");

        for p in [&enc_path, &dec_path, &jnt_path, &vocab_path] {
            if !p.is_file() {
                bail!("missing model file: {}", p.display());
            }
        }

        let encoder = Session::builder()
            .context("ort session builder")?
            .commit_from_file(&enc_path)
            .with_context(|| format!("load encoder {}", enc_path.display()))?;
        let decoder = Session::builder()
            .context("ort session builder")?
            .commit_from_file(&dec_path)
            .with_context(|| format!("load decoder {}", dec_path.display()))?;
        let joint = Session::builder()
            .context("ort session builder")?
            .commit_from_file(&jnt_path)
            .with_context(|| format!("load joint {}", jnt_path.display()))?;

        let vocab = Vocab::load(&vocab_path)?;
        let blank = vocab.blank_id() as i64;

        Ok(Self {
            model_dir,
            encoder,
            decoder,
            joint,
            vocab,
            mel: MelFrontend::new(),
            lang_id: resolve_lang_id(language),
            cache_last_channel: Array4::zeros((1, NUM_LAYERS, LEFT_CTX, HIDDEN)),
            cache_last_time: Array4::zeros((1, NUM_LAYERS, HIDDEN, CONV_CTX)),
            cache_last_channel_len: Array1::from_vec(vec![0i64]),
            h: Array3::zeros((LSTM_LAYERS, 1, LSTM_DIM)),
            c: Array3::zeros((LSTM_LAYERS, 1, LSTM_DIM)),
            last_token: blank,
            raw_audio: Vec::new(),
            processed_mel_frames: 0,
            tokens: Vec::new(),
        })
    }

    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }

    pub fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    pub fn chunk_samples(&self) -> usize {
        CHUNK_SAMPLES
    }

    pub fn set_language(&mut self, language: &str) {
        self.lang_id = resolve_lang_id(language);
    }

    /// Reset streaming state (caches, decoder, audio buffer, tokens).
    pub fn reset(&mut self) {
        self.cache_last_channel.fill(0.0);
        self.cache_last_time.fill(0.0);
        self.cache_last_channel_len[0] = 0;
        self.h.fill(0.0);
        self.c.fill(0.0);
        self.last_token = self.vocab.blank_id() as i64;
        self.raw_audio.clear();
        self.processed_mel_frames = 0;
        self.tokens.clear();
    }

    /// Current partial transcript.
    pub fn partial_text(&self) -> String {
        self.vocab.decode(&self.tokens)
    }

    /// Transcribe a whole mono PCM buffer at 16 kHz.
    pub fn transcribe_pcm(&mut self, audio_16k: &[f32]) -> Result<TranscribeResult> {
        self.reset();
        self.raw_audio.extend_from_slice(audio_16k);
        self.drain_mel_chunks()?;
        Ok(TranscribeResult {
            text: self.vocab.decode(&self.tokens),
            token_ids: self.tokens.clone(),
        })
    }

    /// Transcribe a WAV file (any rate; resampled to 16 kHz mono).
    pub fn transcribe_wav(&mut self, path: impl AsRef<Path>) -> Result<TranscribeResult> {
        let audio = load_wav_16k_mono(path.as_ref())?;
        self.transcribe_pcm(&audio)
    }

    /// Append 16 kHz mono samples and decode any newly completed mel chunks.
    /// Returns text decoded from tokens emitted in this call (delta).
    pub fn push_audio(&mut self, samples_16k: &[f32]) -> Result<String> {
        let before = self.tokens.len();
        self.raw_audio.extend_from_slice(samples_16k);
        self.drain_mel_chunks()?;
        Ok(self.vocab.decode(&self.tokens[before..]))
    }

    /// Process one fixed-size PCM chunk (pad/truncate to `CHUNK_SAMPLES`).
    pub fn process_pcm_chunk(&mut self, chunk: &[f32]) -> Result<String> {
        let mut buf = chunk.to_vec();
        if buf.len() < CHUNK_SAMPLES {
            buf.resize(CHUNK_SAMPLES, 0.0);
        } else if buf.len() > CHUNK_SAMPLES {
            buf.truncate(CHUNK_SAMPLES);
        }
        self.push_audio(&buf)
    }

    /// Flush remaining audio shorter than a full chunk (pads final mel chunk).
    pub fn flush(&mut self) -> Result<String> {
        let before = self.tokens.len();
        let (mel, n_frames) = self.mel.log_mel_frame_major(&self.raw_audio);
        if n_frames > self.processed_mel_frames {
            let main_start = self.processed_mel_frames;
            let main_len = n_frames - main_start;
            // Pad to CHUNK_MEL_FRAMES with zeros in encode path
            self.run_chunk_from_mel(&mel, n_frames, main_start, main_len)?;
            self.processed_mel_frames = n_frames;
        }
        Ok(self.vocab.decode(&self.tokens[before..]))
    }

    fn drain_mel_chunks(&mut self) -> Result<()> {
        let (mel, n_frames) = self.mel.log_mel_frame_major(&self.raw_audio);
        while self.processed_mel_frames + CHUNK_MEL_FRAMES <= n_frames {
            let main_start = self.processed_mel_frames;
            self.run_chunk_from_mel(&mel, n_frames, main_start, CHUNK_MEL_FRAMES)?;
            self.processed_mel_frames += CHUNK_MEL_FRAMES;
        }
        Ok(())
    }

    /// Build `[1, 65, 128]` features and run encoder + RNNT greedy decode.
    fn run_chunk_from_mel(
        &mut self,
        mel: &[f32],
        n_frames: usize,
        main_start: usize,
        main_len: usize,
    ) -> Result<()> {
        debug_assert_eq!(mel.len(), n_frames * N_MELS);
        let mut feat = Array2::<f32>::zeros((ENCODER_MEL_FRAMES, N_MELS));

        // Pre-encode cache: previous PRE_ENCODE_CACHE frames (zeros on first chunk).
        if main_start > 0 {
            let cache_start = main_start.saturating_sub(PRE_ENCODE_CACHE);
            let cache_frames = main_start - cache_start;
            let offset = PRE_ENCODE_CACHE - cache_frames;
            for f in 0..cache_frames {
                let src = (cache_start + f) * N_MELS;
                for m in 0..N_MELS {
                    feat[[offset + f, m]] = mel[src + m];
                }
            }
        }

        // New frames at PRE_ENCODE_CACHE offset
        for f in 0..main_len.min(CHUNK_MEL_FRAMES) {
            let src = (main_start + f) * N_MELS;
            if main_start + f >= n_frames {
                break;
            }
            for m in 0..N_MELS {
                feat[[PRE_ENCODE_CACHE + f, m]] = mel[src + m];
            }
        }

        let length = (PRE_ENCODE_CACHE + main_len.min(CHUNK_MEL_FRAMES)) as i64;
        self.run_encoder_decode(feat, length)
    }

    fn run_encoder_decode(&mut self, feat: Array2<f32>, length: i64) -> Result<()> {
        // audio_signal: [1, 65, 128]
        let audio_signal = feat
            .to_shape((1, ENCODER_MEL_FRAMES, N_MELS))
            .context("reshape audio_signal")?
            .to_owned();
        let length_arr = Array1::from_vec(vec![length]);
        let lang = Array1::from_vec(vec![self.lang_id]);

        let audio_t = Tensor::from_array(audio_signal).context("audio_signal tensor")?;
        let length_t = Tensor::from_array(length_arr).context("length tensor")?;
        let cache_ch_t = Tensor::from_array(self.cache_last_channel.clone())
            .context("cache_last_channel tensor")?;
        let cache_t_t =
            Tensor::from_array(self.cache_last_time.clone()).context("cache_last_time tensor")?;
        let cache_len_t = Tensor::from_array(self.cache_last_channel_len.clone())
            .context("cache_last_channel_len tensor")?;
        let lang_t = Tensor::from_array(lang).context("lang_id tensor")?;

        let (enc_out, enc_len) = {
            let outputs = self
                .encoder
                .run(ort::inputs![
                    "audio_signal" => audio_t,
                    "length" => length_t,
                    "cache_last_channel" => cache_ch_t,
                    "cache_last_time" => cache_t_t,
                    "cache_last_channel_len" => cache_len_t,
                    "lang_id" => lang_t,
                ])
                .context("encoder run")?;

            let enc_out = extract_3d_f32(&outputs["outputs"]).context("encoder outputs")?;
            let enc_len =
                extract_scalar_i64(&outputs["encoded_lengths"]).context("encoded_lengths")?;
            self.cache_last_channel =
                extract_4d_f32(&outputs["cache_last_channel_next"]).context("cache_ch_next")?;
            self.cache_last_time =
                extract_4d_f32(&outputs["cache_last_time_next"]).context("cache_t_next")?;
            self.cache_last_channel_len = extract_1d_i64(&outputs["cache_last_channel_len_next"])
                .context("cache_len_next")?;
            (enc_out, enc_len)
        };

        let t_frames = (enc_len as usize).min(enc_out.shape()[1]);
        self.greedy_decode(&enc_out, t_frames)?;
        Ok(())
    }

    fn greedy_decode(&mut self, enc_out: &Array3<f32>, t_frames: usize) -> Result<()> {
        let blank = self.vocab.blank_id();
        for t in 0..t_frames {
            for _ in 0..MAX_SYMBOLS {
                let targets = Array2::from_shape_vec((1, 1), vec![self.last_token])
                    .context("targets shape")?;
                let targets_t = Tensor::from_array(targets).context("targets tensor")?;
                let h_t = Tensor::from_array(self.h.clone()).context("h_in tensor")?;
                let c_t = Tensor::from_array(self.c.clone()).context("c_in tensor")?;

                let dec_outs = self
                    .decoder
                    .run(ort::inputs![
                        "targets" => targets_t,
                        "h_in" => h_t,
                        "c_in" => c_t,
                    ])
                    .context("decoder run")?;

                let dec_out =
                    extract_3d_f32(&dec_outs["decoder_output"]).context("decoder_output")?;
                // decoder_output: [batch, 640, target_len] → joint wants [batch, target_len, 640]
                let dec_for_joint = dec_out
                    .permuted_axes([0, 2, 1])
                    .as_standard_layout()
                    .to_owned();
                let h_new = extract_3d_f32(&dec_outs["h_out"]).context("h_out")?;
                let c_new = extract_3d_f32(&dec_outs["c_out"]).context("c_out")?;

                let enc_frame = enc_out.slice(s![.., t..t + 1, ..]).to_owned();
                let enc_t = Tensor::from_array(enc_frame).context("encoder_output tensor")?;
                let dec_t = Tensor::from_array(dec_for_joint).context("decoder_output tensor")?;

                let jnt_outs = self
                    .joint
                    .run(ort::inputs![
                        "encoder_output" => enc_t,
                        "decoder_output" => dec_t,
                    ])
                    .context("joint run")?;

                let (_, logits) = jnt_outs["joint_output"]
                    .try_extract_tensor::<f32>()
                    .map_err(|e| anyhow::anyhow!("joint logits: {e}"))?;
                let (pred, _) = argmax_f32(logits.iter().copied());
                if pred == blank {
                    break;
                }
                self.tokens.push(pred);
                self.last_token = pred as i64;
                self.h = h_new;
                self.c = c_new;
            }
        }
        Ok(())
    }
}

fn extract_3d_f32(value: &ort::value::DynValue) -> Result<Array3<f32>> {
    let (shape, data) = value
        .try_extract_tensor::<f32>()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let dims = shape.as_ref();
    if dims.len() != 3 {
        bail!("expected 3D tensor, got {dims:?}");
    }
    Array3::from_shape_vec(
        (dims[0] as usize, dims[1] as usize, dims[2] as usize),
        data.to_vec(),
    )
    .context("reshape 3d")
}

fn extract_4d_f32(value: &ort::value::DynValue) -> Result<Array4<f32>> {
    let (shape, data) = value
        .try_extract_tensor::<f32>()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let dims = shape.as_ref();
    if dims.len() != 4 {
        bail!("expected 4D tensor, got {dims:?}");
    }
    Array4::from_shape_vec(
        (
            dims[0] as usize,
            dims[1] as usize,
            dims[2] as usize,
            dims[3] as usize,
        ),
        data.to_vec(),
    )
    .context("reshape 4d")
}

fn extract_1d_i64(value: &ort::value::DynValue) -> Result<Array1<i64>> {
    let (_shape, data) = value
        .try_extract_tensor::<i64>()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(Array1::from_vec(data.to_vec()))
}

fn extract_scalar_i64(value: &ort::value::DynValue) -> Result<i64> {
    let (_shape, data) = value
        .try_extract_tensor::<i64>()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    data.first().copied().context("empty i64 tensor")
}

fn argmax_f32(values: impl IntoIterator<Item = f32>) -> (usize, f32) {
    let mut max_idx = 0usize;
    let mut max_val = f32::NEG_INFINITY;
    for (i, v) in values.into_iter().enumerate() {
        if v > max_val {
            max_val = v;
            max_idx = i;
        }
    }
    (max_idx, max_val)
}
