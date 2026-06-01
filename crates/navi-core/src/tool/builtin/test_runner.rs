use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

const _TEST_DEFAULT_TIMEOUT_MS: u64 = 120_000;
const TEST_MAX_TIMEOUT_MS: u64 = 600_000;
const TEST_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq)]
struct TestFailure {
    test_name: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct TestRunOutput {
    status: &'static str,
    framework: &'static str,
    total: u64,
    passed: u64,
    failed: u64,
    skipped: u64,
    duration_ms: u64,
    failures: Vec<TestFailure>,
    summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_output: Option<String>,
}

pub(crate) struct TestRunnerTool;

#[async_trait]
impl Tool for TestRunnerTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "test_runner",
            "Run project tests with structured output. Auto-detects framework (cargo test, npm test, bun test, pytest, go test). Returns pass/fail counts, failure details, and duration instead of raw stdout.",
            ToolKind::Command,
            json!({
                "type": "object",
                "properties": {
                    "test_path": {
                        "type": "string",
                        "description": "Specific test file or filter to run. Omit to run all tests."
                    },
                    "flags": {
                        "type": "string",
                        "description": "Extra flags to pass to the test runner (e.g. --release, -p crate_name)."
                    }
                },
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let test_path = helpers::optional_string(&invocation.input, "test_path");
        let flags = helpers::optional_string(&invocation.input, "flags");

        let framework = detect_test_framework().await?;
        let command = build_test_command(&framework, test_path.as_deref(), flags.as_deref());

        let timeout_ms = TEST_MAX_TIMEOUT_MS;

        let mut child = tokio::process::Command::new("bash")
            .arg("-lc")
            .arg(&command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to spawn test command: {command}"))?;

        let stdout_data = Arc::new(Mutex::new(Vec::new()));
        let stderr_data = Arc::new(Mutex::new(Vec::new()));

        spawn_reader(child.stdout.take().unwrap(), stdout_data.clone());
        spawn_reader(child.stderr.take().unwrap(), stderr_data.clone());

        let timeout_duration = Duration::from_millis(timeout_ms);
        let status_result = tokio::time::timeout(timeout_duration, child.wait()).await;

        let (exit_ok, exit_code, error_msg) = match status_result {
            Ok(Ok(status)) => (status.success(), status.code(), None),
            Ok(Err(e)) => (false, None, Some(format!("failed to wait for tests: {e}"))),
            Err(_) => (false, None, Some("test command timed out".to_string())),
        };

        tokio::time::sleep(Duration::from_millis(50)).await;

        let stdout_bytes = stdout_data.lock().await.clone();
        let stderr_bytes = stderr_data.lock().await.clone();
        let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
        let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();

        if let Some(err) = error_msg {
            let mut output = helpers::tool_error(
                "test_command_failed",
                err,
                true,
                Some("Retry with a narrower test_path or higher timeout if the command timed out."),
                Some(helpers::truncate_string(stderr, TEST_OUTPUT_LIMIT_BYTES)),
            );
            if let Value::Object(ref mut object) = output {
                object.insert("framework".to_string(), json!(framework));
                object.insert(
                    "stdout".to_string(),
                    json!(helpers::truncate_string(stdout, TEST_OUTPUT_LIMIT_BYTES)),
                );
            }
            return Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output,
            });
        }

        let combined = format!("{stdout}\n{stderr}");
        let result = parse_test_output(&framework, &combined, exit_ok, exit_code);

        Ok(helpers::ok(invocation.id, result))
    }
}

fn spawn_reader<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    reader: R,
    buffer: Arc<Mutex<Vec<u8>>>,
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

