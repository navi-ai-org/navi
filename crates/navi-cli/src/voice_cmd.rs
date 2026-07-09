use anyhow::{Context, Result, bail};
use navi_core::LoadedConfig;
use navi_voice::{
    AsrEngineId, DoctorInput, VoiceInstallOptions, download_engine, engine_installed,
    resolve_model_dir, run_doctor, voice_root,
};
#[cfg(feature = "voice-onnx")]
use navi_voice::NemotronOnnxEngine;

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
            println!("  Engine (config): {}", voice.engine);
            println!("  Language: {}", voice.language);
            println!("  Capture: {}", voice.capture);
            println!("  Recorder: {}", voice.recorder);
            println!("  HF repo (nemotron): {}", voice.hf_repo_nemotron);

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
        }
        crate::VoiceAction::Init { engine, force } => {
            let engine = AsrEngineId::parse(&engine).with_context(|| {
                format!(
                    "unknown engine '{engine}'. Use: nemotron_streaming | distil_whisper"
                )
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
                        println!(
                            "       done {:.1} MB",
                            downloaded as f64 / 1_048_576.0
                        );
                    }
                }
            });

            let dir = download_engine(data_dir, &options, engine, force, Some(progress))
                .await
                .with_context(|| format!("download engine {}", engine.as_str()))?;
            println!("  [OK] Engine ready at {}", dir.display());
            println!();
            println!("Next: set `[voice] enabled = true` in ~/.config/navi/config.toml");
            println!("      (TUI Ctrl+Space dictation lands in a follow-up change)");
        }
        crate::VoiceAction::Doctor => {
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
            #[cfg(not(feature = "voice-onnx"))]
            {
                let _ = (path, language, engine_installed, resolve_model_dir, options);
                bail!(
                    "voice transcription requires the navi-cli `voice-onnx` feature (ONNX Runtime)."
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
                let lang = if language.trim().is_empty() {
                    voice.language.as_str()
                } else {
                    language.as_str()
                };
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
