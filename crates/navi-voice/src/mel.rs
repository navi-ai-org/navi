//! NeMo-compatible log-mel features for Nemotron streaming ASR.
//!
//! Matches the constants and pipeline used by NeMo / parakeet-rs:
//! preemphasis → center-padded STFT (power) → Slaney mel filterbank → ln(x + ε).
//! No per-feature normalization.

use std::f32::consts::PI;
use std::sync::Arc;

use realfft::RealToComplex;

/// 16 kHz mono PCM expected by Nemotron streaming.
pub const SAMPLE_RATE: u32 = 16_000;
pub const N_FFT: usize = 512;
pub const WIN_LENGTH: usize = 400;
pub const HOP_LENGTH: usize = 160;
pub const N_MELS: usize = 128;
pub const PREEMPH: f32 = 0.97;
/// genai_config / NeMo log guard (≈ 2^-24).
pub const LOG_ZERO_GUARD: f32 = 5.960_464_5e-8;
/// Mel frames per streaming chunk (560 ms at hop 160).
pub const CHUNK_MEL_FRAMES: usize = 56;
/// Pre-encode cache frames prepended to each encoder input.
pub const PRE_ENCODE_CACHE: usize = 9;
/// Encoder input frames: PRE_ENCODE_CACHE + CHUNK_MEL_FRAMES.
pub const ENCODER_MEL_FRAMES: usize = PRE_ENCODE_CACHE + CHUNK_MEL_FRAMES; // 65
/// PCM samples per streaming chunk (0.56 s @ 16 kHz).
pub const CHUNK_SAMPLES: usize = CHUNK_MEL_FRAMES * HOP_LENGTH; // 8960

/// Reusable mel filterbank + FFT plan.
pub struct MelFrontend {
    mel_basis: Vec<f32>, // row-major [n_mels, freq_bins]
    freq_bins: usize,
    fft_plan: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
}

impl MelFrontend {
    pub fn new() -> Self {
        let freq_bins = N_FFT / 2 + 1;
        let mel_basis = create_mel_filterbank(N_FFT, N_MELS, SAMPLE_RATE as usize);
        let mut planner = realfft::RealFftPlanner::<f32>::new();
        let fft_plan = planner.plan_fft_forward(N_FFT);
        let window = hann_window(WIN_LENGTH);
        Self {
            mel_basis,
            freq_bins,
            fft_plan,
            window,
        }
    }

    /// Log-mel spectrogram as row-major `[n_frames * n_mels]` with layout
    /// frame-major: for each frame, `n_mels` coeffs (matches ONNX `[1, T, 128]`).
    ///
    /// Also returns `n_frames`.
    pub fn log_mel_frame_major(&self, audio: &[f32]) -> (Vec<f32>, usize) {
        if audio.is_empty() {
            return (Vec::new(), 0);
        }
        let pre = apply_preemphasis(audio, PREEMPH);
        let power = self.stft_power(&pre); // [freq_bins * n_frames], freq-major columns
        let n_frames = power.len() / self.freq_bins;
        let mut out = vec![0.0f32; n_frames * N_MELS];
        for t in 0..n_frames {
            for m in 0..N_MELS {
                let mut acc = 0.0f32;
                let mel_row = m * self.freq_bins;
                let power_col = t; // we'll index power as [k * n_frames + t] if col-major
                // power stored as [freq_bins, n_frames] row-major: power[k * n_frames + t]
                for k in 0..self.freq_bins {
                    acc += self.mel_basis[mel_row + k] * power[k * n_frames + power_col];
                }
                out[t * N_MELS + m] = (acc + LOG_ZERO_GUARD).ln();
            }
        }
        (out, n_frames)
    }

    fn stft_power(&self, audio: &[f32]) -> Vec<f32> {
        let pad = N_FFT / 2;
        let mut padded = vec![0.0f32; pad];
        padded.extend_from_slice(audio);
        padded.resize(padded.len() + pad, 0.0);

        let n_frames = (padded.len() - N_FFT) / HOP_LENGTH + 1;
        let mut spectrogram = vec![0.0f32; self.freq_bins * n_frames];

        let mut input = vec![0.0f32; N_FFT];
        let mut output = self.fft_plan.make_output_vec();
        let mut scratch = self.fft_plan.make_scratch_vec();

        for frame_idx in 0..n_frames {
            let start = frame_idx * HOP_LENGTH;
            input.fill(0.0);
            let available = (padded.len() - start).min(WIN_LENGTH);
            for i in 0..available {
                input[i] = padded[start + i] * self.window[i];
            }
            // realfft may require exclusive ownership of input buffer
            let mut frame = input.clone();
            self.fft_plan
                .process_with_scratch(&mut frame, &mut output, &mut scratch)
                .expect("FFT plan size matches N_FFT");
            for k in 0..self.freq_bins {
                let re = output[k].re;
                let im = output[k].im;
                spectrogram[k * n_frames + frame_idx] = re * re + im * im;
            }
        }
        spectrogram
    }
}

