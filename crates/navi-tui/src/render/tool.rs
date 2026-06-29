use navi_sdk::{ToolInvocation, ToolResult};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const MAX_TOOL_RENDER_LINES: usize = 5000;

fn truncate_to_lines(text: &str, max_lines: usize) -> &str {
    let mut count = 0;
    for (i, ch) in text.char_indices() {
        if ch == '\n' {
            count += 1;
            if count >= max_lines {
                return &text[..=i];
            }
        }
    }
    text
}

pub(crate) fn tool_compact_text(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let mut text = match invocation.tool_name.as_str() {
        // ── Existing (kept) ──────────────────────────────────────────────
        "read_file" | "view_file" => read_file_summary(invocation, result),
        "write_file" => write_file_summary(invocation, result),
        "apply_patch" => apply_patch_summary(invocation),
        "write"
            if invocation.input.get("patch").is_some()
                || invocation.input.get("patches").is_some() =>
        {
            apply_patch_summary(invocation)
        }
        // Direct write (path + content)
        "write" => write_file_summary(invocation, result),
        "bash" => bash_summary(invocation, result),
        "grep" => grep_summary(invocation, result),
        "fs_browser" => fs_browser_summary(invocation, result),

        // ── Process & Command ─────────────────────────────────────────────
        "process" => process_summary(invocation, result),
        "test_runner" => test_runner_summary(invocation, result),
        "build_runner" => build_runner_summary(invocation, result),

        // ── Code Intelligence ─────────────────────────────────────────────
        "code" => code_summary(invocation, result),
        "code_edit" => code_edit_summary(invocation, result),
        "code_exec" => code_exec_summary(invocation, result),
        "ast_search" => ast_search_summary(invocation, result),
        "symbol_goto" => symbol_goto_summary(invocation, result),
        "symbol_references" => symbol_references_summary(invocation, result),
        "dependency_graph_query" => dependency_graph_summary(result),
        "test_discovery" => test_discovery_summary(result),
        "ownership_churn_query" => churn_summary(result),

        // ── Repo Search Aliases ───────────────────────────────────────────
        "search" | "list_dir" | "glob" => search_tool_summary(invocation, result),

        // ── Repo Explore & Subagent ───────────────────────────────────────
        "repo_explore" => repo_explore_summary(invocation, result),
        "subagent" => subagent_summary(invocation, result),

        // ── Planning & Session ────────────────────────────────────────────
        "plan" => plan_summary(invocation, result),
        "init_session" => init_session_summary(result),
        "mark_feature_done" => mark_feature_done_summary(result),

        // ── Interaction ──────────────────────────────────────────────────
        "question" => question_summary(invocation),
        "request_user_input" => request_user_input_summary(invocation),
        "append_note" => append_note_summary(result),

        // ── Utility ──────────────────────────────────────────────────────
        "current_time" => current_time_summary(result),
        "sleep" => sleep_summary(result),
        "get_context_remaining" => context_remaining_summary(result),
        "view_image" | "inspect_image" => view_image_summary(invocation, result),
        "new_context_window" => new_context_window_summary(result),
        "tool_search" => tool_search_summary(invocation, result),
        "verifier" => verifier_summary(invocation, result),
        "runtime_info" => runtime_info_summary(result),
        "branch_race_start" => branch_race_summary(result),
        "history_ops" => history_ops_summary(invocation, result),
        "sandbox" => sandbox_summary(invocation, result),
        "package_manager" => package_manager_summary(invocation, result),

        name => humanize_tool_name(name),
    };

    if !result.ok {
        if let Some(error) = result.output.get("error").and_then(|v| v.as_str()) {
            text.push_str(&format!(" · error: {}", one_line(error)));
        } else {
            text.push_str(" · error");
        }
    }

    text
}

