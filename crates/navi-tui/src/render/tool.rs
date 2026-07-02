use navi_sdk::{ToolInvocation, ToolResult};
use serde_json::Value;
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
        "read" | "read_file" | "view_file" => read_file_summary(invocation, result),
        "write_file" => write_file_summary(invocation, result),
        "apply_patch" => apply_patch_summary(invocation),
        "write" if has_patch_input(invocation) => apply_patch_summary(invocation),
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
        "set_goal" => set_goal_summary(invocation, result),
        "wait" => wait_summary(invocation, result),
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
            append_text_block(&mut content, "Stdout", stdout, "");
            append_text_block(&mut content, "Stderr", stderr, "");
        }
        append_json_section(&mut content, "Output", &result.output);
        return Some(content);
    }

    if !result.ok && invocation.tool_name != "bash" {
        return None;
    }

    if matches!(
        invocation.tool_name.as_str(),
        "read" | "read_file" | "view_file"
    ) {
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
    } else if is_patch_invocation(invocation) {
        let patches = patch_inputs(invocation);
        let summaries: Vec<String> = patches
            .iter()
            .flat_map(|patch| patch_edit_summaries(patch))
            .collect();
        if summaries.is_empty() {
            content.push_str("Applied patch successfully\n");
        } else {
            for summary in summaries {
                content.push_str(&summary);
                content.push('\n');
            }
        }
        append_patch_bodies(&patches, &mut content);
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
        append_text_block(&mut content, "Stdout", stdout, "");
        append_text_block(&mut content, "Stderr", stderr, "");
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
    } else if matches!(
        invocation.tool_name.as_str(),
        "search" | "list_dir" | "glob"
    ) {
        render_search_output(invocation, result, &mut content);
    } else if invocation.tool_name == "process" {
        render_process_output(result, &mut content);
    } else if invocation.tool_name == "test_runner" {
        render_test_runner_output(result, &mut content);
    } else if invocation.tool_name == "build_runner" {
        render_build_runner_output(result, &mut content);
    } else if matches!(
        invocation.tool_name.as_str(),
        "code"
            | "code_edit"
            | "code_exec"
            | "ast_search"
            | "symbol_goto"
            | "symbol_references"
            | "dependency_graph_query"
            | "test_discovery"
            | "ownership_churn_query"
    ) {
        render_named_structured_output("Code output", result, &mut content);
    } else if matches!(
        invocation.tool_name.as_str(),
        "repo_explore"
            | "subagent"
            | "plan"
            | "init_session"
            | "mark_feature_done"
            | "question"
            | "request_user_input"
            | "append_note"
            | "current_time"
            | "sleep"
            | "set_goal"
            | "wait"
            | "get_context_remaining"
            | "view_image"
            | "inspect_image"
            | "new_context_window"
            | "tool_search"
            | "verifier"
            | "runtime_info"
            | "branch_race_start"
            | "history_ops"
            | "sandbox"
            | "package_manager"
    ) {
        render_named_structured_output("Output", result, &mut content);
    } else {
        return None;
    }

    if obj.get("truncated").and_then(|v| v.as_bool()) == Some(true) {
        content.push_str("... (truncated)\n");
    }
    Some(content)
}

fn generic_tool_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let mut content = String::new();
    if result.ok {
        content.push_str(&format!(
            "{} completed successfully\n",
            invocation.tool_name
        ));
    } else if let Some(error) = result.output.get("error").and_then(|v| v.as_str()) {
        content.push_str(&format!("Error: {error}\n"));
    } else {
        content.push_str(&format!("{} failed\n", invocation.tool_name));
    }
    append_json_section(&mut content, "Input", &invocation.input);
    append_json_section(&mut content, "Output", &result.output);
    content
}

