//! Mic recorder discovery (Linux-first).

use std::path::PathBuf;
use std::process::Command;

/// Known system recorders, ordered by preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderKind {
    PwRecord,
    Parec,
    Arecord,
}

impl RecorderKind {
    pub fn binary(self) -> &'static str {
        match self {
            Self::PwRecord => "pw-record",
            Self::Parec => "parec",
            Self::Arecord => "arecord",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::PwRecord => "PipeWire (pw-record)",
            Self::Parec => "PulseAudio (parec)",
            Self::Arecord => "ALSA (arecord)",
        }
    }

    pub fn install_hint(self) -> &'static str {
        match self {
            Self::PwRecord => "Install pipewire (package often provides pw-record)",
            Self::Parec => "Install pulseaudio-utils (parec)",
            Self::Arecord => "Install alsa-utils (arecord)",
        }
    }

    /// All candidates in preference order.
    pub fn all() -> &'static [Self] {
        &[Self::PwRecord, Self::Parec, Self::Arecord]
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => None,
            "pw-record" | "pipewire" | "pw" => Some(Self::PwRecord),
            "parec" | "pulse" | "pulseaudio" => Some(Self::Parec),
            "arecord" | "alsa" => Some(Self::Arecord),
            _ => None,
        }
    }
}

/// Returns absolute path if `binary` is on PATH.
pub fn which_binary(binary: &str) -> Option<PathBuf> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {}", shell_escape(binary)))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

fn shell_escape(s: &str) -> String {
    // binaries are fixed tokens; still quote defensively
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// First available recorder (or the explicit one if set and found).
pub fn discover_recorder(preference: &str) -> Option<(RecorderKind, PathBuf)> {
    if let Some(kind) = RecorderKind::parse(preference) {
        return which_binary(kind.binary()).map(|p| (kind, p));
    }
    for kind in RecorderKind::all() {
        if let Some(path) = which_binary(kind.binary()) {
            return Some((*kind, path));
        }
    }
    None
}

/// List which recorders are present.
pub fn list_available_recorders() -> Vec<(RecorderKind, PathBuf)> {
    RecorderKind::all()
        .iter()
        .filter_map(|k| which_binary(k.binary()).map(|p| (*k, p)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_recorder_kinds() {
        assert_eq!(
            RecorderKind::parse("pw-record"),
            Some(RecorderKind::PwRecord)
        );
        assert_eq!(RecorderKind::parse("parec"), Some(RecorderKind::Parec));
        assert_eq!(RecorderKind::parse("arecord"), Some(RecorderKind::Arecord));
        assert_eq!(RecorderKind::parse("auto"), None);
    }
}