pub(crate) fn tool_full_content(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let mut content = format!(
        "{} {}\n\n",
        if result.ok { "✓" } else { "✗" },
        tool_compact_text(invocation, result),
    );

    if let Some(formatted) = formatted_tool_output(invocation, result) {
        content.push_str(&formatted);
    } else {
        content.push_str(&generic_tool_summary(invocation, result));
    }

    content
}
fn formatted_tool_output(invocation: &ToolInvocation, result: &ToolResult) -> Option<String> {
    let obj = result.output.as_object()?;
    let mut content = String::new();

    if let Some(error) = obj.get("error").and_then(|v| v.as_str()) {
        content.push_str(&format!("Error: {error}\n"));
        if invocation.tool_name == "bash" {
            let stdout = obj.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
            let stderr = obj.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
            if !stdout.is_empty() {
                content.push_str("\nStdout:\n```\n");
                let truncated_stdout = truncate_to_lines(stdout, MAX_TOOL_RENDER_LINES);
                content.push_str(truncated_stdout);
                if !truncated_stdout.ends_with('\n') {
                    content.push('\n');
                }
                content.push_str("```\n");
            }
            if !stderr.is_empty() {
                content.push_str("\nStderr:\n```\n");
                let truncated_stderr = truncate_to_lines(stderr, MAX_TOOL_RENDER_LINES);
                content.push_str(truncated_stderr);
                if !truncated_stderr.ends_with('\n') {
                    content.push('\n');
                }
                content.push_str("```\n");
            }
        }
        return Some(content);
    }

    if !result.ok && invocation.tool_name != "bash" {
        return None;
    }

    if invocation.tool_name == "read_file" || invocation.tool_name == "view_file" {
        let path = obj.get("path").and_then(|v| v.as_str())?;
        content.push_str(&format!("View {}", display_path(path)));
        if let Some(details) = read_file_line_details(result) {
            content.push_str(&format!(" ({details})"));
        }
        content.push_str("\n\n");
        if let Some(file_content) = obj.get("content").and_then(|v| v.as_str()) {
            let language = language_for_path(path);
            content.push_str(&format!("```{language}\n"));
            let truncated_content = truncate_to_lines(file_content, MAX_TOOL_RENDER_LINES);
            content.push_str(truncated_content);
            if !truncated_content.ends_with('\n') {
                content.push('\n');
            }
            if truncated_content.len() < file_content.len() {
                content.push_str(&format!(
                    "... (truncated, {} lines total)\n",
                    file_content.lines().count()
                ));
            }
            content.push_str("```\n");
        }
    } else if invocation.tool_name == "apply_patch"
        || (invocation.tool_name == "write"
            && (invocation.input.get("patch").is_some()
                || invocation.input.get("patches").is_some()))
    {
        if let Some(patch) = invocation.input.get("patch").and_then(|v| v.as_str()) {
            let summaries = patch_edit_summaries(patch);
            if summaries.is_empty() {
                content.push_str("Applied patch\n");
            } else {
                for summary in summaries {
                    content.push_str(&summary);
                    content.push('\n');
                }
            }
        } else {
            content.push_str("Applied patch successfully\n");
        }
        append_patch_bodies(invocation, &mut content);
        let stdout = obj.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
        let stderr = obj.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
        if !stdout.is_empty() {
            content.push_str("\nStdout:\n```\n");
            content.push_str(stdout);
            if !stdout.ends_with('\n') {
                content.push('\n');
            }
            content.push_str("```\n");
        }
        if !stderr.is_empty() {
            content.push_str("\nStderr:\n```\n");
            content.push_str(stderr);
            if !stderr.ends_with('\n') {
                content.push('\n');
            }
            content.push_str("```\n");
        }
    } else if invocation.tool_name == "write_file" || invocation.tool_name == "write" {
        let path = obj.get("path").and_then(|v| v.as_str())?;
        let (added, removed) = write_file_line_counts(invocation, result);
        content.push_str(&format!(
            "Edited {} (+{added} -{removed} lines)\n",
            display_path(path)
        ));
    } else if invocation.tool_name == "bash" {
        let status = obj.get("status").and_then(|v| v.as_i64());
        if let Some(status_code) = status {
            content.push_str(&format!("Command exited with status {status_code}\n"));
        } else {
            content.push_str("Command completed\n");
        }
        let stdout = obj.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
        let stderr = obj.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
        if !stdout.is_empty() {
            content.push_str("\nStdout:\n```\n");
            let truncated_stdout = truncate_to_lines(stdout, MAX_TOOL_RENDER_LINES);
            content.push_str(truncated_stdout);
            if !truncated_stdout.ends_with('\n') {
                content.push('\n');
            }
            if truncated_stdout.len() < stdout.len() {
                content.push_str(&format!(
                    "... (truncated, {} lines total)\n",
                    stdout.lines().count()
                ));
            }
            content.push_str("```\n");
        }
        if !stderr.is_empty() {
            content.push_str("\nStderr:\n```\n");
            let truncated_stderr = truncate_to_lines(stderr, MAX_TOOL_RENDER_LINES);
            content.push_str(truncated_stderr);
            if !truncated_stderr.ends_with('\n') {
                content.push('\n');
            }
            if truncated_stderr.len() < stderr.len() {
                content.push_str(&format!(
                    "... (truncated, {} lines total)\n",
                    stderr.lines().count()
                ));
            }
            content.push_str("```\n");
        }
    } else if invocation.tool_name == "grep" {
        content.push_str("Found matches:\n\n");
        if let Some(matches) = obj.get("matches").and_then(|v| v.as_array()) {
            for m in matches {
                if let Some(m_obj) = m.as_object() {
                    let path = m_obj.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    let line = m_obj.get("line").and_then(|v| v.as_u64()).unwrap_or(0);
                    let text = m_obj.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    content.push_str(&format!("{path}:{line}: {text}\n"));
                }
            }
        }
    } else if invocation.tool_name == "fs_browser" {
        content.push_str("Browse filesystem\n\n");
        if let Some(files) = obj.get("files").and_then(|v| v.as_array()) {
            for (i, file) in files.iter().enumerate() {
                if let Some(file) = file.as_str() {
                    content.push_str(&format!("{:>4}  {}\n", i + 1, file));
                }
            }
        }
        if let Some(entries) = obj.get("entries").and_then(|v| v.as_array()) {
            render_tree_entries(entries, &mut content, 0);
        }
    } else {
        return None;
    }

    if obj.get("truncated").and_then(|v| v.as_bool()) == Some(true) {
        content.push_str("... (truncated)\n");
    }
    Some(content)
}

