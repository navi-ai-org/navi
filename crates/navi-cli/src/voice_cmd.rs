use anyhow::{Context, Result, bail};
use navi_core::{
    CredentialStore, LoadedConfig, ProviderConfig, ProviderKind, find_transcription_provider,
    resolve_provider_api_key, resolve_transcription_model, transcription_provider_catalog,
};
#[cfg(feature = "voice-onnx")]
use navi_voice::NemotronOnnxEngine;
use navi_voice::{
    AsrEngineId, DoctorInput, RemoteTranscriptionConfig, RemoteTranscriptionKind,
    VoiceInstallOptions, download_engine, engine_installed, resolve_model_dir, run_doctor,
    transcribe_file_remote, voice_root,
};

pub async fn handle_voice_command(
    action: crate::VoiceAction,
    loaded_config: &LoadedConfig,
) -> Result<()> {
    let voice = &loaded_config.config.voice;
    let options = VoiceInstallOptions {
        model_dir: voice.model_dir.clone(),
        hf_repo_nemotron: voice.hf_repo_nemotron.clone(),
    };
    let data_dir = &loaded_config.data_dir;

    match action {
        crate::VoiceAction::Status => {
            println!("Voice System Status:");
            println!("  Data dir: {}", data_dir.display());
            println!("  Voice root: {}", voice_root(data_dir).display());
            println!("  Enabled: {}", voice.enabled);
            let provider = if voice.provider.trim().is_empty() {
                "local"
            } else {
                voice.provider.as_str()
            };
            println!("  Provider: {provider}");
            if voice.uses_remote_transcription() {
                if let Some(reg) = find_transcription_provider(provider) {
                    let model = resolve_transcription_model(&reg, &voice.model);
                    println!("  Remote model: {model}");
                    println!("  API kind: {}", reg.kind);
                    println!("  Base URL: {}", reg.base_url);
                    println!("  API key env: ${}", reg.api_key_env);
                    let store = CredentialStore::new(data_dir.clone());
                    let synthetic = ProviderConfig {
                        id: reg.id.clone(),
                        label: reg.label.clone(),
                        description: reg.description.clone(),
                        kind: ProviderKind::OpenAiChatCompletions,
                        api_key_env: reg.api_key_env.clone(),
                        base_url: Some(reg.base_url.clone()),
                        ..Default::default()
                    };
                    let has_key = resolve_provider_api_key(&store, &synthetic, &reg.id).is_some();
                    println!(
                        "  Credentials: {}",
                        if has_key { "configured" } else { "missing" }
                    );
                } else {
                    println!("  Remote model: {} (unknown provider)", voice.model);
                }
            } else {
                println!("  Engine (local): {}", voice.engine);
                if !voice.model.is_empty() {
                    println!("  Model (unused for local): {}", voice.model);
                }
            }
            println!("  Language: {}", voice.language);
            println!("  Capture: {}", voice.capture);
            println!("  Recorder: {}", voice.recorder);
            println!("  HF repo (nemotron): {}", voice.hf_repo_nemotron);

            println!();
            println!("Local engines:");
            for engine in [AsrEngineId::NemotronStreaming, AsrEngineId::DistilWhisper] {
                let installed = engine_installed(data_dir, &options, engine);
                println!(
                    "  {}: {}",
                    engine.as_str(),
                    if installed {
                        "installed"
                    } else {
                        "not installed"
                    }
                );
            }
            println!();
            println!("Remote transcription providers (registry):");
            for p in transcription_provider_catalog() {
                let default = p.resolved_default_model().unwrap_or("?");
                println!(
                    "  {} — {} (default model: {}, {} models)",
                    p.id,
                    p.label,
                    default,
                    p.models.len()
                );
            }
        }
        crate::VoiceAction::Providers => {
            println!("Remote transcription providers:");
            for p in transcription_provider_catalog() {
                println!();
                println!("{} ({})", p.label, p.id);
                println!("  kind: {}", p.kind);
                println!("  base_url: {}", p.base_url);
                println!("  api_key_env: ${}", p.api_key_env);
                println!(
                    "  default_model: {}",
                    p.resolved_default_model().unwrap_or("?")
                );
                println!("  models:");
                for m in &p.models {
                    let label = m.label.as_deref().unwrap_or(m.name.as_str());
                    println!("    - {} — {label}", m.name);
                }
            }
        }
        crate::VoiceAction::Init { engine, force } => {
            let engine = AsrEngineId::parse(&engine).with_context(|| {
                format!("unknown engine '{engine}'. Use: nemotron_streaming | distil_whisper")
            })?;
            println!("Voice init — {}", engine.display_name());
            println!("  Destination under: {}", voice_root(data_dir).display());
            if force {
                println!("  Force re-download: yes");
            }

            let last_file = std::sync::Mutex::new(String::new());
            let progress = Box::new(move |downloaded: u64, total: Option<u64>, file: &str| {
                let mut last = last_file.lock().unwrap_or_else(|e| e.into_inner());
                if *last != file {
                    *last = file.to_string();
                    println!("  [..] {file}");
                }
                if let Some(t) = total {
                    if t > 0 && downloaded == t {
                        println!("       done {:.1} MB", downloaded as f64 / 1_048_576.0);
                    }
                }
            });

            let dir = download_engine(data_dir, &options, engine, force, Some(progress))
                .await
                .with_context(|| format!("download engine {}", engine.as_str()))?;
            println!("  [OK] Engine ready at {}", dir.display());
            println!();
            println!("Next: set `[voice] enabled = true` in ~/.config/navi/config.toml");
            println!("      For remote dictation, also set:");
            println!("        provider = \"openai\"   # or groq | wispr-flow");
            println!("        model = \"whisper-1\"");
        }
        crate::VoiceAction::Doctor => {
            if voice.uses_remote_transcription() {
                let provider = voice.provider.as_str();
                println!("Voice doctor (remote provider: {provider})");
                let Some(reg) = find_transcription_provider(provider) else {
                    bail!("unknown transcription provider '{provider}'");
                };
                println!("  [OK] Provider found in registry: {}", reg.label);
                println!("  kind: {}", reg.kind);
                println!("  models: {}", reg.models.len());
                let store = CredentialStore::new(data_dir.clone());
                let synthetic = ProviderConfig {
                    id: reg.id.clone(),
                    label: reg.label.clone(),
                    description: reg.description.clone(),
                    kind: ProviderKind::OpenAiChatCompletions,
                    api_key_env: reg.api_key_env.clone(),
                    base_url: Some(reg.base_url.clone()),
                    ..Default::default()
                };
                if resolve_provider_api_key(&store, &synthetic, &reg.id).is_some() {
                    println!("  [OK] API key resolved (${})", reg.api_key_env);
                } else {
                    println!("  [FAIL] Missing API key — set ${}", reg.api_key_env);
                    bail!("voice doctor found issues");
                }
                return Ok(());
            }
            let engine = AsrEngineId::parse(&voice.engine).unwrap_or_default();
            let report = run_doctor(
                data_dir,
                &DoctorInput {
                    enabled: voice.enabled,
                    engine,
                    language: voice.language.clone(),
                    capture: voice.capture.clone(),
                    recorder: voice.recorder.clone(),
                    options,
                },
            )?;
            for line in &report.lines {
                println!("{line}");
            }
            if !report.ok {
                bail!("voice doctor found issues");
            }
        }
        crate::VoiceAction::Transcribe { path, language } => {
            let lang = if language.trim().is_empty() {
                voice.language.as_str()
            } else {
                language.as_str()
            };

            if voice.uses_remote_transcription() {
                let provider_id = voice.provider.trim();
                let reg = find_transcription_provider(provider_id)
                    .with_context(|| format!("unknown transcription provider '{provider_id}'"))?;
                let kind = RemoteTranscriptionKind::parse(&reg.kind)
                    .with_context(|| format!("unsupported kind '{}'", reg.kind))?;
                let model = resolve_transcription_model(&reg, &voice.model);
                let store = CredentialStore::new(data_dir.clone());
                let synthetic = ProviderConfig {
                    id: reg.id.clone(),
                    label: reg.label.clone(),
                    description: reg.description.clone(),
                    kind: ProviderKind::OpenAiChatCompletions,
                    api_key_env: reg.api_key_env.clone(),
                    base_url: Some(reg.base_url.clone()),
                    ..Default::default()
                };
                let api_key =
                    resolve_provider_api_key(&store, &synthetic, &reg.id).with_context(|| {
                        format!("missing API key for '{}'. Set ${}", reg.id, reg.api_key_env)
                    })?;
                let language = if lang.eq_ignore_ascii_case("auto") || lang.is_empty() {
                    None
                } else {
                    Some(lang.to_string())
                };
                let cfg = RemoteTranscriptionConfig {
                    provider_id: reg.id.clone(),
                    kind,
                    base_url: reg.base_url.clone(),
                    transcription_path: reg.resolved_path().to_string(),
                    api_key,
                    model: model.clone(),
                    language,
                };
                println!("Remote transcription — {}", reg.label);
                println!("  Provider: {}", reg.id);
                println!("  Model: {model}");
                println!("  Language: {lang}");
                println!("  Audio: {path}");
                let started = std::time::Instant::now();
                let result = transcribe_file_remote(&cfg, path.as_ref())
                    .await
                    .with_context(|| format!("transcribe {path}"))?;
                let elapsed = started.elapsed();
                println!();
                println!("{}", result.text);
                println!();
                if let Some(det) = result.detected_language {
                    println!("(detected_language={det}, {:.2}s)", elapsed.as_secs_f64());
                } else {
                    println!("({:.2}s)", elapsed.as_secs_f64());
                }
                return Ok(());
            }

            #[cfg(not(feature = "voice-onnx"))]
            {
                let _ = (path, language, engine_installed, resolve_model_dir, options);
                bail!(
                    "local voice transcription requires the navi-cli `voice-onnx` feature \
                     (ONNX Runtime), or set [voice] provider to a remote registry provider \
                     (openai | groq | wispr-flow)."
                );
            }
            #[cfg(feature = "voice-onnx")]
            {
                let engine_id = AsrEngineId::NemotronStreaming;
                if !engine_installed(data_dir, &options, engine_id) {
                    bail!(
                        "Nemotron streaming engine not installed. Run: navi voice init --engine nemotron_streaming"
                    );
                }
                let model_dir = resolve_model_dir(data_dir, &options, engine_id);
                println!("Loading {}", engine_id.display_name());
                println!("  Model: {}", model_dir.display());
                println!("  Language: {lang}");
                println!("  Audio: {path}");
                let mut engine = NemotronOnnxEngine::load(&model_dir, lang)
                    .context("load Nemotron ONNX engine")?;
                let started = std::time::Instant::now();
                let result = engine
                    .transcribe_wav(&path)
                    .with_context(|| format!("transcribe {path}"))?;
                let elapsed = started.elapsed();
                println!();
                println!("{}", result.text);
                println!();
                println!(
                    "({} tokens, {:.2}s)",
                    result.token_ids.len(),
                    elapsed.as_secs_f64()
                );
            }
        }
    }

    Ok(())
}
