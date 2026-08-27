//! Download NAVI voice model packages from Hugging Face.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::paths::{VoicePaths, default_hf_repo};
use crate::types::{AsrEngineId, VoiceInstallOptions, VoiceManifest};

/// Progress callback: (downloaded_bytes, total_bytes_if_known, current_file).
pub type DownloadProgress = Box<dyn Fn(u64, Option<u64>, &str) + Send + Sync>;

/// True when manifest + checksums exist for the engine.
pub fn engine_installed(
    data_dir: &Path,
    options: &VoiceInstallOptions,
    engine: AsrEngineId,
) -> bool {
    VoicePaths::resolve(data_dir, options, engine).is_installed()
}

/// Download (or re-download) a voice engine package into `{data_dir}/voice/models/…`.
pub async fn download_engine(
    data_dir: &Path,
    options: &VoiceInstallOptions,
    engine: AsrEngineId,
    force: bool,
    progress: Option<DownloadProgress>,
) -> Result<PathBuf> {
    match engine {
        AsrEngineId::NemotronStreaming => {
            download_nemotron(data_dir, options, force, progress).await
        }
        AsrEngineId::DistilWhisper => {
            bail!(
                "distil_whisper packaging is not ready yet. Use --engine nemotron_streaming for now."
            )
        }
    }
}

async fn download_nemotron(
    data_dir: &Path,
    options: &VoiceInstallOptions,
    force: bool,
    progress: Option<DownloadProgress>,
) -> Result<PathBuf> {
    let paths = VoicePaths::resolve(data_dir, options, AsrEngineId::NemotronStreaming);
    fs::create_dir_all(&paths.engine_dir)
        .with_context(|| format!("create model dir {}", paths.engine_dir.display()))?;

    if paths.is_installed() && !force {
        // Still verify checksums when present.
        verify_checksums(&paths.engine_dir, &paths.checksums)?;
        return Ok(paths.engine_dir);
    }

    if force && paths.engine_dir.exists() {
        // Remove previous install (keep parent models/ root).
        for entry in fs::read_dir(&paths.engine_dir)
            .with_context(|| format!("read model dir {}", paths.engine_dir.display()))?
        {
            let entry = entry.with_context(|| {
                format!("read entry in model dir {}", paths.engine_dir.display())
            })?;
            let p = entry.path();
            if p.is_dir() {
                fs::remove_dir_all(&p).with_context(|| format!("remove dir {}", p.display()))?;
            } else {
                fs::remove_file(&p).with_context(|| format!("remove file {}", p.display()))?;
            }
        }
    }

    let repo = default_hf_repo(options, AsrEngineId::NemotronStreaming);
    let client = http_client()?;

    // 1) SHA256SUMS defines the file set.
    let checksums_text = download_text(
        &client,
        &repo,
        "SHA256SUMS",
        &paths.checksums,
        progress.as_ref(),
    )
    .await?;
    let entries = parse_sha256sums(&checksums_text)?;

    // 2) Download each file listed in checksums.
    for (expected_hash, rel) in &entries {
        let dest = paths.engine_dir.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        if dest.is_file() && !force {
            let actual = file_sha256(&dest)?;
            if actual.eq_ignore_ascii_case(expected_hash) {
                continue;
            }
        }
        download_file(&client, &repo, rel, &dest, progress.as_ref()).await?;
    }

    verify_checksums(&paths.engine_dir, &paths.checksums)?;

    // Ensure manifest is present (also in checksums).
    if !paths.manifest.is_file() {
        bail!("navi-manifest.json missing after download from {}", repo);
    }
    let _manifest: VoiceManifest = serde_json::from_str(&fs::read_to_string(&paths.manifest)?)
        .context("parse navi-manifest.json")?;

    Ok(paths.engine_dir)
}

fn http_client() -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(600));
    // Optional HF token for gated/rate-limited downloads.
    if let Ok(token) =
        std::env::var("HF_TOKEN").or_else(|_| std::env::var("HUGGING_FACE_HUB_TOKEN"))
        && !token.trim().is_empty()
    {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", token.trim())
                .parse()
                .context("invalid HF token header")?,
        );
        builder = builder.default_headers(headers);
    }
    builder.build().context("build HTTP client")
}