fn generic_tool_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    if result.ok {
        format!("{} completed successfully\n", invocation.tool_name)
    } else if let Some(error) = result.output.get("error").and_then(|v| v.as_str()) {
        format!("Error: {error}\n")
    } else {
        format!("{} failed\n", invocation.tool_name)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Existing summaries (unchanged)
// ═══════════════════════════════════════════════════════════════════════════

fn read_file_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let path = result
        .output
        .get("path")
        .or_else(|| invocation.input.get("path"))
        .and_then(|v| v.as_str())
        .unwrap_or("file");
    if let Some(details) = read_file_line_details(result) {
        format!("Read {} ({details})", display_path(path))
    } else {
        format!("Read {}", display_path(path))
    }
}

fn read_file_line_details(result: &ToolResult) -> Option<String> {
    let start = result.output.get("start_line").and_then(|v| v.as_u64());
    let end = result.output.get("end_line").and_then(|v| v.as_u64());
    let total = result.output.get("total_lines").and_then(|v| v.as_u64());
    let read_lines = match (start, end) {
        (Some(start), Some(end)) if end >= start => Some(end - start + 1),
        (Some(_), Some(_)) => Some(0),
        _ => result
            .output
            .get("content")
            .and_then(|v| v.as_str())
            .map(count_changed_lines)
            .map(|count| count as u64),
    };

    let read_lines = read_lines?;
    let line_count = pluralize_lines(read_lines);

    match (start, end, total) {
        (Some(start), Some(end), Some(total)) if read_lines > 0 => Some(format!(
            "lines {start}-{end} of {total}, {read_lines} {line_count} read"
        )),
        (Some(start), Some(end), None) if read_lines > 0 => Some(format!(
            "lines {start}-{end}, {read_lines} {line_count} read"
        )),
        (_, _, Some(total)) => Some(format!("{read_lines} {line_count} read of {total}")),
        _ => Some(format!("{read_lines} {line_count} read")),
    }
}

fn pluralize_lines(count: u64) -> &'static str {
    if count == 1 { "line" } else { "lines" }
}

fn write_file_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let path = result
        .output
        .get("path")
        .or_else(|| invocation.input.get("path"))
        .and_then(|v| v.as_str())
        .unwrap_or("file");
    let (added, removed) = write_file_line_counts(invocation, result);
    format!("Write {} (+{added} -{removed} lines)", display_path(path))
}

fn write_file_line_counts(invocation: &ToolInvocation, result: &ToolResult) -> (usize, usize) {
    let added = result
        .output
        .get("lines_added")
        .and_then(|v| v.as_u64())
        .map(|value| value as usize)
        .or_else(|| {
            invocation
                .input
                .get("content")
                .and_then(|v| v.as_str())
                .map(count_changed_lines)
        })
        .unwrap_or(0);
    let removed = result
        .output
        .get("lines_removed")
        .and_then(|v| v.as_u64())
        .map(|value| value as usize)
        .unwrap_or(0);
    (added, removed)
}

fn apply_patch_summary(invocation: &ToolInvocation) -> String {
    let Some(patch) = first_patch_input(invocation) else {
        return if invocation.tool_name == "write" {
            "Write patch".to_string()
        } else {
            "Apply patch".to_string()
        };
    };
    patch_edit_summaries(patch)
        .into_iter()
        .next()
        .unwrap_or_else(|| "Apply patch".to_string())
}

fn first_patch_input(invocation: &ToolInvocation) -> Option<&str> {
    invocation
        .input
        .get("patch")
        .and_then(|v| v.as_str())
        .or_else(|| {
            invocation
                .input
                .get("patches")
                .and_then(|v| v.as_array())
                .and_then(|patches| patches.first())
                .and_then(|v| v.as_str())
        })
}

fn append_patch_bodies(invocation: &ToolInvocation, content: &mut String) {
    let patches = patch_inputs(invocation);
    if patches.is_empty() {
        return;
    }

    for (index, patch) in patches.iter().enumerate() {
        if patches.len() == 1 {
            content.push_str("\nPatch:\n");
        } else {
            content.push_str(&format!("\nPatch {}:\n", index + 1));
        }
        content.push_str("```diff\n");
        let truncated_patch = truncate_to_lines(patch, MAX_TOOL_RENDER_LINES);
        content.push_str(truncated_patch);
        if !truncated_patch.ends_with('\n') {
            content.push('\n');
        }
        if truncated_patch.len() < patch.len() {
            content.push_str(&format!(
                "... (truncated, {} lines total)\n",
                patch.lines().count()
            ));
        }
        content.push_str("```\n");
    }
}

