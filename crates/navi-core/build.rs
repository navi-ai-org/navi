//! Build script for navi-core.
//!
//! Fetches the provider registry snapshot from `navi-ai-org/navi-registry` and
//! embeds it into the compiled binary so NAVI works offline and has a fallback
//! when the SQLite cache and remote fetch both fail.
//!
//! Sources (in order):
//!   1. `NAVI_REGISTRY_DIR` (offline/local override)
//!   2. `NAVI_REGISTRY_REF` (branch, tag, or full commit; releases use `refs/heads/main`)
//!   3. `registry.lock` pinned commit (default for dev/CI builds)

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const REGISTRY_ORG: &str = "navi-ai-org";
const REGISTRY_REPO: &str = "navi-registry";
const REGISTRY_LOCK_FILE: &str = "registry.lock";
const MAX_TARBALL_SIZE: u64 = 50 * 1024 * 1024; // 50 MiB

fn main() -> Result<()> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").context("OUT_DIR not set")?);
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR not set")?);
    let lock_file = manifest_dir.join(REGISTRY_LOCK_FILE);

    println!("cargo:rerun-if-changed={}", lock_file.display());
    println!("cargo:rerun-if-env-changed=NAVI_REGISTRY_DIR");
    println!("cargo:rerun-if-env-changed=NAVI_OFFLINE");
    println!("cargo:rerun-if-env-changed=NAVI_REGISTRY_REF");

    let embedded_dir = out_dir.join("embedded_registry");
    fs::create_dir_all(&embedded_dir).context("failed to create embedded_registry output dir")?;

    // Resolve the snapshot source: local override, NAVI_REGISTRY_REF, or lock.
    let snapshot_dir = resolve_snapshot_dir(&manifest_dir, &lock_file, &out_dir)?;
    println!(
        "cargo:rerun-if-changed={}",
        snapshot_dir.join("manifest.json").display()
    );

    // Copy manifest.json into the embedded dir so include_str!("manifest.json") works.
    let manifest_src = snapshot_dir.join("manifest.json");
    let manifest_dst = embedded_dir.join("manifest.json");
    fs::copy(&manifest_src, &manifest_dst).with_context(|| {
        format!(
            "failed to copy embedded manifest from {}",
            manifest_src.display()
        )
    })?;

    let providers_dir = snapshot_dir.join("providers");
    let transcription_providers_dir = snapshot_dir.join("transcription-providers");
    let models_dir = snapshot_dir.join("models");
    let bases_dir = snapshot_dir.join("bases");
    let schema_src = snapshot_dir.join("schemas");

    // Copy and embed LLM provider files, sorted for deterministic output.
    let embedded_providers_dir = embedded_dir.join("providers");
    fs::create_dir_all(&embedded_providers_dir)
        .context("failed to create embedded providers dir")?;
    let provider_files =
        collect_json_files(&providers_dir).context("failed to read providers directory")?;
    let mut entries = Vec::with_capacity(provider_files.len());
    for path in &provider_files {
        let id = file_stem(path).context("provider file has invalid name")?;
        let dst = embedded_providers_dir.join(format!("{id}.json"));
        fs::copy(path, &dst).with_context(|| format!("failed to copy embedded provider {id}"))?;
        entries.push((id.to_string(), embedded_path(&dst, &embedded_dir)?));
    }

    // Copy and embed transcription / dictation provider files.
    let embedded_transcription_dir = embedded_dir.join("transcription-providers");
    fs::create_dir_all(&embedded_transcription_dir)
        .context("failed to create embedded transcription-providers dir")?;
    let mut transcription_entries = Vec::new();
    if transcription_providers_dir.is_dir() {
        let files = collect_json_files(&transcription_providers_dir)
            .context("failed to read transcription-providers directory")?;
        for path in &files {
            let id = file_stem(path).context("transcription provider file has invalid name")?;
            let dst = embedded_transcription_dir.join(format!("{id}.json"));
            fs::copy(path, &dst)
                .with_context(|| format!("failed to copy embedded transcription provider {id}"))?;
            transcription_entries.push((id.to_string(), embedded_path(&dst, &embedded_dir)?));
        }
    }

    // Copy and embed canonical model catalog files.
    let embedded_models_dir = embedded_dir.join("models");
    fs::create_dir_all(&embedded_models_dir).context("failed to create embedded models dir")?;
    let model_files = if models_dir.is_dir() {
        collect_json_files(&models_dir).context("failed to read models directory")?
    } else {
        Vec::new()
    };
    let mut model_catalog_entries = Vec::with_capacity(model_files.len());
    for path in &model_files {
        // Filenames must stay Windows-safe (no `:`). Model ids may contain
        // `:` (Ollama tags like `gemma3:12b`); encode as `__` on disk and
        // restore when embedding the catalog id.
        let stem = path
            .file_stem()
            .expect("model file has no stem")
            .to_str()
            .context("model file name is not valid UTF-8")?;
        let safe_stem = stem.replace(':', "__");
        let id = safe_stem.replace("__", ":");
        let safe_name = format!("{safe_stem}.json");
        let dst = embedded_models_dir.join(&safe_name);
        fs::copy(path, &dst)
            .with_context(|| format!("failed to copy embedded canonical model {id}"))?;
        model_catalog_entries.push((id, embedded_path(&dst, &embedded_dir)?));
    }

    // Copy and embed provider base definitions (for `extends`).
    let embedded_bases_dir = embedded_dir.join("bases");
    fs::create_dir_all(&embedded_bases_dir).context("failed to create embedded bases dir")?;
    let mut base_entries = Vec::new();
    if bases_dir.is_dir() {
        let base_files =
            collect_json_files(&bases_dir).context("failed to read bases directory")?;
        for path in &base_files {
            let id = file_stem(path).context("base file has invalid name")?;
            let dst = embedded_bases_dir.join(format!("{id}.json"));
            fs::copy(path, &dst).with_context(|| format!("failed to copy embedded base {id}"))?;
            base_entries.push((id.to_string(), embedded_path(&dst, &embedded_dir)?));
        }
    }

    // Copy schema files.
    if schema_src.is_dir() {
        let embedded_schema_dir = embedded_dir.join("schemas");
        fs::create_dir_all(&embedded_schema_dir)
            .context("failed to create embedded schemas dir")?;
        for entry in fs::read_dir(&schema_src)
            .with_context(|| format!("failed to read schemas directory {}", schema_src.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                let dst = embedded_schema_dir.join(path.file_name().unwrap());
                fs::copy(&path, &dst)
                    .with_context(|| format!("failed to copy schema {}", path.display()))?;
            }
        }
    }

    // Generate the Rust source that includes the embedded files.
    let mut src = String::new();
    src.push_str("// Auto-generated by build.rs. Do not edit.\n");
    src.push_str("// Embeds the registry snapshot into the binary.\n\n");
    src.push_str("pub const MANIFEST_JSON: &str = include_str!(\"manifest.json\");\n\n");
    src.push_str("pub const PROVIDER_FILES: &[(&str, &str)] = &[\n");
    for (id, path) in &entries {
        src.push_str(&format!("    ({id:?}, include_str!({path:?})),\n"));
    }
    src.push_str("];\n\n");
    src.push_str("pub const TRANSCRIPTION_PROVIDER_FILES: &[(&str, &str)] = &[\n");
    for (id, path) in &transcription_entries {
        src.push_str(&format!("    ({id:?}, include_str!({path:?})),\n"));
    }
    src.push_str("];\n\n");
    src.push_str("pub const BASE_FILES: &[(&str, &str)] = &[\n");
    for (id, path) in &base_entries {
        src.push_str(&format!("    ({id:?}, include_str!({path:?})),\n"));
    }
    src.push_str("];\n\n");
    src.push_str("pub const MODEL_CATALOG_FILES: &[(&str, &str)] = &[\n");
    for (id, path) in &model_catalog_entries {
        src.push_str(&format!("    ({id:?}, include_str!({path:?})),\n"));
    }
    src.push_str("];\n");

    let out_file = embedded_dir.join("embedded.rs");
    fs::write(&out_file, src).context("failed to write embedded.rs")?;

    // Tell cargo to look in the embedded dir for include_str! paths.
    println!(
        "cargo:rustc-env=NAVI_EMBEDDED_REGISTRY_DIR={}",
        embedded_dir.display()
    );

    Ok(())
}

