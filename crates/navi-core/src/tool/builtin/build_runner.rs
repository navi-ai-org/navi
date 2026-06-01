use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

const BUILD_DEFAULT_TIMEOUT_MS: u64 = 300_000;
const BUILD_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
struct BuildDiagnostic {
    file: Option<String>,
    line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    column: Option<u64>,
    message: String,
    level: String,
    code: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
struct BuildOutput {
    status: String,
    cached: bool,
    duration_ms: u64,
    warnings: Vec<BuildDiagnostic>,
    errors: Vec<BuildDiagnostic>,
    artifact_path: Option<String>,
    summary: String,
}

pub(crate) struct BuildRunnerTool {
    state: Arc<Mutex<BuildState>>,
}

struct BuildState {
    last_build_time: Option<SystemTime>,
    last_source_mtime: Option<SystemTime>,
    last_result: Option<CachedResult>,
}

struct CachedResult {
    _status: String,
    warnings: Vec<Value>,
    errors: Vec<Value>,
    artifact_path: Option<String>,
}

impl BuildRunnerTool {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(BuildState {
                last_build_time: None,
                last_source_mtime: None,
                last_result: None,
            })),
        }
    }
}

#[async_trait]
impl Tool for BuildRunnerTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "build_runner",
            "Build/compile the project with caching. Returns structured warnings and errors instead of raw compiler output. Skips rebuild if no source files changed (incremental=true).",
            ToolKind::Command,
            json!({
                "type": "object",
                "properties": {
                    "profile": {
                        "type": "string",
                        "enum": ["debug", "release"],
                        "description": "Build profile. Defaults to debug."
                    },
                    "features": {
                        "type": "string",
                        "description": "Extra features or flags (e.g. cargo features, npm script args)."
                    },
                    "incremental": {
                        "type": "boolean",
                        "description": "When true (default), skip build if no source files changed since last build."
                    }
                },
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let profile = helpers::optional_string(&invocation.input, "profile")
            .unwrap_or_else(|| "debug".to_string());
        let features = helpers::optional_string(&invocation.input, "features");
        let incremental = helpers::optional_bool(&invocation.input, "incremental").unwrap_or(true);

        let build_system = detect_build_system().await?;

        if incremental {
            let current_mtime = latest_source_mtime(&build_system);
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());

            if let (Some(last_build), Some(src_mtime), Some(cached)) = (
                state.last_build_time,
                state.last_source_mtime,
                &state.last_result,
            ) && let Some(current) = current_mtime
                && current <= src_mtime
                && last_build.elapsed().unwrap_or_default().as_secs() < 300
            {
                return Ok(helpers::ok(
                    invocation.id,
                    helpers::versioned(BuildOutput {
                        status: "cached".to_string(),
                        cached: true,
                        duration_ms: 0,
                        warnings: cached
                            .warnings
                            .iter()
                            .filter_map(|v| serde_json::from_value(v.clone()).ok())
                            .collect(),
                        errors: cached
                            .errors
                            .iter()
                            .filter_map(|v| serde_json::from_value(v.clone()).ok())
                            .collect(),
                        artifact_path: cached.artifact_path.clone(),
                        summary: "build cached — no source changes detected".to_string(),
                    }),
                ));
            }
        }

        let command = build_command(&build_system, &profile, features.as_deref());

        let mut child = tokio::process::Command::new("bash")
            .arg("-lc")
            .arg(&command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to spawn build command: {command}"))?;

        let stdout_data = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let stderr_data = Arc::new(tokio::sync::Mutex::new(Vec::new()));

        let stdout = child.stdout.take().context("stdout was not piped")?;
        let stderr = child.stderr.take().context("stderr was not piped")?;
        spawn_reader(stdout, stdout_data.clone());
        spawn_reader(stderr, stderr_data.clone());

        let timeout_duration = Duration::from_millis(BUILD_DEFAULT_TIMEOUT_MS);
        let status_result = tokio::time::timeout(timeout_duration, child.wait()).await;

        let (exit_ok, exit_code, error_msg) = match status_result {
            Ok(Ok(status)) => (status.success(), status.code(), None),
            Ok(Err(e)) => (false, None, Some(format!("failed to wait for build: {e}"))),
            Err(_) => (false, None, Some("build timed out".to_string())),
        };

        tokio::time::sleep(Duration::from_millis(50)).await;

        let stdout_bytes = stdout_data.lock().await.clone();
        let stderr_bytes = stderr_data.lock().await.clone();
        let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
        let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();

        if let Some(err) = error_msg {
            return Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: helpers::tool_error(
                    "build_command_failed",
                    err,
                    true,
                    Some("Inspect stdout/stderr and retry after fixing build errors."),
                    Some(format!(
                        "stdout:\n{}\nstderr:\n{}",
                        helpers::truncate_string(stdout, BUILD_OUTPUT_LIMIT_BYTES),
                        helpers::truncate_string(stderr, BUILD_OUTPUT_LIMIT_BYTES)
                    )),
                ),
            });
        }

        let combined = format!("{stdout}\n{stderr}");
        let result = parse_build_output(&build_system, &combined, exit_ok, exit_code);

        // Update cache state
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.last_build_time = Some(SystemTime::now());
            state.last_source_mtime = latest_source_mtime(&build_system);
            state.last_result = Some(CachedResult {
                _status: result["status"].as_str().unwrap_or("error").to_string(),
                warnings: result["warnings"].as_array().cloned().unwrap_or_default(),
                errors: result["errors"].as_array().cloned().unwrap_or_default(),
                artifact_path: result["artifact_path"].as_str().map(|s| s.to_string()),
            });
        }

        Ok(helpers::ok(invocation.id, result))
    }
}