fn patch_inputs(invocation: &ToolInvocation) -> Vec<&str> {
    if let Some(patch) = invocation.input.get("patch").and_then(|v| v.as_str()) {
        return vec![patch];
    }
    invocation
        .input
        .get("patches")
        .and_then(|v| v.as_array())
        .map(|patches| patches.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default()
}

fn bash_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let action = invocation.input.get("action").and_then(|v| v.as_str());
    if action == Some("list") {
        return "List background commands".to_string();
    }
    if let Some(task_id) = invocation.input.get("task_id").and_then(|v| v.as_str()) {
        return if action == Some("cancel") {
            format!("Cancel background command {task_id}")
        } else {
            format!("Poll background command {task_id}")
        };
    }

    let command = invocation
        .input
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("command");
    let is_background = result.output.get("background").and_then(|v| v.as_bool()) == Some(true);

    if is_background {
        let status = result
            .output
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let elapsed = result
            .output
            .get("elapsed_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let elapsed_str = crate::background::format_duration_ms(elapsed);
        let mut summary = format!("Run {} ({} · {})", one_line(command), status, elapsed_str);
        if let Some(exit_code) = result.output.get("exit_code").and_then(|v| v.as_i64()) {
            summary = format!(
                "Run {} ({} · exit {} · {})",
                one_line(command),
                status,
                exit_code,
                elapsed_str
            );
        }
        summary
    } else {
        let mut summary = format!("Run {}", one_line(command));
        if let Some(status) = result.output.get("status").and_then(|v| v.as_i64()) {
            summary.push_str(&format!(" (exit {status})"));
        }
        summary
    }
}

fn grep_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let pattern = invocation
        .input
        .get("pattern")
        .and_then(|v| v.as_str())
        .unwrap_or("pattern");
    let path = invocation
        .input
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let matches = result
        .output
        .get("matches")
        .and_then(|v| v.as_array())
        .map(|matches| matches.len())
        .unwrap_or(0);
    let truncated = result
        .output
        .get("truncated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let suffix = if truncated { "+" } else { "" };
    format!(
        "Search \"{}\" in {} ({}{suffix} matches)",
        one_line(pattern),
        path,
        matches
    )
}

fn fs_browser_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let action = invocation
        .input
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("browse");
    let path = result
        .output
        .get("path")
        .or_else(|| invocation.input.get("path"))
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let count = result
        .output
        .get("files")
        .and_then(|v| v.as_array())
        .or_else(|| result.output.get("entries").and_then(|v| v.as_array()))
        .map(|items| items.len());
    let action = match action {
        "list" => "List",
        "tree" => "Tree",
        "find" => "Find",
        "stat" => "Stat",
        _ => "Browse",
    };
    if let Some(count) = count {
        format!("{action} {} ({count} items)", display_path(path))
    } else {
        format!("{action} {}", display_path(path))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  New: Process & Command tools
// ═══════════════════════════════════════════════════════════════════════════

fn process_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let action = invocation
        .input
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("exec");

    match action {
        "list" => {
            let count = result
                .output
                .get("processes")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("List processes ({count} running)")
        }
        "cancel" => {
            let pid = invocation
                .input
                .get("process_id")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("Cancel process {pid}")
        }
        "wait" => {
            let pid = invocation
                .input
                .get("process_id")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            if let Some(exit_code) = result.output.get("exit_code").and_then(|v| v.as_i64()) {
                format!("Wait process {pid} (exit {exit_code})")
            } else {
                format!("Wait process {pid}")
            }
        }
        "stdin" => {
            let pid = invocation
                .input
                .get("process_id")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let bytes = result
                .output
                .get("bytes_written")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            format!("Write stdin to {pid} ({bytes} bytes)")
        }
        _ => {
            // "exec" action (default)
            let command = invocation
                .input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("command");
            let is_background =
                invocation.input.get("background").and_then(|v| v.as_bool()) == Some(true);
            if is_background {
                let pid = result
                    .output
                    .get("process_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let elapsed = result
                    .output
                    .get("elapsed_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                format!(
                    "Run {} (bg pid={} · {})",
                    one_line(command),
                    pid,
                    crate::background::format_duration_ms(elapsed)
                )
            } else if let Some(exit_code) = result.output.get("exit_code").and_then(|v| v.as_i64())
            {
                format!("Run {} (exit {exit_code})", one_line(command))
            } else {
                format!("Run {}", one_line(command))
            }
        }
    }
}

fn test_runner_summary(_invocation: &ToolInvocation, result: &ToolResult) -> String {
    let framework = result
        .output
        .get("framework")
        .and_then(|v| v.as_str())
        .unwrap_or("test");
    let passed = result
        .output
        .get("passed")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let failed = result
        .output
        .get("failed")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let skipped = result
        .output
        .get("skipped")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let duration = result
        .output
        .get("duration_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    format!(
        "Test ({framework}) — passed {passed}, failed {failed}, skipped {skipped} · {}",
        crate::background::format_duration_ms(duration)
    )
}

fn build_runner_summary(_invocation: &ToolInvocation, result: &ToolResult) -> String {
    let status = result
        .output
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let cached = result.output.get("cached").and_then(|v| v.as_bool()) == Some(true);
    let warnings = result
        .output
        .get("warnings")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let errors = result
        .output
        .get("errors")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let duration = result
        .output
        .get("duration_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    if cached {
        format!("Build (cached, {status})")
    } else if errors > 0 {
        format!(
            "Build (failed) — {errors} errors, {warnings} warnings · {}",
            crate::background::format_duration_ms(duration)
        )
    } else if warnings > 0 {
        format!(
            "Build ({status}) — {warnings} warnings · {}",
            crate::background::format_duration_ms(duration)
        )
    } else {
        format!(
            "Build ({status}) · {}",
            crate::background::format_duration_ms(duration)
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  New: Code Intelligence tools
// ═══════════════════════════════════════════════════════════════════════════

fn code_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let action = invocation
        .input
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("?");

    match action {
        "overview" => {
            let path = result
                .output
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let symbols = result
                .output
                .get("symbols")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            let files = result
                .output
                .get("files_scanned")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            format!(
                "Code overview {} ({symbols} symbols in {files} files)",
                display_path(path)
            )
        }
        "find" => {
            let query = result
                .output
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let matches = result
                .output
                .get("matches")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("Code find \"{query}\" ({matches} symbols)")
        }
        "references" => {
            let name = invocation
                .input
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let refs = result
                .output
                .get("references")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("Code references to \"{name}\" ({refs} refs)")
        }
        "diagnostics" => {
            let path = result
                .output
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let issues = result
                .output
                .get("diagnostics")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("Code diagnostics {} ({issues} issues)", display_path(path))
        }
        _ => "Code".to_string(),
    }
}

fn code_edit_summary(_invocation: &ToolInvocation, result: &ToolResult) -> String {
    let action = result
        .output
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("edit");
    let path = result
        .output
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("file");
    let edits = result
        .output
        .get("edits")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let start = result.output.get("start_line").and_then(|v| v.as_u64());
    let end = result.output.get("end_line").and_then(|v| v.as_u64());

    match (start, end) {
        (Some(s), Some(e)) => {
            format!(
                "Code edit {action} {} ({} edits, lines {s}-{e})",
                display_path(path),
                edits
            )
        }
        _ => {
            format!(
                "Code edit {action} {} ({} edits)",
                display_path(path),
                edits
            )
        }
    }
}

fn code_exec_summary(_invocation: &ToolInvocation, result: &ToolResult) -> String {
    let status = result
        .output
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let ops = result
        .output
        .get("ops_executed")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if let Some(failed_op) = result.output.get("failed_op").and_then(|v| v.as_u64()) {
        format!("Code exec (failed at op {failed_op}/{ops})")
    } else {
        format!("Code exec ({status}, {ops} ops)")
    }
}

fn ast_search_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let query = invocation
        .input
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let matches = result
        .output
        .get("matches")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    format!("AST search \"{query}\" ({matches} matches)")
}

fn symbol_goto_summary(_invocation: &ToolInvocation, result: &ToolResult) -> String {
    let name = result
        .output
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    if let Some(symbol) = result.output.get("symbol").and_then(|v| v.as_object()) {
        let path = symbol.get("path").and_then(|v| v.as_str()).unwrap_or("?");
        let line = symbol.get("line").and_then(|v| v.as_u64()).unwrap_or(0);
        format!("Goto {name} → {}:{line}", display_path(path))
    } else {
        format!("Goto {name} (not found)")
    }
}

fn symbol_references_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let name = invocation
        .input
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let refs = result
        .output
        .get("references")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    format!("References to \"{name}\" ({refs} refs)")
}

fn dependency_graph_summary(result: &ToolResult) -> String {
    let edges = result
        .output
        .get("edges")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let files = result
        .output
        .get("files_indexed")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    format!("Dependency graph ({edges} edges in {files} files)")
}

fn test_discovery_summary(result: &ToolResult) -> String {
    let tests = result
        .output
        .get("tests")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    if tests == 0 {
        "Test discovery (no tests found)".to_string()
    } else if let Some(tests_arr) = result.output.get("tests").and_then(|v| v.as_array()) {
        if let Some(cmd) = tests_arr
            .first()
            .and_then(|v| v.get("command"))
            .or_else(|| tests_arr.first().and_then(|v| v.get("suggestion")))
            .and_then(|v| v.as_str())
        {
            format!("Test discovery → {cmd}")
        } else {
            format!("Test discovery ({tests} suggestions)")
        }
    } else {
        format!("Test discovery ({tests} suggestions)")
    }
}

fn churn_summary(result: &ToolResult) -> String {
    let files = result
        .output
        .get("churn")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    format!("Churn query ({files} files)")
}

// ═══════════════════════════════════════════════════════════════════════════
//  New: Repo Search Aliases
// ═══════════════════════════════════════════════════════════════════════════

fn search_tool_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    // search, list_dir, glob are aliases of SearchTool.
    // For search the action is explicit; for list_dir it's always "list";
    // for glob it's always "find".
    let action = invocation
        .input
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| match invocation.tool_name.as_str() {
            "list_dir" => "list",
            "glob" => "find",
            _ => "grep",
        });

    match action {
        "grep" => grep_summary(invocation, result),
        "list" => {
            let path = result
                .output
                .get("path")
                .or_else(|| invocation.input.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let count = result
                .output
                .get("files")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("List {} ({count} items)", display_path(path))
        }
        "tree" => {
            let path = result
                .output
                .get("path")
                .or_else(|| invocation.input.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let count = result
                .output
                .get("entries")
                .and_then(|v| v.as_array())
                .map(|entries| count_tree_entries(entries))
                .unwrap_or(0);
            format!("Tree {} ({count} items)", display_path(path))
        }
        "find" => {
            let pattern = invocation
                .input
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("*");
            let count = result
                .output
                .get("files")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("Find \"{pattern}\" ({count} files)")
        }
        "stat" => {
            let path = result
                .output
                .get("path")
                .or_else(|| invocation.input.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            if let Some(size) = result.output.get("size").and_then(|v| v.as_u64()) {
                format!("Stat {} ({size} bytes)", display_path(path))
            } else {
                format!("Stat {}", display_path(path))
            }
        }
        _ => "Search".to_string(),
    }
}

fn count_tree_entries(entries: &[serde_json::Value]) -> usize {
    let mut count = 0;
    for entry in entries {
        count += 1;
        if let Some(children) = entry.get("entries").and_then(|v| v.as_array()) {
            count += count_tree_entries(children);
        }
    }
    count
}

// ═══════════════════════════════════════════════════════════════════════════
//  New: Repo Explore & Subagent
// ═══════════════════════════════════════════════════════════════════════════

fn repo_explore_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let query = invocation
        .input
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let elapsed = result
        .output
        .get("elapsed_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    format!(
        "Repo explore \"{}\" · {}",
        one_line(query),
        crate::background::format_duration_ms(elapsed)
    )
}

fn subagent_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let action = invocation.input.get("action").and_then(|v| v.as_str());

    if action == Some("list") {
        let count = result
            .output
            .get("tasks")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        return format!("Subagent list ({count} tasks)");
    }

    if let Some(task_id) = invocation.input.get("task_id").and_then(|v| v.as_str()) {
        return if action == Some("cancel") {
            format!("Cancel subagent {task_id}")
        } else {
            // poll
            let status = result
                .output
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let elapsed = result
                .output
                .get("elapsed_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            format!(
                "Poll subagent {task_id} ({status} · {})",
                crate::background::format_duration_ms(elapsed)
            )
        };
    }

    // New subagent run
    let prompt = invocation
        .input
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("task");
    let is_background = invocation.input.get("background").and_then(|v| v.as_bool()) == Some(true);
    let elapsed = result
        .output
        .get("elapsed_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    if is_background {
        let task_id = result
            .output
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        format!(
            "Subagent \"{}\" (bg {})",
            truncate_for_summary(prompt, 40),
            task_id
        )
    } else {
        format!(
            "Subagent \"{}\" · {}",
            truncate_for_summary(prompt, 40),
            crate::background::format_duration_ms(elapsed)
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  New: Planning & Session
// ═══════════════════════════════════════════════════════════════════════════

fn plan_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let action = invocation
        .input
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("?");

    match action {
        "create" => {
            let title = result
                .output
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("plan");
            let steps = result
                .output
                .get("steps_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            format!("Plan create \"{title}\" ({steps} steps)")
        }
        "update" => {
            let plan_id = result
                .output
                .get("plan_id")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let status = result
                .output
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("updated");
            format!("Plan update {plan_id} ({status})")
        }
        "complete_step" => {
            let plan_id = result
                .output
                .get("plan_id")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let step = result
                .output
                .get("step_index")
                .and_then(|v| v.as_u64())
                .map(|i| format!("#{i}"))
                .unwrap_or_else(|| "done".to_string());
            format!("Plan {plan_id} step {step} completed")
        }
        "get" => {
            if let Some(plan) = result.output.get("plan").and_then(|v| v.as_object()) {
                let title = plan.get("title").and_then(|v| v.as_str()).unwrap_or("?");
                format!("Plan get \"{title}\"")
            } else {
                "Plan get".to_string()
            }
        }
        "list" => {
            let count = result
                .output
                .get("count")
                .and_then(|v| v.as_u64())
                .or_else(|| {
                    result
                        .output
                        .get("plans")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len() as u64)
                })
                .unwrap_or(0);
            format!("Plan list ({count} plans)")
        }
        "active" => {
            if let Some(plan) = result.output.get("plan").and_then(|v| v.as_object()) {
                let title = plan.get("title").and_then(|v| v.as_str()).unwrap_or("?");
                format!("Plan active \"{title}\"")
            } else {
                "Plan active (none)".to_string()
            }
        }
        _ => "Plan".to_string(),
    }
}

fn init_session_summary(result: &ToolResult) -> String {
    let status = result
        .output
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    if status == "initialized" {
        let total = result
            .output
            .get("features_total")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        format!("Init session ({total} features)")
    } else {
        format!("Init session ({status})")
    }
}

fn mark_feature_done_summary(result: &ToolResult) -> String {
    let status = result
        .output
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let fid = result
        .output
        .get("feature_id")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let passes = result
        .output
        .get("passes")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if passes {
        format!("Mark done {fid} (all checks passed)")
    } else {
        format!("Mark done {fid} ({status})")
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  New: Interaction tools
// ═══════════════════════════════════════════════════════════════════════════

fn question_summary(invocation: &ToolInvocation) -> String {
    let question = invocation
        .input
        .get("question")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    format!("Question \"{}\"", truncate_for_summary(question, 50))
}

fn request_user_input_summary(invocation: &ToolInvocation) -> String {
    let title = invocation
        .input
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    format!("Request input \"{title}\"")
}

fn append_note_summary(result: &ToolResult) -> String {
    let path = result
        .output
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("notes.md");
    format!("Append note to {}", display_path(path))
}

// ═══════════════════════════════════════════════════════════════════════════
//  New: Utility tools
// ═══════════════════════════════════════════════════════════════════════════

fn current_time_summary(result: &ToolResult) -> String {
    let iso = result
        .output
        .get("utc_iso")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    format!("Current time: {iso}")
}

fn sleep_summary(result: &ToolResult) -> String {
    let secs = result
        .output
        .get("slept_seconds")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    format!("Sleep ({secs}s)")
}

fn context_remaining_summary(result: &ToolResult) -> String {
    let remaining = result
        .output
        .get("remaining_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let window = result
        .output
        .get("context_window")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let pct = result
        .output
        .get("usage_percent")
        .and_then(|v| v.as_str())
        .unwrap_or("?%");
    format!("Context: {remaining} / {window} ({pct})")
}

fn view_image_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let path = result
        .output
        .get("path")
        .or_else(|| invocation.input.get("path"))
        .and_then(|v| v.as_str())
        .unwrap_or("image");
    let format = result
        .output
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let size = result
        .output
        .get("size_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    format!(
        "View image {} ({format}, {} bytes)",
        display_path(path),
        size
    )
}

fn new_context_window_summary(result: &ToolResult) -> String {
    let requested = result
        .output
        .get("new_context_requested")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if requested {
        "New context window requested".to_string()
    } else {
        "New context window".to_string()
    }
}

fn tool_search_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let query = invocation
        .input
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let count = result
        .output
        .get("count")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            result
                .output
                .get("results")
                .and_then(|v| v.as_array())
                .map(|a| a.len() as u64)
        })
        .unwrap_or(0);
    format!("Tool search \"{query}\" ({count} results)")
}

fn verifier_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let action = invocation
        .input
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("run");

    match action {
        "list" => {
            let total = result
                .output
                .get("total")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            format!("Verifier list ({total} results)")
        }
        "status" => {
            let key = invocation
                .input
                .get("key")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let status = result
                .output
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("Verifier status {key} ({status})")
        }
        _ => {
            // run
            let summary = result
                .output
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !summary.is_empty() {
                format!("Verify {}", one_line(summary))
            } else {
                let status = result
                    .output
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let cmd = result
                    .output
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                format!("Verify {} ({status})", one_line(cmd))
            }
        }
    }
}