fn render_search_output(invocation: &ToolInvocation, result: &ToolResult, content: &mut String) {
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
        "grep" => {
            content.push_str("Found matches:\n\n");
            render_matches(result.output.get("matches"), content);
        }
        "tree" => {
            content.push_str("Directory tree:\n\n");
            if let Some(entries) = result.output.get("entries").and_then(|v| v.as_array()) {
                render_tree_entries(entries, content, 0);
            }
        }
        "list" | "find" => {
            let title = if action == "find" {
                "Files found"
            } else {
                "Directory entries"
            };
            content.push_str(title);
            content.push_str(":\n\n");
            render_file_list(result.output.get("files"), content);
        }
        "stat" => {
            content.push_str("File metadata:\n");
            append_json_section(content, "Output", &result.output);
        }
        _ => append_json_section(content, "Output", &result.output),
    }
}

fn render_process_output(result: &ToolResult, content: &mut String) {
    if let Some(processes) = result.output.get("processes").and_then(|v| v.as_array()) {
        content.push_str("Processes:\n\n");
        for process in processes {
            let id = process
                .get("process_id")
                .or_else(|| process.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let status = process
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let command = process
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("process");
            content.push_str(&format!("- {id} [{status}] {}\n", one_line(command)));
        }
    }

    if let Some(process_id) = result.output.get("process_id").and_then(|v| v.as_str()) {
        content.push_str(&format!("Process: {process_id}\n"));
    }
    if let Some(status) = result.output.get("status").and_then(|v| v.as_str()) {
        content.push_str(&format!("Status: {status}\n"));
    }
    if let Some(exit_code) = result.output.get("exit_code").and_then(|v| v.as_i64()) {
        content.push_str(&format!("Exit code: {exit_code}\n"));
    }
    let stdout = result
        .output
        .get("stdout")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let stderr = result
        .output
        .get("stderr")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    append_text_block(content, "Stdout", stdout, "");
    append_text_block(content, "Stderr", stderr, "");

    append_json_section(content, "Output", &result.output);
}

fn render_test_runner_output(result: &ToolResult, content: &mut String) {
    if let Some(summary) = result.output.get("summary").and_then(|v| v.as_str()) {
        content.push_str(summary);
        content.push_str("\n\n");
    }
    if let Some(failures) = result.output.get("failures").and_then(|v| v.as_array())
        && !failures.is_empty()
    {
        content.push_str("Failures:\n\n");
        for failure in failures {
            let name = failure
                .get("test_name")
                .and_then(|v| v.as_str())
                .unwrap_or("test");
            let message = failure
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            content.push_str(&format!("- {name}: {}\n", one_line(message)));
            if let Some(location) = failure.get("location").and_then(|v| v.as_str()) {
                content.push_str(&format!("  at {location}\n"));
            }
        }
        content.push('\n');
    }
    if let Some(raw) = result.output.get("raw_output").and_then(|v| v.as_str()) {
        append_text_block(content, "Raw output", raw, "");
    }
    append_json_section(content, "Output", &result.output);
}

fn render_build_runner_output(result: &ToolResult, content: &mut String) {
    if let Some(summary) = result.output.get("summary").and_then(|v| v.as_str()) {
        content.push_str(summary);
        content.push_str("\n\n");
    }
    render_diagnostic_list("Errors", result.output.get("errors"), content);
    render_diagnostic_list("Warnings", result.output.get("warnings"), content);
    append_json_section(content, "Output", &result.output);
}

fn render_named_structured_output(title: &str, result: &ToolResult, content: &mut String) {
    if let Some(message) = result.output.get("message").and_then(|v| v.as_str()) {
        content.push_str(message);
        content.push_str("\n\n");
    } else if let Some(summary) = result.output.get("summary").and_then(|v| v.as_str()) {
        content.push_str(summary);
        content.push_str("\n\n");
    }
    append_json_section(content, title, &result.output);
}

fn render_matches(matches: Option<&Value>, content: &mut String) {
    let Some(matches) = matches.and_then(|v| v.as_array()) else {
        return;
    };
    for m in matches {
        if let Some(m_obj) = m.as_object() {
            let path = m_obj.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let line = m_obj
                .get("line")
                .or_else(|| m_obj.get("line_number"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let text = m_obj
                .get("text")
                .or_else(|| m_obj.get("line_text"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            content.push_str(&format!("{path}:{line}: {text}\n"));
        } else if let Some(text) = m.as_str() {
            content.push_str(text);
            content.push('\n');
        }
    }
}

fn render_file_list(files: Option<&Value>, content: &mut String) {
    let Some(files) = files.and_then(|v| v.as_array()) else {
        return;
    };
    for (index, file) in files.iter().enumerate() {
        if let Some(file) = file.as_str() {
            content.push_str(&format!("{:>4}  {}\n", index + 1, file));
        } else if let Some(path) = file.get("path").and_then(|v| v.as_str()) {
            content.push_str(&format!("{:>4}  {}\n", index + 1, path));
        } else {
            content.push_str(&format!("{:>4}  {}\n", index + 1, compact_json(file)));
        }
    }
}

fn render_diagnostic_list(title: &str, value: Option<&Value>, content: &mut String) {
    let Some(items) = value.and_then(|v| v.as_array()) else {
        return;
    };
    if items.is_empty() {
        return;
    }
    content.push_str(title);
    content.push_str(":\n\n");
    for item in items {
        let message = item
            .get("message")
            .or_else(|| item.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| item.as_str().unwrap_or(""));
        let path = item.get("path").and_then(|v| v.as_str());
        let line = item.get("line").and_then(|v| v.as_u64());
        match (path, line) {
            (Some(path), Some(line)) => content.push_str(&format!(
                "- {}:{line}: {}\n",
                display_path(path),
                one_line(message)
            )),
            (Some(path), None) => content.push_str(&format!(
                "- {}: {}\n",
                display_path(path),
                one_line(message)
            )),
            _ => content.push_str(&format!("- {}\n", one_line(message))),
        }
    }
    content.push('\n');
}

fn append_json_section(content: &mut String, title: &str, value: &Value) {
    if value.is_null() {
        return;
    }
    content.push_str(&format!("\n{title}:\n```json\n"));
    let rendered = pretty_json(value);
    let truncated = truncate_to_lines(&rendered, MAX_TOOL_RENDER_LINES);
    content.push_str(truncated);
    if !truncated.ends_with('\n') {
        content.push('\n');
    }
    if truncated.len() < rendered.len() {
        content.push_str(&format!(
            "... (truncated, {} lines total)\n",
            rendered.lines().count()
        ));
    }
    content.push_str("```\n");
}

fn append_text_block(content: &mut String, title: &str, text: &str, language: &str) {
    if text.is_empty() {
        return;
    }
    content.push_str(&format!("\n{title}:\n```{language}\n"));
    let truncated = truncate_to_lines(text, MAX_TOOL_RENDER_LINES);
    content.push_str(truncated);
    if !truncated.ends_with('\n') {
        content.push('\n');
    }
    if truncated.len() < text.len() {
        content.push_str(&format!(
            "... (truncated, {} lines total)\n",
            text.lines().count()
        ));
    }
    content.push_str("```\n");
}

fn pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
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

fn has_patch_input(invocation: &ToolInvocation) -> bool {
    invocation.input.get("patch").is_some()
        || invocation.input.get("patches").is_some()
        || invocation.input.get("edits").is_some()
}

fn is_patch_invocation(invocation: &ToolInvocation) -> bool {
    invocation.tool_name == "apply_patch"
        || (invocation.tool_name == "write" && has_patch_input(invocation))
}

fn apply_patch_summary(invocation: &ToolInvocation) -> String {
    let Some(patch) = first_patch_input(invocation) else {
        return if invocation.tool_name == "write" {
            "Write patch".to_string()
        } else {
            "Apply patch".to_string()
        };
    };
    patch_edit_summaries(&patch)
        .into_iter()
        .next()
        .unwrap_or_else(|| "Apply patch".to_string())
}

fn first_patch_input(invocation: &ToolInvocation) -> Option<String> {
    patch_inputs(invocation).into_iter().next()
}

fn append_patch_bodies(patches: &[String], content: &mut String) {
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

fn patch_inputs(invocation: &ToolInvocation) -> Vec<String> {
    if let Some(patch) = invocation.input.get("patch").and_then(|v| v.as_str()) {
        return vec![patch.to_string()];
    }
    if let Some(patches) = invocation.input.get("patches").and_then(|v| v.as_array()) {
        let patches = patches
            .iter()
            .filter_map(|v| v.as_str())
            .filter(|patch| !patch.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if !patches.is_empty() {
            return patches;
        }
    }
    invocation
        .input
        .get("edits")
        .and_then(|v| v.as_array())
        .map(|edits| edits.iter().filter_map(edit_to_patch_body).collect())
        .unwrap_or_default()
}

fn edit_to_patch_body(edit: &Value) -> Option<String> {
    let path = edit.get("path").and_then(Value::as_str)?;
    let search = edit.get("search").and_then(Value::as_str)?;
    let replace = edit.get("replace").and_then(Value::as_str)?;
    let mut patch = format!("*** Begin Patch\n*** Update File: {path}\n@@\n");
    append_prefixed_lines(&mut patch, '-', search);
    append_prefixed_lines(&mut patch, '+', replace);
    patch.push_str("*** End Patch");
    Some(patch)
}

fn append_prefixed_lines(output: &mut String, prefix: char, text: &str) {
    for line in text.lines() {
        output.push(prefix);
        output.push_str(line);
        output.push('\n');
    }
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

fn set_goal_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let objective = invocation
        .input
        .get("objective")
        .and_then(|v| v.as_str())
        .or_else(|| result.output.get("objective").and_then(|v| v.as_str()))
        .unwrap_or("goal");
    let budget = invocation
        .input
        .get("token_budget")
        .and_then(|v| v.as_i64())
        .or_else(|| result.output.get("token_budget").and_then(|v| v.as_i64()));
    if let Some(budget) = budget {
        format!(
            "Set goal \"{}\" ({budget} tokens)",
            truncate_for_summary(objective, 48)
        )
    } else {
        format!("Set goal \"{}\"", truncate_for_summary(objective, 48))
    }
}

fn wait_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let file = invocation
        .input
        .get("file_path")
        .and_then(|v| v.as_str())
        .or_else(|| result.output.get("file_path").and_then(|v| v.as_str()));
    let timeout = invocation
        .input
        .get("timeout_seconds")
        .and_then(|v| v.as_u64())
        .unwrap_or(120);
    let status = result
        .output
        .get("status")
        .and_then(|v| v.as_str())
        .or_else(|| result.output.get("message").and_then(|v| v.as_str()));
    match (file, status) {
        (Some(file), Some(status)) => {
            format!("Wait {} ({})", display_path(file), one_line(status))
        }
        (Some(file), None) => format!("Wait {} (up to {timeout}s)", display_path(file)),
        (None, Some(status)) => format!("Wait ({})", one_line(status)),
        (None, None) => format!("Wait ({timeout}s)"),
    }
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

    fn err_result(output: serde_json::Value) -> ToolResult {
        ToolResult {
            invocation_id: "tool-1".to_string(),
            ok: false,
            output,
        }
    }

    fn broad_input(tool_name: &str) -> serde_json::Value {
        match tool_name {
            "read" | "read_file" | "view_file" => json!({ "path": "src/lib.rs" }),
            "write" | "write_file" => json!({ "path": "src/lib.rs", "content": "fn main() {}\n" }),
            "apply_patch" => {
                json!({ "patch": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch" })
            }
            "bash" | "process" => json!({ "command": "echo hi" }),
            "grep" => json!({ "pattern": "needle", "path": "src" }),
            "search" | "fs_browser" => json!({ "action": "list", "path": "src" }),
            "list_dir" => json!({ "directory": "src" }),
            "glob" => json!({ "pattern": "*.rs" }),
            "question" => json!({ "question": "Continue?", "options": ["yes", "no"] }),
            "plan" => json!({ "action": "list" }),
            "package_manager" => json!({ "action": "install" }),
            "code" => json!({ "action": "overview", "path": "src" }),
            "code_exec" => json!({ "ops": [{ "op": "trace-note", "note": "done" }] }),
            "ast_search" | "tool_search" => json!({ "query": "Renderer" }),
            "symbol_goto" | "symbol_references" => json!({ "name": "Renderer" }),
            "test_discovery" => json!({ "paths": ["src/lib.rs"] }),
            "subagent" => json!({ "prompt": "inspect renderer" }),
            "request_user_input" => json!({ "title": "Need info", "description": "Provide info" }),
            "set_goal" => json!({ "objective": "fix renderers", "token_budget": 10000 }),
            "wait" => json!({ "file_path": "src/lib.rs", "timeout_seconds": 30 }),
            "view_image" | "inspect_image" => json!({ "path": "image.png" }),
            "verifier" => json!({ "action": "run", "command": "cargo test" }),
            "history_ops" => json!({ "action": "recent" }),
            "sandbox" => json!({ "action": "status" }),
            _ => json!({}),
        }
    }

    fn broad_output() -> serde_json::Value {
        let mut output = serde_json::Map::new();
        output.insert("path".to_string(), json!("src/lib.rs"));
        output.insert("content".to_string(), json!("fn main() {}\n"));
        output.insert("start_line".to_string(), json!(1));
        output.insert("end_line".to_string(), json!(1));
        output.insert("total_lines".to_string(), json!(1));
        output.insert("files".to_string(), json!(["src/lib.rs"]));
        output.insert(
            "entries".to_string(),
            json!([{ "name": "lib.rs", "type": "file", "size": 12 }]),
        );
        output.insert(
            "matches".to_string(),
            json!([{ "path": "src/lib.rs", "line": 1, "text": "needle" }]),
        );
        output.insert("stdout".to_string(), json!("hi\n"));
        output.insert("stderr".to_string(), json!(""));
        output.insert("exit_code".to_string(), json!(0));
        output.insert("status".to_string(), json!("success"));
        output.insert("summary".to_string(), json!("completed"));
        output.insert("framework".to_string(), json!("cargo"));
        output.insert("passed".to_string(), json!(1));
        output.insert("failed".to_string(), json!(0));
        output.insert("skipped".to_string(), json!(0));
        output.insert("duration_ms".to_string(), json!(10));
        output.insert("warnings".to_string(), json!([]));
        output.insert("errors".to_string(), json!([]));
        output.insert("symbols".to_string(), json!([]));
        output.insert("files_scanned".to_string(), json!(1));
        output.insert("references".to_string(), json!([]));
        output.insert("edges".to_string(), json!([]));
        output.insert("files_indexed".to_string(), json!(1));
        output.insert("tests".to_string(), json!([]));
        output.insert("churn".to_string(), json!([]));
        output.insert("tasks".to_string(), json!([]));
        output.insert("plans".to_string(), json!([]));
        output.insert("features_total".to_string(), json!(1));
        output.insert("feature_id".to_string(), json!("feature-1"));
        output.insert("passes".to_string(), json!(true));
        output.insert("utc_iso".to_string(), json!("2026-06-30T00:00:00Z"));
        output.insert("slept_seconds".to_string(), json!(1));
        output.insert("remaining_tokens".to_string(), json!(1000));
        output.insert("context_window".to_string(), json!(2000));
        output.insert("usage_percent".to_string(), json!("50%"));
        output.insert("format".to_string(), json!("png"));
        output.insert("size_bytes".to_string(), json!(12));
        output.insert("new_context_requested".to_string(), json!(true));
        output.insert("count".to_string(), json!(1));
        output.insert("total".to_string(), json!(1));
        output.insert("harness_profile".to_string(), json!("medium"));
        output.insert("task".to_string(), json!("fix renderers"));
        output.insert("hypotheses".to_string(), json!([]));
        output.insert("results".to_string(), json!([]));
        output.insert("has_changes".to_string(), json!(false));
        output.insert("manager".to_string(), json!("cargo"));
        output.insert("message".to_string(), json!("Goal set"));
        output.insert("file_path".to_string(), json!("src/lib.rs"));
        serde_json::Value::Object(output)
    }

    #[test]
    fn known_builtin_tools_have_explicit_full_rendering() {
        let builtin_names = [
            "read",
            "read_file",
            "search",
            "grep",
            "fs_browser",
            "list_dir",
            "glob",
            "write",
            "write_file",
            "apply_patch",
            "bash",
            "process",
            "question",
            "plan",
            "package_manager",
            "test_runner",
            "build_runner",
            "runtime_info",
            "code",
            "code_edit",
            "code_exec",
            "ast_search",
            "symbol_goto",
            "symbol_references",
            "dependency_graph_query",
            "test_discovery",
            "ownership_churn_query",
            "branch_race_start",
            "init_session",
            "mark_feature_done",
            "append_note",
            "history_ops",
            "current_time",
            "sleep",
            "set_goal",
            "get_context_remaining",
            "request_user_input",
            "sandbox",
            "view_image",
            "inspect_image",
            "new_context_window",
            "tool_search",
            "verifier",
            "wait",
            "subagent",
            "repo_explore",
        ];

        for name in builtin_names {
            let content = tool_full_content(
                &invocation(name, broad_input(name)),
                &ok_result(broad_output()),
            );
            assert!(
                !content.contains(&format!("{name} completed successfully")),
                "{name} fell back to generic success renderer:\n{content}"
            );
        }
    }

    #[test]
    fn read_alias_uses_file_renderer() {
        let content = tool_full_content(
            &invocation("read", json!({ "path": "src/lib.rs" })),
            &ok_result(json!({
                "path": "src/lib.rs",
                "content": "fn main() {}\n",
                "start_line": 1,
                "end_line": 1,
                "total_lines": 1
            })),
        );

        assert!(content.contains("Read src/lib.rs"));
        assert!(content.contains("```rust\nfn main() {}\n```"));
        assert!(!content.contains("read completed successfully"));
    }

    #[test]
    fn compact_renderer_covers_goal_and_wait() {
        let goal = tool_compact_text(
            &invocation(
                "set_goal",
                json!({ "objective": "fix every tool renderer", "token_budget": 10000 }),
            ),
            &ok_result(json!({ "message": "Goal set" })),
        );
        let wait = tool_compact_text(
            &invocation(
                "wait",
                json!({ "file_path": "src/lib.rs", "timeout_seconds": 30 }),
            ),
            &ok_result(json!({ "status": "released", "file_path": "src/lib.rs" })),
        );

        assert_eq!(goal, "Set goal \"fix every tool renderer\" (10000 tokens)");
        assert_eq!(wait, "Wait src/lib.rs (released)");
    }

    #[test]
    fn search_alias_full_view_renders_files() {
        let content = tool_full_content(
            &invocation("list_dir", json!({ "directory": "src" })),
            &ok_result(json!({
                "path": "src",
                "files": ["src/lib.rs", "src/main.rs"]
            })),
        );

        assert!(content.contains("Directory entries:"));
        assert!(content.contains("1  src/lib.rs"));
        assert!(content.contains("2  src/main.rs"));
        assert!(!content.contains("list_dir completed successfully"));
    }

    #[test]
    fn process_full_view_renders_stdout_and_structured_output() {
        let content = tool_full_content(
            &invocation("process", json!({ "command": "printf hi" })),
            &ok_result(json!({
                "exit_code": 0,
                "stdout": "hi\n",
                "stderr": "",
                "elapsed_ms": 12
            })),
        );

        assert!(content.contains("Exit code: 0"));
        assert!(content.contains("Stdout:\n```\nhi\n```"));
        assert!(content.contains("Output:\n```json"));
    }

    #[test]
    fn unknown_tool_full_view_includes_input_and_output_json() {
        let content = tool_full_content(
            &invocation("plugin__demo__lookup", json!({ "query": "abc" })),
            &ok_result(json!({
                "items": [{ "title": "Result" }],
                "count": 1
            })),
        );

        assert!(content.contains("plugin__demo__lookup completed successfully"));
        assert!(content.contains("Input:\n```json"));
        assert!(content.contains("\"query\": \"abc\""));
        assert!(content.contains("Output:\n```json"));
        assert!(content.contains("\"count\": 1"));
    }

    #[test]
    fn non_bash_error_preserves_structured_output() {
        let content = tool_full_content(
            &invocation("test_runner", json!({ "test_path": "bad" })),
            &err_result(json!({
                "error": "test command failed",
                "framework": "cargo",
                "stdout": "running 1 test"
            })),
        );

        assert!(content.contains("Error: test command failed"));
        assert!(content.contains("Output:\n```json"));
        assert!(content.contains("\"framework\": \"cargo\""));
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