impl Default for MelFrontend {
    fn default() -> Self {
        Self::new()
    }
}

pub fn apply_preemphasis(audio: &[f32], coef: f32) -> Vec<f32> {
    if audio.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(audio.len());
    out.push(audio[0]);
    for i in 1..audio.len() {
        out.push(audio[i] - coef * audio[i - 1]);
    }
    out
}

fn hann_window(window_length: usize) -> Vec<f32> {
    (0..window_length)
        .map(|i| 0.5 - 0.5 * ((2.0 * PI * i as f32) / (window_length as f32 - 1.0)).cos())
        .collect()
}

// Slaney mel scale (librosa-compatible).
const F_SP: f64 = 200.0 / 3.0;
const MIN_LOG_HZ: f64 = 1000.0;
const MIN_LOG_MEL: f64 = MIN_LOG_HZ / F_SP;
const LOG_STEP: f64 = 0.068_751_777_420_949_12;

fn hz_to_mel_slaney(hz: f64) -> f64 {
    if hz < MIN_LOG_HZ {
        hz / F_SP
    } else {
        MIN_LOG_MEL + (hz / MIN_LOG_HZ).ln() / LOG_STEP
    }
}

fn mel_to_hz_slaney(mel: f64) -> f64 {
    if mel < MIN_LOG_MEL {
        mel * F_SP
    } else {
        MIN_LOG_HZ * ((mel - MIN_LOG_MEL) * LOG_STEP).exp()
    }
}

/// Row-major `[n_mels, freq_bins]` Slaney-normalized mel filterbank.
pub fn create_mel_filterbank(n_fft: usize, n_mels: usize, sample_rate: usize) -> Vec<f32> {
    let freq_bins = n_fft / 2 + 1;
    let mut filterbank = vec![0.0f32; n_mels * freq_bins];

    let fmax = sample_rate as f64 / 2.0;
    let mel_min = hz_to_mel_slaney(0.0);
    let mel_max = hz_to_mel_slaney(fmax);

    let mel_points: Vec<f64> = (0..=n_mels + 1)
        .map(|i| mel_to_hz_slaney(mel_min + (mel_max - mel_min) * i as f64 / (n_mels + 1) as f64))
        .collect();

    let fft_freqs: Vec<f64> = (0..freq_bins)
        .map(|i| i as f64 * sample_rate as f64 / n_fft as f64)
        .collect();

    let fdiff: Vec<f64> = mel_points.windows(2).map(|w| w[1] - w[0]).collect();

    for i in 0..n_mels {
        for (k, &freq) in fft_freqs.iter().enumerate() {
            let lower = (freq - mel_points[i]) / fdiff[i];
            let upper = (mel_points[i + 2] - freq) / fdiff[i + 1];
            filterbank[i * freq_bins + k] = 0.0f64.max(lower.min(upper)) as f32;
        }
        let enorm = 2.0 / (mel_points[i + 2] - mel_points[i]);
        for k in 0..freq_bins {
            filterbank[i * freq_bins + k] *= enorm as f32;
        }
    }

    filterbank
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_constants_match_genai() {
        assert_eq!(CHUNK_SAMPLES, 8960);
        assert_eq!(ENCODER_MEL_FRAMES, 65);
        assert_eq!(CHUNK_MEL_FRAMES * HOP_LENGTH, CHUNK_SAMPLES);
    }

    #[test]
    fn preemphasis_first_sample_unchanged() {
        let x = [0.5, 0.4, 0.3];
        let y = apply_preemphasis(&x, 0.97);
        assert!((y[0] - 0.5).abs() < 1e-6);
        assert!((y[1] - (0.4 - 0.97 * 0.5)).abs() < 1e-6);
    }

    #[test]
    fn mel_on_sine_has_expected_shape() {
        let fe = MelFrontend::new();
        let sr = SAMPLE_RATE as usize;
        let audio: Vec<f32> = (0..sr)
            .map(|i| (2.0 * PI * 440.0 * i as f32 / sr as f32).sin() * 0.3)
            .collect();
        let (mel, n_frames) = fe.log_mel_frame_major(&audio);
        assert_eq!(mel.len(), n_frames * N_MELS);
        assert!(n_frames > 50);
        // Finite log-mels
        assert!(mel.iter().all(|v| v.is_finite()));
    }
}