fn runtime_info_summary(result: &ToolResult) -> String {
    let profile = result
        .output
        .get("harness_profile")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    format!("Runtime info: {profile} profile")
}

fn branch_race_summary(result: &ToolResult) -> String {
    let task = result
        .output
        .get("task")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let hypotheses = result
        .output
        .get("hypotheses")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    format!(
        "Branch race \"{}\" ({hypotheses} hypotheses)",
        truncate_for_summary(task, 40)
    )
}

// ═══════════════════════════════════════════════════════════════════════════
//  New: History, Sandbox, Package Manager
// ═══════════════════════════════════════════════════════════════════════════

fn history_ops_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let action = invocation
        .input
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("?");

    match action {
        "search" => {
            let query = invocation
                .input
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let count = result
                .output
                .get("results")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("History search \"{query}\" ({count} results)")
        }
        "recent" => {
            let count = result
                .output
                .get("results")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("History recent ({count} events)")
        }
        "get" => format!("History get"),
        "summaries" => {
            let count = result
                .output
                .get("sessions")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("History summaries ({count} sessions)")
        }
        _ => "History".to_string(),
    }
}

fn sandbox_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let action = invocation
        .input
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("?");

    match action {
        "snapshot" => {
            let files = result
                .output
                .get("files_snapshotted")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            format!("Sandbox snapshot ({files} files)")
        }
        "rollback" => {
            let restored = result
                .output
                .get("files_restored")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let created = result
                .output
                .get("files_created_and_removed")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            format!("Sandbox rollback ({restored} restored, {created} removed)")
        }
        "status" => {
            let has_changes = result
                .output
                .get("has_changes")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if has_changes {
                "Sandbox status (changes detected)".to_string()
            } else {
                "Sandbox status (clean)".to_string()
            }
        }
        "reset" => "Sandbox reset".to_string(),
        _ => "Sandbox".to_string(),
    }
}