fn resolve_snapshot_dir(_manifest_dir: &Path, lock_file: &Path, out_dir: &Path) -> Result<PathBuf> {
    if let Ok(local_dir) = env::var("NAVI_REGISTRY_DIR") {
        let local_path = PathBuf::from(local_dir);
        if !local_path.is_dir() {
            anyhow::bail!(
                "NAVI_REGISTRY_DIR '{}' is not a directory",
                local_path.display()
            );
        }
        if !local_path.join("manifest.json").is_file() {
            anyhow::bail!(
                "NAVI_REGISTRY_DIR '{}' does not contain manifest.json",
                local_path.display()
            );
        }
        return Ok(local_path);
    }

    if env::var_os("NAVI_OFFLINE").is_some() {
        anyhow::bail!(
            "NAVI_OFFLINE is set but NAVI_REGISTRY_DIR is not; \
             provide a local registry directory to build offline"
        );
    }

    // Prefer an explicit ref (branch, tag, or full commit). Releases use
    // `refs/heads/main` so the binary always ships with the latest snapshot.
    if let Ok(registry_ref) = env::var("NAVI_REGISTRY_REF") {
        if registry_ref.is_empty() {
            anyhow::bail!("NAVI_REGISTRY_REF is empty");
        }
        // Cache only when the ref is an unambiguous full git SHA.
        let use_cache = is_full_git_sha(&registry_ref);
        return fetch_and_extract_registry(&registry_ref, out_dir, use_cache);
    }

    let lock_text = fs::read_to_string(lock_file).with_context(|| {
        format!(
            "failed to read {}. Set NAVI_REGISTRY_DIR or NAVI_REGISTRY_REF to build.",
            lock_file.display()
        )
    })?;
    let commit = lock_text
        .lines()
        .map(|l| l.split_once('#').map_or(l, |(before, _)| before).trim())
        .find(|l| !l.is_empty())
        .context("registry.lock is empty")?;
    if commit.len() < 12 {
        anyhow::bail!("registry.lock does not contain a valid commit hash");
    }

    fetch_and_extract_registry(commit, out_dir, true)
}