fn spawn_reader<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    reader: R,
    buffer: Arc<tokio::sync::Mutex<Vec<u8>>>,
) {
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut reader = reader;
        let mut buf = vec![0u8; 8192];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => buffer.lock().await.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
    });
}

async fn detect_build_system() -> Result<String> {
    if Path::new("Cargo.toml").exists() {
        return Ok("cargo".to_string());
    }
    if Path::new("package.json").exists() {
        if Path::new("bun.lockb").exists() {
            return Ok("bun".to_string());
        }
        return Ok("npm".to_string());
    }
    if Path::new("Makefile").exists() {
        return Ok("make".to_string());
    }
    anyhow::bail!("no build system detected in current directory");
}

fn build_command(system: &str, profile: &str, features: Option<&str>) -> String {
    let mut cmd = match system {
        "cargo" => {
            let mut c = "cargo build".to_string();
            if profile == "release" {
                c.push_str(" --release");
            }
            c
        }
        "npm" => "npm run build".to_string(),
        "bun" => "bun run build".to_string(),
        "make" => "make".to_string(),
        _ => "cargo build".to_string(),
    };

    if let Some(f) = features.filter(|f| !f.is_empty()) {
        match system {
            "cargo" => cmd.push_str(&format!(" --features {f}")),
            _ => cmd.push_str(&format!(" {f}")),
        }
    }

    cmd
}

fn latest_source_mtime(system: &str) -> Option<SystemTime> {
    let extensions: &[&str] = match system {
        "cargo" => &["rs"],
        "npm" | "bun" => &["ts", "tsx", "js", "jsx"],
        "make" => &["c", "cpp", "h", "hpp"],
        _ => &["rs"],
    };

    let mut latest = None;
    walk_source_files(Path::new("."), extensions, &mut latest);
    latest
}

fn walk_source_files(dir: &Path, extensions: &[&str], latest: &mut Option<SystemTime>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with('.') {
            continue;
        }
        if matches!(
            name_str.as_ref(),
            "target" | "node_modules" | ".cache" | ".venv" | "__pycache__" | "dist" | "build"
        ) {
            continue;
        }

        let path = entry.path();
        if path.is_dir() {
            walk_source_files(&path, extensions, latest);
        } else {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default();
            if extensions.contains(&ext)
                && let Ok(meta) = fs::metadata(&path)
                && let Ok(mtime) = meta.modified()
                && latest.is_none_or(|current| mtime > current)
            {
                *latest = Some(mtime);
            }
        }
    }
}

fn parse_build_output(system: &str, output: &str, exit_ok: bool, _exit_code: Option<i32>) -> Value {
    helpers::versioned(parse_build_output_typed(system, output, exit_ok))
}

