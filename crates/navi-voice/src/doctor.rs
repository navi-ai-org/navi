//! Health checks for voice capture (recorder binaries on PATH).

use anyhow::Result;

use crate::capture::{RecorderKind, discover_recorder, list_available_recorders};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub lines: Vec<String>,
}

/// Inputs for doctor (from navi-core VoiceConfig or CLI).
#[derive(Debug, Clone)]
pub struct DoctorInput {
    pub enabled: bool,
    pub language: String,
    pub capture: String,
    pub recorder: String,
}

/// Run diagnostics for the voice capture stack.
pub fn run_doctor(input: &DoctorInput) -> Result<DoctorReport> {
    let mut lines = Vec::new();
    let mut ok = true;

    lines.push(format!("Config enabled: {}", input.enabled));
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

    if input.enabled && available.is_empty() {
        ok = false;
        lines.push("Voice is enabled in config but no recorder is available.".into());
    }

    lines.push(if ok {
        "Doctor: OK".into()
    } else {
        "Doctor: issues found".into()
    });

    Ok(DoctorReport { ok, lines })
}
