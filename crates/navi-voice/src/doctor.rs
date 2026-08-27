//! Health checks for the local voice stack.

use std::path::Path;

use anyhow::Result;

use crate::capture::{RecorderKind, discover_recorder, list_available_recorders};
use crate::download::{engine_installed, verify_engine_checksums};
use crate::paths::VoicePaths;
use crate::types::{AsrEngineId, VoiceInstallOptions};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub lines: Vec<String>,
}

/// Inputs for doctor (from navi-core VoiceConfig or CLI).
#[derive(Debug, Clone)]
pub struct DoctorInput {
    pub enabled: bool,
    pub engine: AsrEngineId,
    pub language: String,
    pub capture: String,
    pub recorder: String,
    pub options: VoiceInstallOptions,
}

/// Run diagnostics for voice capture + model install.
pub fn run_doctor(data_dir: &Path, input: &DoctorInput) -> Result<DoctorReport> {
    let mut lines = Vec::new();
    let mut ok = true;

    lines.push(format!("Voice root: {}", data_dir.join("voice").display()));
    lines.push(format!("Config enabled: {}", input.enabled));
    lines.push(format!(
        "Engine: {} ({})",
        input.engine.as_str(),
        input.engine.display_name()
    ));
    lines.push(format!("Language: {}", input.language));
    lines.push(format!("Capture mode: {}", input.capture));
    lines.push(format!("Recorder preference: {}", input.recorder));

    // Recorders
    let available = list_available_recorders();
    if available.is_empty() {
        ok = false;
        lines.push("Recorder: NONE found on PATH".into());
        for kind in RecorderKind::all() {
            lines.push(format!(
                "  - missing {} — {}",
                kind.binary(),
                kind.install_hint()
            ));
        }
    } else {
        lines.push("Recorders on PATH:".into());
        for (kind, path) in &available {
            lines.push(format!(
                "  [OK] {} → {}",
                kind.display_name(),
                path.display()
            ));
        }
        match discover_recorder(&input.recorder) {
            Some((kind, path)) => {
                lines.push(format!(
                    "Selected recorder: {} ({})",
                    kind.display_name(),
                    path.display()
                ));
            }
            None => {
                ok = false;
                lines.push(format!("Selected recorder '{}' not found", input.recorder));
            }
        }
    }

    // Engine install
    for engine in [AsrEngineId::NemotronStreaming, AsrEngineId::DistilWhisper] {
        let paths = VoicePaths::resolve(data_dir, &input.options, engine);
        let installed = engine_installed(data_dir, &input.options, engine);
        if installed {
            lines.push(format!(
                "[OK] {} installed at {}",
                engine.as_str(),
                paths.engine_dir.display()
            ));
            match verify_engine_checksums(data_dir, &input.options, engine) {
                Ok(()) => lines.push(format!("     checksums OK ({})", paths.checksums.display())),
                Err(err) => {
                    ok = false;
                    lines.push(format!("     checksum FAILED: {err:#}"));
                }
            }
        } else {
            let mark = if engine == input.engine {
                ok = false;
                "[MISSING]"
            } else {
                "[—]"
            };
            lines.push(format!(
                "{mark} {} not installed (expected {})",
                engine.as_str(),
                paths.engine_dir.display()
            ));
            if engine == AsrEngineId::NemotronStreaming {
                lines.push("     Run: navi voice init --engine nemotron_streaming".into());
            } else {
                lines.push("     distil_whisper packaging lands in a later release".into());
            }
        }
    }

    if input.enabled && !engine_installed(data_dir, &input.options, input.engine) {
        ok = false;
        lines.push("Voice is enabled in config but the selected engine is not installed.".into());
    }

    lines.push(if ok {
        "Doctor: OK".into()
    } else {
        "Doctor: issues found".into()
    });

    Ok(DoctorReport { ok, lines })
}