fn parse_build_output_typed(system: &str, output: &str, exit_ok: bool) -> BuildOutput {
    let mut warnings: Vec<BuildDiagnostic> = Vec::new();
    let mut errors: Vec<BuildDiagnostic> = Vec::new();
    let mut artifact_path: Option<String> = None;

    for line in output.lines() {
        // Prefer location diagnostics before generic warning/error matching.
        if let Some(parsed) = parse_location_diagnostic_typed(line) {
            if parsed.level == "warning" {
                warnings.push(parsed);
            } else if parsed.level == "error" {
                errors.push(parsed);
            }
            continue;
        }

        // Cargo warning: warning: `foo` (bar) generated 1 warning
        // Cargo warning: warning: unused variable: `x`
        if line.starts_with("warning:") || line.contains(" warning:") {
            if let Some(parsed) = parse_compiler_diagnostic_typed(line, "warning") {
                warnings.push(parsed);
            }
            continue;
        }

        // Cargo error: error: could not compile `foo`
        if line.starts_with("error:") || line.contains(" error:") {
            if let Some(parsed) = parse_compiler_diagnostic_typed(line, "error") {
                errors.push(parsed);
            }
            continue;
        }

        // Artifact path: Finished release [optimized] target/release/navi
        if line.contains("Finished") || line.contains("Compiling") {
            // cargo
        }

        // npm/bun build: output to dist/
        if (line.contains("dist/") || line.contains("build/")) && artifact_path.is_none() {
            artifact_path = Some(line.trim().to_string());
        }
    }

    // Try to find cargo artifact
    if system == "cargo" && artifact_path.is_none() {
        for line in output.lines() {
            if line.contains("target/")
                && (line.contains("release") || line.contains("debug"))
                && let Some(path) = extract_cargo_artifact(line)
            {
                artifact_path = Some(path);
            }
        }
    }

    let status = if exit_ok { "success" } else { "error" };
    let summary = if exit_ok {
        if warnings.is_empty() {
            "build succeeded".to_string()
        } else {
            format!("build succeeded with {} warning(s)", warnings.len())
        }
    } else {
        format!(
            "build failed with {} error(s), {} warning(s)",
            errors.len(),
            warnings.len()
        )
    };

    BuildOutput {
        status: status.to_string(),
        cached: false,
        duration_ms: 0,
        warnings,
        errors,
        artifact_path,
        summary,
    }
}

#[cfg(test)]
fn parse_compiler_diagnostic(line: &str, level: &str) -> Option<Value> {
    parse_compiler_diagnostic_typed(line, level).map(helpers::versioned)
}

fn parse_compiler_diagnostic_typed(line: &str, level: &str) -> Option<BuildDiagnostic> {
    // Generic format: level: message
    let msg = line.split_once(":").map(|(_, m)| m.trim()).unwrap_or(line);

    let level = if level == "error" { "error" } else { "warning" };
    Some(BuildDiagnostic {
        file: None,
        line: None,
        column: None,
        message: msg.to_string(),
        level: level.to_string(),
        code: level.to_string(),
    })
}

#[cfg(test)]
fn parse_location_diagnostic(line: &str) -> Option<Value> {
    parse_location_diagnostic_typed(line).map(helpers::versioned)
}

fn parse_location_diagnostic_typed(line: &str) -> Option<BuildDiagnostic> {
    // Format: src/main.rs:42:5: warning: unused variable `x`
    let parts: Vec<&str> = line.splitn(5, ':').collect();
    if parts.len() >= 5 {
        let file = parts[0].trim();
        let line_num: Option<u64> = parts[1].trim().parse().ok();
        let column: Option<u64> = parts[2].trim().parse().ok();
        let level = parts[3].trim();
        if level != "warning" && level != "error" {
            return None;
        }
        let message = parts[4].trim();

        Some(BuildDiagnostic {
            file: Some(file.to_string()),
            line: line_num,
            column,
            message: message.to_string(),
            level: level.to_string(),
            code: level.to_string(),
        })
    } else {
        None
    }
}

