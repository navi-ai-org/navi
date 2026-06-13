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
        "read_file" | "view_file" => read_file_summary(invocation, result),
        "write_file" => write_file_summary(invocation, result),
        "apply_patch" => apply_patch_summary(invocation),
        "bash" => bash_summary(invocation, result),
        "grep" => grep_summary(invocation, result),
        "fs_browser" => fs_browser_summary(invocation, result),
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

pub(crate) fn tool_detail_block(
    invocation: &ToolInvocation,
    result: &ToolResult,
) -> Option<String> {
    if !result.ok {
        return Some(tool_full_content(invocation, result));
    }

    match invocation.tool_name.as_str() {
        "apply_patch" => invocation
            .input
            .get("patch")
            .and_then(|v| v.as_str())
            .filter(|patch| !patch.trim().is_empty())
            .map(|patch| fenced_block("diff", patch)),
        "bash" => bash_detail_block(result),
        name if !matches!(name, "read_file" | "view_file" | "grep" | "fs_browser") => {
            generic_data_block(result)
        }
        _ => None,
    }
}

fn bash_detail_block(result: &ToolResult) -> Option<String> {
    let obj = result.output.as_object()?;
    let stdout = obj.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
    let stderr = obj.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
    if stdout.is_empty() && stderr.is_empty() {
        return None;
    }

    let mut content = String::new();
    if !stdout.is_empty() {
        content.push_str("Stdout\n");
        content.push_str(&fenced_block(
            "",
            truncate_to_lines(stdout, MAX_TOOL_RENDER_LINES),
        ));
    }
    if !stderr.is_empty() {
        if !content.is_empty() {
            content.push('\n');
        }
        content.push_str("Stderr\n");
        content.push_str(&fenced_block(
            "",
            truncate_to_lines(stderr, MAX_TOOL_RENDER_LINES),
        ));
    }
    Some(content)
}

fn generic_data_block(result: &ToolResult) -> Option<String> {
    if result.output.is_null() {
        return None;
    }
    if result
        .output
        .as_object()
        .is_some_and(serde_json::Map::is_empty)
    {
        return None;
    }
    serde_json::to_string_pretty(&result.output)
        .ok()
        .map(|json| fenced_block("json", &json))
}

fn fenced_block(language: &str, content: &str) -> String {
    let mut block = format!("```{language}\n");
    block.push_str(content.trim_end_matches('\n'));
    block.push('\n');
    block.push_str("```\n");
    block
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
    } else if invocation.tool_name == "write_file" {
        let path = obj.get("path").and_then(|v| v.as_str())?;
        let (added, removed) = write_file_line_counts(invocation, result);
        content.push_str(&format!(
            "Edited {} (+{added} -{removed} lines)\n",
            display_path(path)
        ));
    } else if invocation.tool_name == "apply_patch" {
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
    let Some(patch) = invocation.input.get("patch").and_then(|v| v.as_str()) else {
        return "Apply patch".to_string();
    };
    patch_edit_summaries(patch)
        .into_iter()
        .next()
        .unwrap_or_else(|| "Apply patch".to_string())
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
    let mut summary = format!("Run {}", one_line(command));
    if let Some(status) = result.output.get("status").and_then(|v| v.as_i64()) {
        summary.push_str(&format!(" (exit {status})"));
    }
    summary
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