fn package_manager_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let action = invocation
        .input
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let manager = result
        .output
        .get("manager")
        .and_then(|v| v.as_str())
        .unwrap_or("?");

    match action {
        "install" => format!("Package install ({manager})"),
        "add" => {
            let packages = invocation
                .input
                .get("packages")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            let pkgs = invocation
                .input
                .get("packages")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .take(3)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            if packages == 1 {
                format!("Package add {pkgs} ({manager})")
            } else {
                format!("Package add ({packages} packages, {manager})")
            }
        }
        "remove" => {
            let packages = invocation
                .input
                .get("packages")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            let pkgs = invocation
                .input
                .get("packages")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .take(3)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            if packages == 1 {
                format!("Package remove {pkgs} ({manager})")
            } else {
                format!("Package remove ({packages} packages, {manager})")
            }
        }
        "update" => {
            let packages = invocation
                .input
                .get("packages")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            if packages == 0 {
                format!("Package update all ({manager})")
            } else {
                format!("Package update ({packages} packages, {manager})")
            }
        }
        "check" => format!("Package check ({manager})"),
        _ => "Package manager".to_string(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Shared helpers
// ═══════════════════════════════════════════════════════════════════════════

fn humanize_tool_name(name: &str) -> String {
    let mut chars = name.replace('_', " ").chars().collect::<Vec<_>>();
    if let Some(first) = chars.first_mut() {
        first.make_ascii_uppercase();
    }
    chars.into_iter().collect()
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn display_path(path: &str) -> String {
    let path_ref = Path::new(path);
    if path_ref.is_absolute()
        && let Some(project_dir) = project_dir()
        && let Ok(stripped) = path_ref.strip_prefix(project_dir)
    {
        return stripped.to_string_lossy().to_string();
    }
    path.to_string()
}

fn truncate_for_summary(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_string()
    } else {
        let truncated: String = value.chars().take(max_chars).collect();
        format!("{truncated}…")
    }
}

fn project_dir() -> Option<&'static PathBuf> {
    static PROJECT_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    PROJECT_DIR
        .get_or_init(|| std::env::current_dir().ok())
        .as_ref()
}

fn count_changed_lines(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.lines().count().max(1)
    }
}