fn extract_cargo_artifact(line: &str) -> Option<String> {
    // Look for a path after target/
    let idx = line.find("target/")?;
    let rest = &line[idx..];
    let path: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
    if path.is_empty() { None } else { Some(path) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── build_command ──────────────────────────────────────────────────────

    #[test]
    fn build_command_cargo_debug() {
        assert_eq!(build_command("cargo", "debug", None), "cargo build");
    }

    #[test]
    fn build_command_cargo_release() {
        assert_eq!(
            build_command("cargo", "release", None),
            "cargo build --release"
        );
    }

    #[test]
    fn build_command_cargo_with_features() {
        assert_eq!(
            build_command("cargo", "debug", Some("feat_a,feat_b")),
            "cargo build --features feat_a,feat_b"
        );
    }

    #[test]
    fn build_command_cargo_release_with_features() {
        assert_eq!(
            build_command("cargo", "release", Some("simd")),
            "cargo build --release --features simd"
        );
    }

    #[test]
    fn build_command_npm() {
        assert_eq!(build_command("npm", "debug", None), "npm run build");
    }

    #[test]
    fn build_command_bun() {
        assert_eq!(build_command("bun", "debug", None), "bun run build");
    }

    #[test]
    fn build_command_make() {
        assert_eq!(build_command("make", "debug", None), "make");
    }

    #[test]
    fn build_command_npm_with_features() {
        assert_eq!(
            build_command("npm", "debug", Some("--verbose")),
            "npm run build --verbose"
        );
    }

    #[test]
    fn build_command_empty_features_ignored() {
        assert_eq!(build_command("cargo", "debug", Some("")), "cargo build");
    }

    // ── parse_compiler_diagnostic ──────────────────────────────────────────

    #[test]
    fn parse_compiler_diagnostic_warning() {
        let result = parse_compiler_diagnostic("warning: unused variable `x`", "warning").unwrap();
        assert_eq!(result["code"], "warning");
        assert_eq!(result["message"], "unused variable `x`");
        assert!(result["file"].is_null());
    }

    #[test]
    fn parse_compiler_diagnostic_error() {
        let result = parse_compiler_diagnostic("error: mismatched types", "error").unwrap();
        assert_eq!(result["code"], "error");
        assert_eq!(result["message"], "mismatched types");
    }

    #[test]
    fn parse_compiler_diagnostic_cargo_generated() {
        let result =
            parse_compiler_diagnostic("warning: `foo` (lib) generated 1 warning", "warning")
                .unwrap();
        assert_eq!(result["message"], "`foo` (lib) generated 1 warning");
    }

    // ── parse_location_diagnostic ──────────────────────────────────────────

    #[test]
    fn parse_location_diagnostic_parses_file_line_message() {
        let line = "src/main.rs:42:5: warning: unused variable `x`";
        let result = parse_location_diagnostic(line).unwrap();
        assert_eq!(result["file"], "src/main.rs");
        assert_eq!(result["line"], 42);
        assert!(
            result["message"]
                .as_str()
                .unwrap()
                .contains("unused variable")
        );
    }

    #[test]
    fn parse_location_diagnostic_returns_none_for_short_line() {
        assert!(parse_location_diagnostic("just a warning").is_none());
    }

    #[test]
    fn parse_location_diagnostic_parses_error_location() {
        let line = "src/lib.rs:10:1: error: expected `;`";
        let result = parse_location_diagnostic(line).unwrap();
        assert_eq!(result["file"], "src/lib.rs");
        assert_eq!(result["line"], 10);
        assert!(result["message"].as_str().unwrap().contains("expected"));
    }

    // ── extract_cargo_artifact ─────────────────────────────────────────────

    #[test]
    fn extract_cargo_artifact_finds_path() {
        let line = "   Compiling navi v0.1.0 (/home/user/navi)";
        // This line doesn't have target/, so should return None
        assert!(extract_cargo_artifact(line).is_none());
    }

    #[test]
    fn extract_cargo_artifact_finds_target_path() {
        let line = "   Finished release [optimized] target/release/navi";
        let result = extract_cargo_artifact(line).unwrap();
        assert_eq!(result, "target/release/navi");
    }

    #[test]
    fn extract_cargo_artifact_finds_debug_path() {
        let line = "   Finished dev [unoptimized + debuginfo] target/debug/navi";
        let result = extract_cargo_artifact(line).unwrap();
        assert_eq!(result, "target/debug/navi");
    }

    #[test]
    fn extract_cargo_artifact_returns_none_without_target() {
        assert!(extract_cargo_artifact("no artifact here").is_none());
    }

    // ── parse_build_output ─────────────────────────────────────────────────

    #[test]
    fn parse_build_output_success_no_warnings() {
        let output =
            "   Compiling navi v0.1.0\n   Finished dev [unoptimized + debuginfo] target/debug/navi";
        let result = parse_build_output("cargo", output, true, Some(0));
        assert_eq!(result["schema_version"], 1);
        assert_eq!(result["status"], "success");
        assert_eq!(result["cached"], false);
        assert!(result["warnings"].as_array().unwrap().is_empty());
        assert!(result["errors"].as_array().unwrap().is_empty());
        assert_eq!(result["summary"], "build succeeded");
    }

    #[test]
    fn parse_build_output_success_with_warnings() {
        let output = "warning: unused variable `x`\n   Finished dev target/debug/navi";
        let result = parse_build_output("cargo", output, true, Some(0));
        assert_eq!(result["status"], "success");
        assert_eq!(result["warnings"].as_array().unwrap().len(), 1);
        assert_eq!(result["summary"], "build succeeded with 1 warning(s)");
    }

    #[test]
    fn parse_build_output_failure_with_errors() {
        let output = "error: mismatched types\nwarning: unused variable `x`";
        let result = parse_build_output("cargo", output, false, Some(101));
        assert_eq!(result["status"], "error");
        assert_eq!(result["errors"].as_array().unwrap().len(), 1);
        assert_eq!(result["warnings"].as_array().unwrap().len(), 1);
        assert_eq!(
            result["summary"],
            "build failed with 1 error(s), 1 warning(s)"
        );
    }

    #[test]
    fn parse_build_output_with_location_diagnostics() {
        // This line matches " warning:" check first, so it's parsed by parse_compiler_diagnostic
        // which returns file=null, line=null (generic diagnostic)
        let output = "src/main.rs:42:5: warning: unused variable `x`";
        let result = parse_build_output("cargo", output, true, Some(0));
        assert_eq!(result["warnings"].as_array().unwrap().len(), 1);
        // The first match is the " warning:" check, which produces a generic diagnostic
        let warning = &result["warnings"][0];
        assert_eq!(warning["file"], "src/main.rs");
        assert_eq!(warning["line"], 42);
        assert_eq!(warning["column"], 5);
        assert!(
            warning["message"]
                .as_str()
                .unwrap()
                .contains("unused variable")
        );
    }

    #[test]
    fn parse_build_output_extracts_artifact() {
        let output = "   Finished release [optimized] target/release/navi";
        let result = parse_build_output("cargo", output, true, Some(0));
        assert_eq!(result["artifact_path"], "target/release/navi");
    }

    #[test]
    fn parse_build_output_npm_dist() {
        let output = "Build complete! Output to dist/index.js";
        let result = parse_build_output("npm", output, true, Some(0));
        assert!(result["artifact_path"].as_str().is_some());
    }

    // ── Mutation-killing: parse_build_output_typed ────────────────────────

    #[test]
    fn parse_build_output_generic_warning() {
        let output = "warning: something happened";
        let result = parse_build_output("cargo", output, true, Some(0));
        assert_eq!(result["warnings"].as_array().unwrap().len(), 1);
        assert_eq!(result["warnings"][0]["file"], serde_json::Value::Null);
        assert_eq!(result["warnings"][0]["code"], "warning");
    }

    #[test]
    fn parse_build_output_generic_error() {
        let output = "error: something broke";
        let result = parse_build_output("cargo", output, false, Some(1));
        assert_eq!(result["errors"].as_array().unwrap().len(), 1);
        assert_eq!(result["errors"][0]["file"], serde_json::Value::Null);
        assert_eq!(result["errors"][0]["code"], "error");
    }

    #[test]
    fn parse_build_output_error_with_warning_prefix() {
        let output = "src/main.rs:10:1: error: expected `;`";
        let result = parse_build_output("cargo", output, false, Some(1));
        assert_eq!(result["errors"].as_array().unwrap().len(), 1);
        assert_eq!(result["errors"][0]["file"], "src/main.rs");
    }

    #[test]
    fn parse_build_output_non_diagnostic_line_ignored() {
        let output = "Compiling crate v0.1.0\nFinished dev target/debug/app";
        let result = parse_build_output("cargo", output, true, Some(0));
        assert!(result["warnings"].as_array().unwrap().is_empty());
        assert!(result["errors"].as_array().unwrap().is_empty());
    }

    // ── Mutation-killing: location diagnostic rejects non-warning/error ──

    #[test]
    fn parse_location_diagnostic_rejects_info_level() {
        let line = "src/main.rs:1:1: info: something happened";
        assert!(parse_location_diagnostic_typed(line).is_none());
    }
}