fn fetch_and_extract_registry(
    registry_ref: &str,
    out_dir: &Path,
    use_cache: bool,
) -> Result<PathBuf> {
    let (work_dir, tar_path, extract_dir) = if use_cache {
        let cache_dir = registry_cache_dir(registry_ref)?;
        (
            cache_dir.clone(),
            cache_dir.join("registry.tar.gz"),
            cache_dir.join("extracted"),
        )
    } else {
        // Mutable refs (e.g. refs/heads/main) must not be cached. Use a
        // per-build directory inside OUT_DIR and always re-fetch.
        let safe_ref = registry_ref.replace(['/', ':', '\\'], "_");
        let work_dir = out_dir.join(format!("registry-ref-{safe_ref}"));
        if work_dir.is_dir() {
            fs::remove_dir_all(&work_dir).with_context(|| {
                format!(
                    "failed to remove stale registry work directory {}",
                    work_dir.display()
                )
            })?;
        }
        (
            work_dir.clone(),
            work_dir.join("registry.tar.gz"),
            work_dir.join("extracted"),
        )
    };

    // Find an already-extracted source tree (only valid for pinned/cached refs).
    if use_cache && let Some(source) = find_registry_source(&extract_dir)? {
        return Ok(source);
    }

    // Download the tarball.
    fs::create_dir_all(&work_dir).with_context(|| {
        format!(
            "failed to create registry work directory {}",
            work_dir.display()
        )
    })?;

    let url =
        format!("https://codeload.github.com/{REGISTRY_ORG}/{REGISTRY_REPO}/tar.gz/{registry_ref}");
    eprintln!("navi-core build: fetching registry from {url}");

    let mut response = ureq::get(&url)
        .call()
        .with_context(|| format!("failed to fetch registry tarball from {url}"))?;

    let mut reader = response
        .body_mut()
        .with_config()
        .limit(MAX_TARBALL_SIZE)
        .reader();

    let mut file = fs::File::create(&tar_path)
        .with_context(|| format!("failed to create {}", tar_path.display()))?;
    io::copy(&mut reader, &mut file)
        .with_context(|| format!("failed to write registry tarball to {}", tar_path.display()))?;

    // Extract.
    fs::create_dir_all(&extract_dir)
        .with_context(|| format!("failed to create {}", extract_dir.display()))?;
    let tar_gz = fs::File::open(&tar_path)
        .with_context(|| format!("failed to open {}", tar_path.display()))?;
    let gz = flate2::read::GzDecoder::new(tar_gz);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(&extract_dir).with_context(|| {
        format!(
            "failed to extract {} into {}",
            tar_path.display(),
            extract_dir.display()
        )
    })?;

    find_registry_source(&extract_dir)?.with_context(|| {
        format!(
            "could not find a valid registry source directory under {}",
            extract_dir.display()
        )
    })
}

fn is_full_git_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn registry_cache_dir(commit: &str) -> Result<PathBuf> {
    // Prefer a user cache directory; fall back to a temporary location if unknown.
    if let Some(base) = directories::BaseDirs::new() {
        return Ok(base.cache_dir().join("navi").join("registry").join(commit));
    }
    if let Ok(tmp) = env::var("TMPDIR") {
        return Ok(PathBuf::from(tmp).join("navi-registry").join(commit));
    }
    Ok(std::env::temp_dir().join("navi-registry").join(commit))
}

fn find_registry_source(extract_dir: &Path) -> Result<Option<PathBuf>> {
    if !extract_dir.is_dir() {
        return Ok(None);
    }
    let mut candidates: Vec<PathBuf> = fs::read_dir(extract_dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();

    if candidates.len() == 1 {
        return Ok(Some(candidates.into_iter().next().unwrap()));
    }

    candidates.retain(|p| p.join("manifest.json").is_file());
    if candidates.len() == 1 {
        return Ok(Some(candidates.into_iter().next().unwrap()));
    }

    Ok(None)
}

fn collect_json_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<_> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    Ok(files)
}

fn file_stem(path: &Path) -> Result<&str> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .with_context(|| format!("invalid UTF-8 file name: {}", path.display()))
}

fn embedded_path(path: &Path, embedded_dir: &Path) -> Result<String> {
    let rel = path.strip_prefix(embedded_dir).with_context(|| {
        format!(
            "path {} is not under {}",
            path.display(),
            embedded_dir.display()
        )
    })?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}