async fn detect_test_framework() -> Result<String> {
    let cwd = std::env::current_dir().context("failed to get cwd")?;

    if Path::new("Cargo.toml").exists() {
        return Ok("cargo".to_string());
    }
    if Path::new("go.mod").exists() {
        return Ok("go".to_string());
    }
    if Path::new("pyproject.toml").exists()
        || Path::new("setup.py").exists()
        || Path::new("pytest.ini").exists()
    {
        return Ok("pytest".to_string());
    }
    if Path::new("package.json").exists() {
        let content = std::fs::read_to_string("package.json").unwrap_or_default();
        if content.contains("vitest") {
            return Ok("vitest".to_string());
        }
        if content.contains("jest") {
            return Ok("jest".to_string());
        }
        if content.contains("bun") || Path::new("bun.lockb").exists() {
            return Ok("bun".to_string());
        }
        return Ok("npm".to_string());
    }

    anyhow::bail!("no test framework detected in {}", cwd.display());
}

fn build_test_command(framework: &str, test_path: Option<&str>, flags: Option<&str>) -> String {
    let mut cmd = match framework {
        "cargo" => "cargo test".to_string(),
        "npm" => "npm test".to_string(),
        "bun" => "bun test".to_string(),
        "jest" => "npx jest".to_string(),
        "vitest" => "npx vitest run".to_string(),
        "pytest" => "pytest -x -q".to_string(),
        "go" => "go test ./...".to_string(),
        _ => "cargo test".to_string(),
    };

    if let Some(path) = test_path {
        match framework {
            "cargo" => cmd.push_str(&format!(" {path}")),
            "pytest" => cmd.push_str(&format!(" {path}")),
            "go" => cmd.push_str(&format!(" {path}")),
            "jest" | "vitest" => cmd.push_str(&format!(" {path}")),
            _ => cmd.push_str(&format!(" -- {path}")),
        }
    }

    if let Some(f) = flags.filter(|f| !f.is_empty()) {
        cmd.push(' ');
        cmd.push_str(f);
    }

    cmd
}

fn parse_test_output(
    framework: &str,
    output: &str,
    exit_ok: bool,
    exit_code: Option<i32>,
) -> Value {
    match framework {
        "cargo" => parse_cargo_test(output, exit_ok, exit_code),
        "jest" | "vitest" | "bun" | "npm" => parse_js_test(output, exit_ok, exit_code),
        "pytest" => parse_pytest(output, exit_ok, exit_code),
        "go" => parse_go_test(output, exit_ok, exit_code),
        _ => parse_generic(output, exit_ok, exit_code),
    }
}

fn parse_cargo_test(output: &str, exit_ok: bool, _exit_code: Option<i32>) -> Value {
    helpers::versioned(parse_cargo_test_output(output, exit_ok))
}

fn parse_cargo_test_output(output: &str, exit_ok: bool) -> TestRunOutput {
    let mut total = 0u64;
    let mut passed = 0u64;
    let mut failed = 0u64;
    let mut ignored = 0u64;
    let mut failures: Vec<TestFailure> = Vec::new();
    let mut duration_ms = 0u64;

    for line in output.lines() {
        // Match: test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.42s
        if line.contains("test result:") {
            let parts: Vec<&str> = line.split(';').collect();
            for part in &parts {
                let part = part.trim();
                if let Some(n) = extract_count(part, "passed") {
                    passed = n;
                }
                if let Some(n) = extract_count(part, "failed") {
                    failed = n;
                }
                if let Some(n) = extract_count(part, "ignored") {
                    ignored = n;
                }
                if let Some(secs) = extract_duration_secs(part) {
                    duration_ms = secs;
                }
            }
            total = passed + failed + ignored;
        }

        // Match: thread 'test_name' panicked at 'message'
        if line.contains("panicked at")
            && let Some((name, msg)) = parse_panic_line(line)
        {
            failures.push(TestFailure {
                test_name: name,
                message: msg,
                location: parse_panic_location(line),
            });
        }

        // Match: ---- test_name stdout ----
        if line.starts_with("---- ") && line.ends_with(" stdout ----") {
            let name = line
                .strip_prefix("---- ")
                .and_then(|s| s.strip_suffix(" stdout ----"))
                .unwrap_or("");
            if !failures.iter().any(|f| f.test_name == name) {
                failures.push(TestFailure {
                    test_name: name.to_string(),
                    message: String::new(),
                    location: None,
                });
            }
        }
    }

    let status = if exit_ok { "pass" } else { "fail" };
    let summary = if total == 0 && exit_ok {
        "tests passed".to_string()
    } else if failed > 0 {
        format!("{passed}/{total} passed, {failed} failed")
    } else {
        format!("{total}/{total} passed")
    };

    TestRunOutput {
        status,
        framework: "cargo",
        total,
        passed,
        failed,
        skipped: ignored,
        duration_ms,
        failures,
        summary,
        raw_output: None,
    }
}

