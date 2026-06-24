use anyhow::Result;
use navi_core::memory::{
    HistoryEvent, MemoryManager, build_rebuild_context, run_checkpoint_writer,
    run_distill_maintenance, run_dream_maintenance,
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
    let manager = MemoryManager::new(cwd.to_path_buf(), memory_config)?;

    match action {
        crate::MemoryAction::Status => {
            println!("Memory System Status:");
            println!("  Memory Root: {:?}", manager.store.memory_root);
            println!(
                "  Global Memory Path: {:?}",
                manager.store.global_memory_path
            );
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
                &manager.store,
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
                &manager.store,
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
        crate::MemoryAction::Dream => {
            let provider = build_provider_for_config(loaded_config)?;
            let model_name = &loaded_config.config.model.name;
            println!(
                "Running memory dream maintenance using model '{}'...",
                model_name
            );
            run_dream_maintenance(
                &manager.store,
                &manager.history,
                provider.as_ref(),
                model_name,
            )
            .await?;
            println!("Dream maintenance finished successfully.");
        }
        crate::MemoryAction::Distill => {
            let provider = build_provider_for_config(loaded_config)?;
            let model_name = &loaded_config.config.model.name;
            println!(
                "Running process distillation maintenance using model '{}'...",
                model_name
            );
            run_distill_maintenance(
                &manager.store,
                &manager.history,
                provider.as_ref(),
                model_name,
            )
            .await?;
            println!("Distill maintenance finished successfully.");
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

            // Check Checkpoint file
            let cp_path = manager.store.checkpoint_path();
            println!("  Checkpoint File: {:?}", cp_path);
            if cp_path.exists() {
                match std::fs::read_to_string(&cp_path) {
                    Ok(content) => println!("    [OK] Readable ({} bytes)", content.len()),
                    Err(e) => println!("    [ERROR] Failed to read checkpoint: {}", e),
                }
            } else {
                println!("    [WARN] Does not exist yet.");
            }

            // Check Notes file
            let notes_path = manager.store.notes_path();
            println!("  Notes File: {:?}", notes_path);
            if notes_path.exists() {
                match std::fs::read_to_string(&notes_path) {
                    Ok(content) => println!("    [OK] Readable ({} bytes)", content.len()),
                    Err(e) => println!("    [ERROR] Failed to read notes: {}", e),
                }
            } else {
                println!("    [WARN] Does not exist yet.");
            }

            // Check Project Memory file
            let pm_path = manager.store.project_memory_path();
            println!("  Project Memory File: {:?}", pm_path);
            if pm_path.exists() {
                match std::fs::read_to_string(&pm_path) {
                    Ok(content) => println!("    [OK] Readable ({} bytes)", content.len()),
                    Err(e) => println!("    [ERROR] Failed to read project memory: {}", e),
                }
            } else {
                println!("    [WARN] Does not exist yet.");
            }

            // Check Global Memory file
            let gm_path = manager.store.global_memory_path;
            println!("  Global Memory File: {:?}", gm_path);
            if gm_path.exists() {
                match std::fs::read_to_string(&gm_path) {
                    Ok(content) => println!("    [OK] Readable ({} bytes)", content.len()),
                    Err(e) => println!("    [ERROR] Failed to read global memory: {}", e),
                }
            } else {
                println!("    [WARN] Does not exist yet.");
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
