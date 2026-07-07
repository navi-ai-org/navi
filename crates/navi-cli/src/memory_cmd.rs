use anyhow::Result;
use navi_core::memory::{
    DreamOptions, HistoryEvent, MemoryManager, build_rebuild_context, run_checkpoint_writer,
    run_distill_maintenance, run_dream_maintenance_with_options,
};
use navi_core::{LoadedConfig, ModelMessage, ModelRole, effective_context_window};
use navi_sdk::build_provider_for_config;
use std::path::Path;

pub async fn handle_memory_command(
    action: crate::MemoryAction,
    loaded_config: &LoadedConfig,
    cwd: &Path,
) -> Result<()> {
    let memory_config = &loaded_config.config.memory;
    let manager = MemoryManager::new(
        cwd.to_path_buf(),
        loaded_config.data_dir.clone(),
        memory_config,
    )?;

    match action {
        crate::MemoryAction::Status => {
            println!("Memory System Status:");
            println!("  Memory Root: {:?}", manager.store.memory_root);
            println!("  Auto-Memory DB: {:?}", manager.auto_memory.db_path);
            println!("  Global Memory DB: {:?}", manager.global_memory.db_path);
            println!("  History DB Path: {:?}", manager.history.db_path);

            let sessions = manager.history.list_sessions()?;
            if let Some(session) = sessions.first() {
                let rebuild_count = manager.history.get_rebuild_count(&session.id)?;
                let checkpoint_count = manager.history.get_checkpoint_count(&session.id)?;
                let last_checkpoint = manager.history.get_last_checkpoint_time(&session.id)?;
                let event_count = manager.history.get_event_count(&session.id, "message")?;

                println!("  Last Active Session ID: {}", session.id);
                println!("  Current Cycle (Rebuilds + 1): {}", rebuild_count + 1);
                println!(
                    "  Checkpoint Thresholds Crossed: {} (Configured: {:?})",
                    checkpoint_count, memory_config.checkpoint_thresholds
                );
                println!(
                    "  Rebuild Threshold: {}%",
                    memory_config.rebuild_threshold * 100.0
                );
                println!(
                    "  Last Checkpoint Time: {}",
                    last_checkpoint.unwrap_or_else(|| "None".to_string())
                );
                println!("  Total Messages Logged in Session: {}", event_count);
            } else {
                println!("  Last Active Session ID: None");
                println!("  Current Cycle: 1");
                println!(
                    "  Checkpoint Thresholds Crossed: 0 (Configured: {:?})",
                    memory_config.checkpoint_thresholds
                );
                println!(
                    "  Rebuild Threshold: {}%",
                    memory_config.rebuild_threshold * 100.0
                );
                println!("  Last Checkpoint Time: None");
                println!("  Total Messages Logged in Session: 0");
            }
        }
        crate::MemoryAction::Checkpoint => {
            let sessions = manager.history.list_sessions()?;
            let session_id = if let Some(s) = sessions.first() {
                s.id.clone()
            } else {
                let fallback_id =
                    format!("manual-{}", navi_core::session::current_unix_timestamp());
                println!(
                    "No active session found. Running manual checkpoint under temporary session ID: {}",
                    fallback_id
                );
                fallback_id
            };

            let events = manager.history.get_recent_events(&session_id, None)?;
            let messages = to_model_messages(&events);

            let provider = build_provider_for_config(loaded_config)?;
            let model_name = &loaded_config.config.model.name;

            println!(
                "Running manual checkpoint writer for session '{}' using model '{}'...",
                session_id, model_name
            );
            run_checkpoint_writer(
                &session_id,
                &messages,
                &manager.auto_memory,
                provider.as_ref(),
                model_name,
            )
            .await?;
            println!("Checkpoint writer finished successfully.");
        }
        crate::MemoryAction::RebuildPreview => {
            let sessions = manager.history.list_sessions()?;
            let session_id = if let Some(s) = sessions.first() {
                s.id.clone()
            } else {
                anyhow::bail!(
                    "No active session found in history database. Cannot generate rebuild preview."
                );
            };

            let events = manager.history.get_recent_events(&session_id, None)?;
            let messages = to_model_messages(&events);
            let context_window = effective_context_window(&loaded_config.config);

            let boot_context = build_rebuild_context(
                &messages,
                &manager.auto_memory,
                &manager.global_memory,
                context_window,
                memory_config.injected_context_token_budget,
            );
            println!("=== REBUILD CONTEXT PREVIEW ===");
            println!("{}", boot_context);
        }
        crate::MemoryAction::History {
            query,
            limit,
            session_id,
        } => {
            let results = manager
                .history
                .search_history(&query, session_id.as_deref(), limit)?;
            println!("History Search Results for '{}':", query);
            if results.is_empty() {
                println!("  No matching events found.");
            } else {
                for event in results {
                    println!(
                        "--- Event ID: {} (Session: {}, Seq: {}, Type: {}) ---",
                        event.id, event.session_id, event.sequence, event.event_type
                    );
                    println!("  Created: {}", event.created_at);
                    if let Some(role) = event.role {
                        println!("  Role: {}", role);
                    }
                    if let Some(tool) = event.tool_name {
                        println!("  Tool: {}", tool);
                    }
                    if let Some(content) = event.content {
                        println!("  Content: {}", content);
                    }
                    if let Some(output) = event.tool_output {
                        println!("  Tool Output: {}", output);
                    }
                }
            }
        }
        crate::MemoryAction::Dream {
            apply,
            sessions,
            instructions,
        } => {
            let provider = build_provider_for_config(loaded_config)?;
            let model_name = &loaded_config.config.model.name;
            println!(
                "Running memory dream maintenance using model '{}'...",
                model_name
            );
            let result = run_dream_maintenance_with_options(
                &manager.auto_memory,
                &manager.global_memory,
                &manager.history,
                provider.as_ref(),
                model_name,
                DreamOptions {
                    session_limit: sessions,
                    instructions,
                    apply,
                },
            )
            .await?;
            println!("Dream maintenance finished successfully.");
            println!("  Output directory: {}", result.output_dir.display());
            println!(
                "  Project memory candidate: {}",
                result.project_memory_path.display()
            );
            println!(
                "  Global memory candidate: {}",
                result.global_memory_path.display()
            );
            println!("  Report: {}", result.report_path.display());
            println!("  Applied to active memory: {}", result.applied);
            if let Some(ref am_report) = result.auto_memory_report {
                println!();
                println!("  Auto-memory consolidation:");
                println!("    Stale memories marked: {}", am_report.marked_stale);
                println!("    Duplicates merged: {}", am_report.duplicates_merged);
                println!(
                    "    Active memories remaining: {}",
                    am_report.remaining_active
                );
            }
        }
        crate::MemoryAction::Distill => {
            let provider = build_provider_for_config(loaded_config)?;
            let model_name = &loaded_config.config.model.name;
            println!(
                "Running process distillation maintenance using model '{}'...",
                model_name
            );
            run_distill_maintenance(
                &manager.auto_memory,
                &manager.history,
                provider.as_ref(),
                model_name,
            )
            .await?;
            println!("Distill maintenance finished successfully.");
        }
        crate::MemoryAction::Init { embeddings, force } => {
            println!("Initializing auto-memory system...");

            let memory_root = &manager.store.memory_root;
            if !memory_root.exists() {
                std::fs::create_dir_all(memory_root)?;
                println!("  [OK] Created memory root: {:?}", memory_root);
            } else {
                println!("  [OK] Memory root exists: {:?}", memory_root);
            }

            let models_dir = memory_root.join("models");
            if !models_dir.exists() {
                std::fs::create_dir_all(&models_dir)?;
                println!("  [OK] Created models directory: {:?}", models_dir);
            } else {
                println!("  [OK] Models directory exists: {:?}", models_dir);
            }

            let db_path = manager.auto_memory.db_path.clone();
            let count = manager.auto_memory.count_active().unwrap_or(0);
            println!("  [OK] Auto-memory database: {:?}", db_path);
            println!("  [OK] Active memories: {}", count);

            if embeddings {
                println!();
                println!("Embedding model setup:");

                let model_file = navi_core::memory::DEFAULT_MODEL_FILE;
                let model_repo = navi_core::memory::DEFAULT_MODEL_REPO;
                let tokenizer_file = navi_core::memory::DEFAULT_TOKENIZER_FILE;
                let tokenizer_repo = navi_core::memory::DEFAULT_TOKENIZER_REPO;
                let model_path = models_dir.join(model_file);
                let tokenizer_path = models_dir.join(tokenizer_file);

                if model_path.exists() && !force {
                    let size = std::fs::metadata(&model_path).map(|m| m.len()).unwrap_or(0);
                    println!("  [OK] Embedding model already exists: {:?}", model_path);
                    println!("       Size: {:.1} MB", size as f64 / 1_048_576.0);
                    if tokenizer_path.exists() {
                        println!("  [OK] Tokenizer exists: {:?}", tokenizer_path);
                    } else {
                        println!("  [WARN] Tokenizer missing — downloading...");
                        let tok_url = format!(
                            "https://huggingface.co/{}/resolve/main/{}",
                            tokenizer_repo, tokenizer_file
                        );
                        let client = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(60))
                            .build()?;
                        let resp = client.get(&tok_url).send().await?;
                        if resp.status().is_success() {
                            std::fs::write(&tokenizer_path, resp.bytes().await?)?;
                            println!("  [OK] Tokenizer downloaded.");
                        } else {
                            println!("  [WARN] Tokenizer download failed: HTTP {}", resp.status());
                        }
                    }
                    println!("       (use --force to re-download)");
                } else {
                    if force && model_path.exists() {
                        println!("  [..] Removing existing model ( --force )...");
                        std::fs::remove_file(&model_path)?;
                    }

                    let client = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(600))
                        .build()?;

                    // Download model
                    let url = format!(
                        "https://huggingface.co/{}/resolve/main/{}",
                        model_repo, model_file
                    );
                    println!("  [..] Downloading embedding model...");
                    println!("       URL: {}", url);
                    println!("       Dest: {:?}", model_path);
                    println!("       This is a one-time download (~400MB Q8_0 GGUF).");

                    let response = client.get(&url).send().await?;
                    if !response.status().is_success() {
                        anyhow::bail!("Download failed: HTTP {} from {}", response.status(), url);
                    }

                    let total = response.content_length();
                    if let Some(t) = total {
                        println!("       Total size: {:.1} MB", t as f64 / 1_048_576.0);
                    }

                    let bytes = response.bytes().await?;
                    std::fs::write(&model_path, &bytes)?;

                    let size = std::fs::metadata(&model_path).map(|m| m.len()).unwrap_or(0);
                    println!(
                        "  [OK] Model downloaded: {:.1} MB",
                        size as f64 / 1_048_576.0
                    );

                    // Download tokenizer
                    let tok_url = format!(
                        "https://huggingface.co/{}/resolve/main/{}",
                        tokenizer_repo, tokenizer_file
                    );
                    println!("  [..] Downloading tokenizer...");
                    let tok_resp = client.get(&tok_url).send().await?;
                    if tok_resp.status().is_success() {
                        std::fs::write(&tokenizer_path, tok_resp.bytes().await?)?;
                        println!("  [OK] Tokenizer downloaded: {:?}", tokenizer_path);
                    } else {
                        println!(
                            "  [WARN] Tokenizer download failed: HTTP {}",
                            tok_resp.status()
                        );
                        println!(
                            "         Semantic search requires the tokenizer. Download manually:"
                        );
                        println!(
                            "         huggingface-cli download {} {}",
                            tokenizer_repo, tokenizer_file
                        );
                    }

                    println!("       Semantic search is now available.");
                }
            } else {
                println!();
                println!("  [INFO] Embedding model not requested ( --embeddings ).");
                println!("         Text matching (LIKE) will be used for memory search.");
                println!("         Run `navi memory init --embeddings` to enable semantic search.");
            }

            println!();
            println!("Auto-memory initialization complete.");
        }
        crate::MemoryAction::List { status, limit } => {
            let store = manager.auto_memory.clone();
            let status_filter = status
                .as_deref()
                .and_then(navi_core::memory::MemoryStatus::from_str);
            let memories = store.list(status_filter)?;

            if memories.is_empty() {
                println!("No memories found.");
            } else {
                let limited: Vec<_> = memories.into_iter().take(limit).collect();
                println!("Memories ({} shown):", limited.len());
                for m in &limited {
                    println!(
                        "  [{}] {} ({}) — {} (conf: {:.2}, {})",
                        m.id,
                        m.name,
                        m.memory_type,
                        m.description,
                        m.confidence,
                        m.status.as_str()
                    );
                }
            }
        }
        crate::MemoryAction::Search { query, limit } => {
            let store = manager.auto_memory.clone();
            let results = store.search_text(&query, limit)?;

            if results.is_empty() {
                println!("No memories found for '{}'.", query);
            } else {
                println!("Search results for '{}' ({}):", query, results.len());
                for m in &results {
                    println!(
                        "  [{}] {} ({}) — {} (conf: {:.2})",
                        m.id, m.name, m.memory_type, m.description, m.confidence
                    );
                }
            }
        }
        crate::MemoryAction::Doctor => {
            println!("Memory Doctor diagnostics:");
            // Check Config
            println!(
                "  [OK] Memory config loaded. Enabled: {}",
                memory_config.enabled
            );

            // Check Root
            let root = &manager.store.memory_root;
            println!("  Memory Root Directory: {:?}", root);
            if root.exists() {
                println!("    [OK] Root path exists and is readable.");
            } else {
                println!(
                    "    [WARN] Root path does not exist yet. It will be created when needed."
                );
            }

            // Check Checkpoint (SQLite)
            let cp_content = manager.auto_memory.read_checkpoint().unwrap_or_default();
            println!("  Checkpoint (SQLite): {:?}", manager.auto_memory.db_path);
            if !cp_content.is_empty() {
                println!("    [OK] Readable ({} bytes)", cp_content.len());
            } else {
                println!("    [WARN] No checkpoint stored yet.");
            }

            // Check Notes (SQLite)
            let notes_content = manager.auto_memory.read_notes().unwrap_or_default();
            println!("  Notes (SQLite): {:?}", manager.auto_memory.db_path);
            if !notes_content.is_empty() {
                println!("    [OK] Readable ({} bytes)", notes_content.len());
            } else {
                println!("    [WARN] No notes stored yet.");
            }

            // Check Project Memory (SQLite)
            let pm_index = manager.auto_memory.render_index();
            println!(
                "  Project Memory (SQLite): {:?}",
                manager.auto_memory.db_path
            );
            if !pm_index.trim().is_empty() {
                println!("    [OK] Readable ({} bytes)", pm_index.len());
            } else {
                println!("    [WARN] No project memories yet.");
            }

            // Check Global Memory (SQLite)
            let gm_index = manager.global_memory.read_index().unwrap_or_default();
            println!(
                "  Global Memory (SQLite): {:?}",
                manager.global_memory.db_path
            );
            if !gm_index.trim().is_empty() {
                println!("    [OK] Readable ({} bytes)", gm_index.len());
            } else {
                println!("    [WARN] No global memories yet.");
            }

            // Check DB and tables
            let db_path = &manager.history.db_path;
            println!("  History DB: {:?}", db_path);
            if db_path.exists() {
                match manager.history.doctor_check() {
                    Ok(logs) => {
                        for log in logs {
                            if log.starts_with("ERROR:") {
                                println!("    [ERROR] {}", log);
                            } else {
                                println!("    [OK] {}", log);
                            }
                        }
                    }
                    Err(e) => println!("    [ERROR] Failed database check: {}", e),
                }
            } else {
                println!("    [WARN] DB file does not exist yet.");
            }
            println!("Doctor diagnostics completed.");
        }
    }

    Ok(())
}

fn to_model_messages(events: &[HistoryEvent]) -> Vec<ModelMessage> {
    events
        .iter()
        .filter(|e| e.event_type == "message")
        .map(|e| {
            let role = match e.role.as_deref() {
                Some("system") => ModelRole::System,
                Some("assistant") => ModelRole::Assistant,
                Some("tool") => ModelRole::Tool,
                _ => ModelRole::User,
            };
            ModelMessage {
                role,
                content: e.content.clone().unwrap_or_default(),
                content_parts: Vec::new(),
                tool_call_id: None,
                tool_name: e.tool_name.clone(),
                tool_calls: Vec::new(),
                created_at: None,
                thinking_content: None,
            }
        })
        .collect()
}