fn parse_js_test(output: &str, exit_ok: bool, _exit_code: Option<i32>) -> Value {
    helpers::versioned(parse_js_test_output(output, exit_ok))
}

fn parse_js_test_output(output: &str, exit_ok: bool) -> TestRunOutput {
    let mut total = 0u64;
    let mut passed = 0u64;
    let mut failed = 0u64;
    let mut skipped = 0u64;
    let mut failures: Vec<TestFailure> = Vec::new();

    for line in output.lines() {
        // Jest/vitest: Tests: 2 failed, 5 passed, 7 total
        if line.contains("Tests:") && line.contains("total") {
            if let Some(n) = extract_count(line, "passed") {
                passed = n;
            }
            if let Some(n) = extract_count(line, "failed") {
                failed = n;
            }
            if let Some(n) = extract_count(line, "skipped") {
                skipped = n;
            }
            total = passed + failed + skipped;
        }

        // Jest: FAIL src/foo.test.ts
        if line.starts_with("FAIL ") {
            let file = line.strip_prefix("FAIL ").unwrap_or("").trim();
            failures.push(TestFailure {
                test_name: file.to_string(),
                message: String::new(),
                location: None,
            });
        }

        // Vitest: ❌ test_name
        if line.contains("FAIL") || line.contains("AssertionError") {
            failures.push(TestFailure {
                test_name: line.trim().to_string(),
                message: String::new(),
                location: None,
            });
        }
    }

    // npm test may just show exit code
    if total == 0 {
        total = passed + failed + skipped;
    }

    let status = if exit_ok { "pass" } else { "fail" };
    let summary = if total == 0 && exit_ok {
        "tests passed".to_string()
    } else if failed > 0 {
        format!("{passed}/{total} passed, {failed} failed")
    } else {
        format!("{total}/{total} passed")
    };

    TestRunOutput {
        status,
        framework: "js",
        total,
        passed,
        failed,
        skipped,
        duration_ms: 0,
        failures,
        summary,
        raw_output: None,
    }
}

fn parse_pytest(output: &str, exit_ok: bool, _exit_code: Option<i32>) -> Value {
    helpers::versioned(parse_pytest_output(output, exit_ok))
}

fn parse_pytest_output(output: &str, exit_ok: bool) -> TestRunOutput {
    let mut total = 0u64;
    let mut passed = 0u64;
    let mut failed = 0u64;
    let mut skipped = 0u64;
    let mut failures: Vec<TestFailure> = Vec::new();

    for line in output.lines() {
        // pytest -q: 5 passed, 2 failed in 3.42s
        // pytest: === 5 passed, 2 failed in 3.42s ===
        if line.contains("passed") || line.contains("failed") {
            if let Some(n) = extract_count(line, "passed") {
                passed = n;
            }
            if let Some(n) = extract_count(line, "failed") {
                failed = n;
            }
            if let Some(n) = extract_count(line, "skipped") {
                skipped = n;
            }
            total = passed + failed + skipped;
        }

        // FAILED tests/test_foo.py::test_bar - AssertionError: ...
        if line.starts_with("FAILED ") {
            let parts: Vec<&str> = line.splitn(3, " - ").collect();
            let test_name = parts[0].strip_prefix("FAILED ").unwrap_or("").trim();
            let message = parts.get(1).unwrap_or(&"").trim();
            failures.push(TestFailure {
                test_name: test_name.to_string(),
                message: message.to_string(),
                location: None,
            });
        }

        // Short failure: F test_name
        if line.starts_with("F ") && !line.starts_with("FAILED") {
            let test_name = line.strip_prefix("F ").unwrap_or("").trim();
            failures.push(TestFailure {
                test_name: test_name.to_string(),
                message: String::new(),
                location: None,
            });
        }
    }

    let status = if exit_ok { "pass" } else { "fail" };
    let summary = if total == 0 && exit_ok {
        "tests passed".to_string()
    } else if failed > 0 {
        format!("{passed}/{total} passed, {failed} failed")
    } else {
        format!("{total}/{total} passed")
    };

    TestRunOutput {
        status,
        framework: "pytest",
        total,
        passed,
        failed,
        skipped,
        duration_ms: 0,
        failures,
        summary,
        raw_output: None,
    }
}

