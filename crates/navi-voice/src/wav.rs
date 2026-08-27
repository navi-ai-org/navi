//! Minimal WAV loading helpers (16-bit/float PCM → mono f32).

use std::path::Path;

use anyhow::{Context, Result, bail};
use hound::{SampleFormat, WavReader};

/// Load a WAV file as mono f32 samples and return (samples, sample_rate).
pub fn load_wav_mono_f32(path: &Path) -> Result<(Vec<f32>, u32)> {
    let mut reader =
        WavReader::open(path).with_context(|| format!("open wav {}", path.display()))?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let sample_rate = spec.sample_rate;

    let interleaved: Vec<f32> = match spec.sample_format {
        SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("read float wav samples")?,
        SampleFormat::Int => {
            let bits = spec.bits_per_sample;
            if bits <= 16 {
                reader
                    .samples::<i16>()
                    .map(|s| s.map(|v| v as f32 / 32768.0))
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .context("read i16 wav samples")?
            } else {
                reader
                    .samples::<i32>()
                    .map(|s| {
                        s.map(|v| {
                            let max = (1i64 << (bits.min(31) - 1)) as f32;
                            v as f32 / max
                        })
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .context("read i32 wav samples")?
            }
        }
    };

    if channels == 1 {
        return Ok((interleaved, sample_rate));
    }

    let mut mono = Vec::with_capacity(interleaved.len() / channels);
    for frame in interleaved.chunks(channels) {
        let sum: f32 = frame.iter().sum();
        mono.push(sum / channels as f32);
    }
    Ok((mono, sample_rate))
}

/// Resample mono audio to target rate with linear interpolation (good enough for 16 kHz ASR).
pub fn resample_linear(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == 0 || to_rate == 0 || samples.is_empty() || from_rate == to_rate {
        return samples.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = ((samples.len() as f64) / ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = i as f64 * ratio;
        let i0 = src.floor() as usize;
        let i1 = (i0 + 1).min(samples.len() - 1);
        let frac = (src - i0 as f64) as f32;
        let v = samples[i0] * (1.0 - frac) + samples[i1] * frac;
        out.push(v);
    }
    out
}

/// Load WAV and ensure 16 kHz mono f32.
pub fn load_wav_16k_mono(path: &Path) -> Result<Vec<f32>> {
    let (samples, sr) = load_wav_mono_f32(path)?;
    if sr == 0 {
        bail!("wav has sample_rate 0: {}", path.display());
    }
    if sr == 16_000 {
        return Ok(samples);
    }
    Ok(resample_linear(&samples, sr, 16_000))
}

/// Encode mono f32 samples as a 16 kHz 16-bit PCM WAV in memory.
pub fn write_wav_16k_mono_bytes(samples: &[f32]) -> Result<Vec<u8>> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(&mut cursor, spec).context("create wav writer")?;
        for &s in samples {
            let clamped = s.clamp(-1.0, 1.0);
            let i = (clamped * 32767.0).round() as i16;
            writer.write_sample(i).context("write wav sample")?;
        }
        writer.finalize().context("finalize wav")?;
    }
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_16k_silence_header() {
        let samples = vec![0.0f32; 1600];
        let bytes = write_wav_16k_mono_bytes(&samples).expect("encode");
        assert!(bytes.len() > 44);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
    }
}