fn patch_edit_summaries(patch: &str) -> Vec<String> {
    let mut summaries = Vec::new();
    let mut current_path: Option<String> = None;
    let mut added = 0usize;
    let mut removed = 0usize;

    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            flush_patch_summary(&mut summaries, &mut current_path, &mut added, &mut removed);
            current_path = Some(path.to_string());
            continue;
        }
        if current_path.is_none() {
            if let Some(path) = line.strip_prefix("*** Update File: ") {
                current_path = Some(path.to_string());
                continue;
            }
            if let Some(path) = line.strip_prefix("*** Add File: ") {
                current_path = Some(path.to_string());
                continue;
            }
        }
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    flush_patch_summary(&mut summaries, &mut current_path, &mut added, &mut removed);

    summaries
}

fn flush_patch_summary(
    summaries: &mut Vec<String>,
    current_path: &mut Option<String>,
    added: &mut usize,
    removed: &mut usize,
) {
    if let Some(path) = current_path.take() {
        summaries.push(format!("Edited {path} (+{} -{})", *added, *removed));
        *added = 0;
        *removed = 0;
    }
}

fn language_for_path(path: &str) -> &'static str {
    match path
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .unwrap_or_default()
    {
        "rs" => "rust",
        "toml" => "toml",
        "json" => "json",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" => "typescript",
        "jsx" => "javascript",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cc" | "cpp" | "hpp" => "cpp",
        "sh" | "bash" => "bash",
        "zsh" => "zsh",
        "fish" => "fish",
        "md" | "markdown" => "markdown",
        "yaml" | "yml" => "yaml",
        "html" => "html",
        "css" => "css",
        "xml" => "xml",
        "sql" => "sql",
        _ => "",
    }
}