fn parse_go_test(output: &str, exit_ok: bool, _exit_code: Option<i32>) -> Value {
    helpers::versioned(parse_go_test_output(output, exit_ok))
}

fn parse_go_test_output(output: &str, exit_ok: bool) -> TestRunOutput {
    let mut total = 0u64;
    let mut passed = 0u64;
    let mut failed = 0u64;
    let mut failures: Vec<TestFailure> = Vec::new();

    for line in output.lines() {
        // ok  	pkg/name	0.123s
        if line.starts_with("ok") {
            passed += 1;
            total += 1;
        }

        // FAIL	pkg/name	0.123s
        if line.starts_with("FAIL\t") || line == "FAIL" || line.starts_with("FAIL ") {
            failed += 1;
            total += 1;
        }

        // --- FAIL: TestName (0.00s)
        if line.starts_with("--- FAIL:") {
            let name = line
                .strip_prefix("--- FAIL: ")
                .and_then(|s| s.split_whitespace().next())
                .unwrap_or("");
            failures.push(TestFailure {
                test_name: name.to_string(),
                message: String::new(),
                location: None,
            });
        }
    }

    let status = if exit_ok { "pass" } else { "fail" };
    let summary = if total == 0 && exit_ok {
        "tests passed".to_string()
    } else if failed > 0 {
        format!("{passed}/{total} passed, {failed} failed")
    } else {
        format!("{total}/{total} passed")
    };

    TestRunOutput {
        status,
        framework: "go",
        total,
        passed,
        failed,
        skipped: 0,
        duration_ms: 0,
        failures,
        summary,
        raw_output: None,
    }
}

fn parse_generic(output: &str, exit_ok: bool, _exit_code: Option<i32>) -> Value {
    helpers::versioned(parse_generic_output(output, exit_ok))
}

fn parse_generic_output(output: &str, exit_ok: bool) -> TestRunOutput {
    let status = if exit_ok { "pass" } else { "fail" };
    TestRunOutput {
        status,
        framework: "unknown",
        total: 0,
        passed: 0,
        failed: 0,
        skipped: 0,
        duration_ms: 0,
        failures: Vec::new(),
        summary: if exit_ok {
            "tests passed"
        } else {
            "tests failed"
        }
        .to_string(),
        raw_output: Some(helpers::truncate_string(
            output.to_string(),
            TEST_OUTPUT_LIMIT_BYTES,
        )),
    }
}

fn extract_count(text: &str, label: &str) -> Option<u64> {
    // Matches "7 passed", "2 failed", etc.
    let idx = text.find(label)?;
    let prefix = &text[..idx].trim_end();
    prefix.split_whitespace().last()?.parse().ok()
}

fn extract_duration_secs(text: &str) -> Option<u64> {
    // Matches "finished in 3.42s"
    let idx = text.find("finished in ")?;
    let rest = &text[idx + "finished in ".len()..];
    let num_str: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let secs: f64 = num_str.parse().ok()?;
    Some((secs * 1000.0) as u64)
}

fn parse_panic_line(line: &str) -> Option<(String, String)> {
    // thread 'test_name' panicked at 'message'
    let name_start = line.find("'")? + 1;
    let name_end = line[name_start..].find("'")? + name_start;
    let name = &line[name_start..name_end];

    let msg_start = line[name_end..].find("panicked at ")?;
    let msg_rest = line[name_end + msg_start + "panicked at ".len()..].trim();
    let msg = quoted_prefix(msg_rest)
        .or_else(|| msg_rest.split_once(", ").map(|(message, _)| message.trim()))
        .unwrap_or(msg_rest)
        .trim_matches(|c| c == '\'' || c == '"')
        .trim();

    Some((name.to_string(), msg.to_string()))
}