fn resolve_url(repo: &str, relative_path: &str) -> String {
    // HF resolve URL encodes path segments; simple join for normal paths.
    let rel = relative_path.trim_start_matches("./");
    format!(
        "https://huggingface.co/{}/resolve/main/{}",
        repo,
        rel.split('/')
            .map(urlencoding_segment)
            .collect::<Vec<_>>()
            .join("/")
    )
}

fn urlencoding_segment(s: &str) -> String {
    // Minimal encode for spaces etc.; paths we ship are safe.
    s.replace(' ', "%20")
}

async fn download_text(
    client: &reqwest::Client,
    repo: &str,
    relative: &str,
    dest: &Path,
    progress: Option<&DownloadProgress>,
) -> Result<String> {
    download_file(client, repo, relative, dest, progress).await?;
    fs::read_to_string(dest).with_context(|| format!("read {}", dest.display()))
}

async fn download_file(
    client: &reqwest::Client,
    repo: &str,
    relative: &str,
    dest: &Path,
    progress: Option<&DownloadProgress>,
) -> Result<()> {
    use futures_util::StreamExt;

    let url = resolve_url(repo, relative);
    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if !response.status().is_success() {
        bail!("download failed: HTTP {} for {url}", response.status());
    }
    let total = response.content_length();
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    let mut file = fs::File::create(dest).with_context(|| format!("create {}", dest.display()))?;
    let mut stream = response.bytes_stream();
    let mut downloaded = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read download chunk")?;
        file.write_all(&chunk)
            .with_context(|| format!("write download chunk to {}", dest.display()))?;
        downloaded += chunk.len() as u64;
        if let Some(cb) = progress {
            cb(downloaded, total, relative);
        }
    }
    Ok(())
}

fn parse_sha256sums(text: &str) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // "hash  path" or "hash *path"
        let mut parts = line.split_whitespace();
        let hash = parts
            .next()
            .with_context(|| format!("bad SHA256SUMS line: {line}"))?
            .to_string();
        let path = parts
            .next()
            .with_context(|| format!("bad SHA256SUMS line: {line}"))?
            .trim_start_matches('*')
            .trim_start_matches("./")
            .to_string();
        if hash.len() != 64 {
            bail!("invalid hash length in SHA256SUMS: {hash}");
        }
        out.push((hash, path));
    }
    if out.is_empty() {
        bail!("SHA256SUMS is empty");
    }
    Ok(out)
}

fn file_sha256(path: &Path) -> Result<String> {
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    Ok(hex::encode(hasher.finalize()))
}

fn verify_checksums(engine_dir: &Path, checksums_path: &Path) -> Result<()> {
    let text = fs::read_to_string(checksums_path)
        .with_context(|| format!("read {}", checksums_path.display()))?;
    let entries = parse_sha256sums(&text)?;
    for (expected, rel) in entries {
        let path = engine_dir.join(&rel);
        if !path.is_file() {
            bail!("missing file listed in SHA256SUMS: {rel}");
        }
        let actual = file_sha256(&path)?;
        if !actual.eq_ignore_ascii_case(&expected) {
            bail!("checksum mismatch for {rel}: expected {expected}, got {actual}");
        }
    }
    Ok(())
}

/// Public helper for doctor.
pub fn verify_engine_checksums(
    data_dir: &Path,
    options: &VoiceInstallOptions,
    engine: AsrEngineId,
) -> Result<()> {
    let paths = VoicePaths::resolve(data_dir, options, engine);
    if !paths.checksums.is_file() {
        bail!("no SHA256SUMS at {}", paths.checksums.display());
    }
    verify_checksums(&paths.engine_dir, &paths.checksums)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::resolve_model_dir;

    #[test]
    fn parse_sha256sums_basic() {
        let text = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  ./foo.txt
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  bar.bin
";
        let entries = parse_sha256sums(text).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].1, "foo.txt");
        assert_eq!(entries[1].1, "bar.bin");
    }

    #[test]
    fn resolve_model_dir_default() {
        let opts = VoiceInstallOptions::default();
        let dir = resolve_model_dir(
            Path::new("/tmp/navi-data"),
            &opts,
            AsrEngineId::NemotronStreaming,
        );
        assert!(dir.ends_with("voice/models/nemotron-3.5-asr-streaming-0.6b-onnx"));
    }
}