fn render_tree_entries(entries: &[serde_json::Value], content: &mut String, indent: usize) {
    let prefix = "  ".repeat(indent);
    for entry in entries {
        let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("file");
        if entry_type == "dir" {
            content.push_str(&format!("{prefix}{name}/\n"));
            if let Some(children) = entry.get("entries").and_then(|v| v.as_array()) {
                render_tree_entries(children, content, indent + 1);
            }
        } else {
            let size = entry.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
            content.push_str(&format!("{prefix}{name} ({size} bytes)\n"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn invocation(tool_name: &str, input: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            id: "tool-1".to_string(),
            tool_name: tool_name.to_string(),
            input,
        }
    }

    fn ok_result(output: serde_json::Value) -> ToolResult {
        ToolResult {
            invocation_id: "tool-1".to_string(),
            ok: true,
            output,
        }
    }

    #[test]
    fn full_apply_patch_output_includes_patch_body() {
        let patch = "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch";
        let content = tool_full_content(
            &invocation("apply_patch", json!({ "patch": patch })),
            &ok_result(json!({
                "method": "structured",
                "patches_applied": 1,
                "files_patched": 1
            })),
        );

        assert!(content.contains("Patch:\n```diff\n"));
        assert!(content.contains("-old\n+new"));
        assert!(content.contains("```"));
    }

    #[test]
    fn full_write_patch_output_includes_multiple_patch_bodies() {
        let content = tool_full_content(
            &invocation(
                "write",
                json!({
                    "patches": [
                        "*** Begin Patch\n*** Add File: one.txt\n+one\n*** End Patch",
                        "*** Begin Patch\n*** Add File: two.txt\n+two\n*** End Patch"
                    ]
                }),
            ),
            &ok_result(json!({
                "method": "structured",
                "patches_applied": 2,
                "files_patched": 2
            })),
        );

        assert!(content.contains("Patch 1:\n```diff\n"));
        assert!(content.contains("Patch 2:\n```diff\n"));
        assert!(content.contains("*** Add File: one.txt"));
        assert!(content.contains("*** Add File: two.txt"));
    }
}
