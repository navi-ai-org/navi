use anyhow::{Context, Result, bail};
use navi_core::{
    CredentialStore, LoadedConfig, ProviderConfig, ProviderKind, find_transcription_provider,
    resolve_provider_api_key, resolve_transcription_model, transcription_provider_catalog,
};
use navi_voice::{
    RecorderKind, RemoteTranscriptionConfig, RemoteTranscriptionKind, discover_recorder,
    list_available_recorders, transcribe_file_remote,
};

pub async fn handle_voice_command(
    action: crate::VoiceAction,
    loaded_config: &LoadedConfig,
) -> Result<()> {
    let voice = &loaded_config.config.voice;
    let data_dir = &loaded_config.data_dir;

    match action {
        crate::VoiceAction::Status => {
            println!("Voice System Status:");
            println!("  Data dir: {}", data_dir.display());
            println!("  Enabled: {}", voice.enabled);
            if voice.uses_remote_transcription() {
                let provider = voice.provider.trim();
                println!("  Provider: {provider}");
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
                println!("  Provider: (none)");
                println!("  Transcription: remote only — set [voice] provider (openai | groq)");
            }
            println!("  Language: {}", voice.language);
            println!("  Capture: {}", voice.capture);
            println!("  Recorder: {}", voice.recorder);

            println!();
            println!("Recorders on PATH:");
            let available = list_available_recorders();
            if available.is_empty() {
                println!("  (none found — install pw-record, parec, or arecord)");
            } else {
                for (kind, path) in &available {
                    println!("  {} → {}", kind.display_name(), path.display());
                }
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
        crate::VoiceAction::Doctor => {
            println!("Voice doctor");
            let mut ok = true;

            if voice.uses_remote_transcription() {
                let provider = voice.provider.trim();
                println!("  Remote provider: {provider}");
                match find_transcription_provider(provider) {
                    Some(reg) => {
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
                            ok = false;
                        }
                    }
                    None => {
                        println!("  [FAIL] Unknown transcription provider '{provider}'");
                        ok = false;
                    }
                }
            } else {
                println!(
                    "  [FAIL] No remote provider configured — set [voice] provider (openai | groq)"
                );
                ok = false;
            }

            // Audio capture is always local, even when transcription is remote.
            let available = list_available_recorders();
            if available.is_empty() {
                println!("  [FAIL] Recorder: none found on PATH");
                for kind in RecorderKind::all() {
                    println!("    - missing {} — {}", kind.binary(), kind.install_hint());
                }
                ok = false;
            } else {
                for (kind, path) in &available {
                    println!(
                        "  [OK] Recorder {} → {}",
                        kind.display_name(),
                        path.display()
                    );
                }
                match discover_recorder(&voice.recorder) {
                    Some((kind, path)) => println!(
                        "  [OK] Selected recorder: {} ({})",
                        kind.display_name(),
                        path.display()
                    ),
                    None => {
                        println!("  [FAIL] Selected recorder '{}' not found", voice.recorder);
                        ok = false;
                    }
                }
            }

            if !ok {
                bail!("voice doctor found issues");
            }
        }
        crate::VoiceAction::Transcribe { path, language } => {
            let lang = if language.trim().is_empty() {
                voice.language.as_str()
            } else {
                language.as_str()
            };

            if !voice.uses_remote_transcription() {
                bail!(
                    "local voice transcription was removed; set [voice] provider to a remote \
                     registry provider (openai | groq) — see `navi voice providers`."
                );
            }

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
        }
    }

    Ok(())
}