fn quoted_prefix(text: &str) -> Option<&str> {
    let quote = text.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let end = text[quote.len_utf8()..].find(quote)? + quote.len_utf8();
    Some(&text[..=end])
}

fn parse_panic_location(line: &str) -> Option<String> {
    line.rsplit_once(", ")
        .map(|(_, location)| location.trim())
        .filter(|location| location.contains(':'))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── extract_count ──────────────────────────────────────────────────────

    #[test]
    fn extract_count_parses_passed() {
        assert_eq!(extract_count("7 passed", "passed"), Some(7));
    }

    #[test]
    fn extract_count_parses_failed() {
        assert_eq!(extract_count("2 failed", "failed"), Some(2));
    }

    #[test]
    fn extract_count_parses_ignored() {
        assert_eq!(extract_count("3 ignored", "ignored"), Some(3));
    }

    #[test]
    fn extract_count_returns_none_for_missing_label() {
        assert_eq!(extract_count("7 passed", "failed"), None);
    }

    #[test]
    fn extract_count_returns_none_for_no_number() {
        assert_eq!(extract_count("passed", "passed"), None);
    }

    #[test]
    fn extract_count_handles_surrounding_text() {
        assert_eq!(
            extract_count("test result: ok. 12 passed; 0 failed", "passed"),
            Some(12)
        );
    }

    // ── extract_duration_secs ──────────────────────────────────────────────

    #[test]
    fn extract_duration_parses_seconds() {
        assert_eq!(extract_duration_secs("finished in 3.42s"), Some(3420));
    }

    #[test]
    fn extract_duration_parses_subsecond() {
        assert_eq!(extract_duration_secs("finished in 0.05s"), Some(50));
    }

    #[test]
    fn extract_duration_returns_none_without_marker() {
        assert_eq!(extract_duration_secs("completed in 1s"), None);
    }

    // ── parse_panic_line ───────────────────────────────────────────────────

    #[test]
    fn parse_panic_extracts_name_and_message() {
        let line = "thread 'test_foo' panicked at 'assertion failed', src/lib.rs:42";
        let (name, msg) = parse_panic_line(line).unwrap();
        assert_eq!(name, "test_foo");
        assert_eq!(msg, "assertion failed");
        assert_eq!(parse_panic_location(line).as_deref(), Some("src/lib.rs:42"));
    }

    #[test]
    fn parse_panic_handles_double_quotes() {
        let line = r#"thread 'test_bar' panicked at "boom", src/lib.rs:10"#;
        let (name, msg) = parse_panic_line(line).unwrap();
        assert_eq!(name, "test_bar");
        assert_eq!(msg, "boom");
        assert_eq!(parse_panic_location(line).as_deref(), Some("src/lib.rs:10"));
    }

    #[test]
    fn parse_panic_returns_none_for_non_panic_line() {
        assert!(parse_panic_line("some random line").is_none());
    }

    // ── build_test_command ─────────────────────────────────────────────────

    #[test]
    fn build_test_command_cargo_no_path() {
        assert_eq!(build_test_command("cargo", None, None), "cargo test");
    }

    #[test]
    fn build_test_command_cargo_with_path() {
        assert_eq!(
            build_test_command("cargo", Some("my_test"), None),
            "cargo test my_test"
        );
    }

    #[test]
    fn build_test_command_cargo_with_flags() {
        assert_eq!(
            build_test_command("cargo", None, Some("--release")),
            "cargo test --release"
        );
    }

    #[test]
    fn build_test_command_pytest_with_path() {
        assert_eq!(
            build_test_command("pytest", Some("test_foo.py"), None),
            "pytest -x -q test_foo.py"
        );
    }

    #[test]
    fn build_test_command_go_with_path() {
        assert_eq!(
            build_test_command("go", Some("./pkg/..."), None),
            "go test ./... ./pkg/..."
        );
    }

    #[test]
    fn build_test_command_npm_uses_separators() {
        assert_eq!(
            build_test_command("npm", Some("auth"), None),
            "npm test -- auth"
        );
    }

    #[test]
    fn build_test_command_jest_with_path() {
        assert_eq!(
            build_test_command("jest", Some("login.test.ts"), None),
            "npx jest login.test.ts"
        );
    }

    #[test]
    fn build_test_command_vitest_with_path() {
        assert_eq!(
            build_test_command("vitest", Some("user.spec.ts"), None),
            "npx vitest run user.spec.ts"
        );
    }

    // ── parse_test_output dispatch ────────────────────────────────────────

    #[test]
    fn parse_test_output_dispatches_cargo() {
        let result = parse_test_output(
            "cargo",
            "test result: ok. 1 passed; 0 failed; finished in 0.01s",
            true,
            Some(0),
        );
        assert_eq!(result["framework"], "cargo");
    }

    #[test]
    fn parse_test_output_dispatches_js() {
        let result = parse_test_output("jest", "Tests: 1 passed, 1 total", true, Some(0));
        assert_eq!(result["framework"], "js");
    }

    #[test]
    fn parse_test_output_dispatches_pytest() {
        let result = parse_test_output("pytest", "1 passed in 0.01s", true, Some(0));
        assert_eq!(result["framework"], "pytest");
    }

    #[test]
    fn parse_test_output_dispatches_go() {
        let result = parse_test_output("go", "ok  \tpkg\t0.01s", true, Some(0));
        assert_eq!(result["framework"], "go");
    }

    #[test]
    fn parse_test_output_dispatches_unknown() {
        let result = parse_test_output("unknown", "some output", true, Some(0));
        assert_eq!(result["framework"], "unknown");
    }

    // ── build_test_command missing arms ───────────────────────────────────

    #[test]
    fn build_test_command_bun_no_path() {
        assert_eq!(build_test_command("bun", None, None), "bun test");
    }

    #[test]
    fn build_test_command_vitest_no_path() {
        assert_eq!(build_test_command("vitest", None, None), "npx vitest run");
    }

    #[test]
    fn build_test_command_go_no_path() {
        assert_eq!(build_test_command("go", None, None), "go test ./...");
    }

    // ── parse_cargo_test ───────────────────────────────────────────────────

    #[test]
    fn parse_cargo_all_passed() {
        let output = "running 5 tests\ntest a ... ok\ntest b ... ok\n\ntest result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s";
        let result = parse_cargo_test(output, true, Some(0));
        assert_eq!(result["status"], "pass");
        assert_eq!(result["schema_version"], 1);
        assert_eq!(result["total"], 5);
        assert_eq!(result["passed"], 5);
        assert_eq!(result["failed"], 0);
        assert_eq!(result["summary"], "5/5 passed");
    }

    #[test]
    fn parse_cargo_with_failures() {
        let output = "thread 'test_bar' panicked at 'boom', src/lib.rs:10\n---- test_bar stdout ----\ntest result: FAILED. 3 passed; 2 failed; 0 ignored; finished in 1.50s";
        let result = parse_cargo_test(output, false, Some(101));
        assert_eq!(result["status"], "fail");
        assert_eq!(result["passed"], 3);
        assert_eq!(result["failed"], 2);
        assert_eq!(result["total"], 5);
        assert!(!result["failures"].as_array().unwrap().is_empty());
        assert_eq!(result["summary"], "3/5 passed, 2 failed");
    }

    #[test]
    fn parse_cargo_duration_extracted() {
        let output = "test result: ok. 1 passed; 0 failed; 0 ignored; finished in 2.50s";
        let result = parse_cargo_test(output, true, Some(0));
        assert_eq!(result["duration_ms"], 2500);
    }

    #[test]
    fn parse_cargo_with_ignored() {
        let output = "test result: ok. 3 passed; 0 failed; 2 ignored; finished in 0.10s";
        let result = parse_cargo_test(output, true, Some(0));
        assert_eq!(result["passed"], 3);
        assert_eq!(result["skipped"], 2);
        assert_eq!(result["total"], 5);
    }

    // ── parse_js_test ──────────────────────────────────────────────────────

    #[test]
    fn parse_js_jest_all_passed() {
        let output = "Tests: 5 passed, 5 total";
        let result = parse_js_test(output, true, Some(0));
        assert_eq!(result["status"], "pass");
        assert_eq!(result["total"], 5);
        assert_eq!(result["passed"], 5);
        assert_eq!(result["failed"], 0);
    }

    #[test]
    fn parse_js_jest_with_failures() {
        let output = "FAIL src/auth.test.ts\nTests: 2 failed, 5 passed, 7 total";
        let result = parse_js_test(output, false, Some(1));
        assert_eq!(result["status"], "fail");
        assert_eq!(result["total"], 7);
        assert_eq!(result["passed"], 5);
        assert_eq!(result["failed"], 2);
        assert!(!result["failures"].as_array().unwrap().is_empty());
    }

    #[test]
    fn parse_js_with_skipped() {
        let output = "Tests: 2 skipped, 3 passed, 5 total";
        let result = parse_js_test(output, true, Some(0));
        assert_eq!(result["passed"], 3);
        assert_eq!(result["skipped"], 2);
        assert_eq!(result["total"], 5);
    }

    // ── parse_pytest ───────────────────────────────────────────────────────

    #[test]
    fn parse_pytest_all_passed() {
        let output = "5 passed in 0.12s";
        let result = parse_pytest(output, true, Some(0));
        assert_eq!(result["status"], "pass");
        assert_eq!(result["total"], 5);
        assert_eq!(result["passed"], 5);
        assert_eq!(result["failed"], 0);
    }

    #[test]
    fn parse_pytest_with_failures() {
        let output = "FAILED tests/test_auth.py::test_login - AssertionError: expected true\n3 passed, 1 failed in 0.50s";
        let result = parse_pytest(output, false, Some(1));
        assert_eq!(result["status"], "fail");
        assert_eq!(result["passed"], 3);
        assert_eq!(result["failed"], 1);
        assert_eq!(result["total"], 4);
        let failures = result["failures"].as_array().unwrap();
        assert!(
            failures
                .iter()
                .any(|f| f["test_name"].as_str().unwrap().contains("test_login"))
        );
    }

    #[test]
    fn parse_pytest_with_skipped() {
        let output = "4 passed, 1 skipped in 0.20s";
        let result = parse_pytest(output, true, Some(0));
        assert_eq!(result["passed"], 4);
        assert_eq!(result["skipped"], 1);
        assert_eq!(result["total"], 5);
    }

    // ── parse_go_test ──────────────────────────────────────────────────────

    #[test]
    fn parse_go_all_passed() {
        let output = "ok  \tpkg/foo\t0.123s\nok  \tpkg/bar\t0.456s";
        let result = parse_go_test(output, true, Some(0));
        assert_eq!(result["status"], "pass");
        assert_eq!(result["total"], 2);
        assert_eq!(result["passed"], 2);
        assert_eq!(result["failed"], 0);
    }

    #[test]
    fn parse_go_with_failure() {
        let output = "ok  \tpkg/foo\t0.123s\n--- FAIL: TestLogin (0.00s)\nFAIL\tpkg/bar\t0.100s";
        let result = parse_go_test(output, false, Some(1));
        assert_eq!(result["status"], "fail");
        assert_eq!(result["passed"], 1);
        assert_eq!(result["failed"], 1);
        assert_eq!(result["total"], 2);
        let failures = result["failures"].as_array().unwrap();
        assert!(failures.iter().any(|f| f["test_name"] == "TestLogin"));
    }

    #[test]
    fn parse_go_empty_output() {
        let result = parse_go_test("", true, Some(0));
        assert_eq!(result["total"], 0);
        assert_eq!(result["summary"], "tests passed");
    }

    // ── Mutation-killing: cargo failure dedup ─────────────────────────────

    #[test]
    fn parse_cargo_deduplicates_failure_names() {
        let output = "---- test_bar stdout ----\n---- test_bar stdout ----\ntest result: ok. 0 passed; 0 failed; finished in 0.01s";
        let result = parse_cargo_test(output, true, Some(0));
        let failures = result["failures"].as_array().unwrap();
        assert_eq!(failures.len(), 1);
    }

    #[test]
    fn parse_cargo_failure_with_panic_preserves_location() {
        let output = "thread 'test_foo' panicked at 'boom', src/lib.rs:10\ntest result: FAILED. 0 passed; 1 failed; finished in 0.01s";
        let result = parse_cargo_test(output, false, Some(101));
        let failures = result["failures"].as_array().unwrap();
        assert_eq!(failures[0]["location"], "src/lib.rs:10");
    }

    // ── Mutation-killing: JS AssertionError ───────────────────────────────

    #[test]
    fn parse_js_captures_assertion_error_without_fail_prefix() {
        let output =
            "AssertionError: expected true to be false\nTests: 1 failed, 0 passed, 1 total";
        let result = parse_js_test(output, false, Some(1));
        let failures = result["failures"].as_array().unwrap();
        assert!(
            failures
                .iter()
                .any(|f| f["test_name"].as_str().unwrap().contains("AssertionError"))
        );
    }

    #[test]
    fn parse_js_total_equals_passed_plus_failed_plus_skipped() {
        let output = "Tests: 1 failed, 2 passed, 1 skipped, 4 total";
        let result = parse_js_test(output, false, Some(1));
        assert_eq!(result["total"], 4);
        assert_eq!(result["passed"], 2);
        assert_eq!(result["failed"], 1);
        assert_eq!(result["skipped"], 1);
    }

    // ── Mutation-killing: pytest FAILED vs F ──────────────────────────────

    #[test]
    fn parse_pytest_short_failure_line() {
        let output = "F test_short_failure\n1 failed in 0.10s";
        let result = parse_pytest(output, false, Some(1));
        let failures = result["failures"].as_array().unwrap();
        assert!(
            failures
                .iter()
                .any(|f| f["test_name"] == "test_short_failure")
        );
    }

    #[test]
    fn parse_pytest_failed_line_not_confused_with_f() {
        let output =
            "FAILED tests/test.py::test_login - AssertionError: expected true\n1 failed in 0.10s";
        let result = parse_pytest(output, false, Some(1));
        let failures = result["failures"].as_array().unwrap();
        assert_eq!(failures.len(), 1);
        assert!(
            failures[0]["test_name"]
                .as_str()
                .unwrap()
                .contains("test_login")
        );
    }

    // ── Mutation-killing: go multiple failures ────────────────────────────

    #[test]
    fn parse_go_multiple_failures() {
        let output = "ok  \tpkg/foo\t0.123s\n--- FAIL: TestA (0.00s)\n--- FAIL: TestB (0.00s)\nFAIL\tpkg/bar\t0.100s";
        let result = parse_go_test(output, false, Some(1));
        assert_eq!(result["passed"], 1);
        assert_eq!(result["failed"], 1);
        assert_eq!(result["total"], 2);
        let failures = result["failures"].as_array().unwrap();
        assert_eq!(failures.len(), 2);
        assert_eq!(failures[0]["test_name"], "TestA");
        assert_eq!(failures[1]["test_name"], "TestB");
    }

    // ── Mutation-killing: quoted_prefix ───────────────────────────────────

    #[test]
    fn quoted_prefix_returns_none_for_non_quote() {
        assert_eq!(quoted_prefix("no quotes here"), None);
    }

    #[test]
    fn quoted_prefix_extracts_single_quoted() {
        assert_eq!(quoted_prefix("'hello', world"), Some("'hello'"));
    }

    #[test]
    fn quoted_prefix_extracts_double_quoted() {
        assert_eq!(quoted_prefix("\"boom\", src/lib.rs"), Some("\"boom\""));
    }
}
