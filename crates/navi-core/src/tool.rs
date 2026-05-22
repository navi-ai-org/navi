use crate::security::{SecurityDecision, SecurityPolicy};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

#[async_trait]
pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub kind: ToolKind,
    #[serde(default)]
    pub input_schema: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolKind {
    Read,
    Write,
    Command,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub id: String,
    pub tool_name: String,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub invocation_id: String,
    pub ok: bool,
    pub output: Value,
}

pub struct ToolExecutor {
    tools: HashMap<String, Arc<dyn Tool>>,
    policy: SecurityPolicy,
}

impl ToolExecutor {
    pub fn new(policy: SecurityPolicy) -> Self {
        let mut executor = Self {
            tools: HashMap::new(),
            policy,
        };
        executor.register_builtin_tools();
        executor
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

    pub fn definition(&self, name: &str) -> Option<ToolDefinition> {
        self.tools.get(name).map(|tool| tool.definition())
    }

    pub fn register_tool(&mut self, tool: Arc<dyn Tool>) -> Option<Arc<dyn Tool>> {
        self.tools.insert(tool.definition().name, tool)
    }

    pub fn validate(&self, invocation: &ToolInvocation) -> SecurityDecision {
        let Some(definition) = self.definition(&invocation.tool_name) else {
            tracing::warn!(tool = %invocation.tool_name, "unknown tool validation denied");
            return SecurityDecision::Deny(format!("unknown tool `{}`", invocation.tool_name));
        };
        let decision = self
            .policy
            .validate_tool_invocation(&definition, invocation);
        match &decision {
            SecurityDecision::Allow => {
                tracing::debug!(tool = %invocation.tool_name, invocation_id = %invocation.id, "tool validation allowed");
            }
            SecurityDecision::NeedsApproval(_) => {
                tracing::info!(tool = %invocation.tool_name, invocation_id = %invocation.id, "tool validation requires approval");
            }
            SecurityDecision::Deny(reason) => {
                tracing::warn!(tool = %invocation.tool_name, invocation_id = %invocation.id, reason = %reason, "tool validation denied");
            }
        }
        decision
    }

    pub async fn invoke(&self, invocation: ToolInvocation) -> ToolResult {
        let invocation_id = invocation.id.clone();
        let tool_name = invocation.tool_name.clone();
        let started_at = std::time::Instant::now();
        if let SecurityDecision::Deny(reason) = self.validate(&invocation) {
            tracing::warn!(tool = %tool_name, invocation_id = %invocation_id, reason = %reason, "tool invocation blocked");
            return ToolResult {
                invocation_id,
                ok: false,
                output: json!({ "error": reason }),
            };
        }
        let Some(tool) = self.tools.get(&invocation.tool_name).cloned() else {
            tracing::warn!(tool = %tool_name, invocation_id = %invocation_id, "unknown tool invocation");
            return ToolResult {
                invocation_id,
                ok: false,
                output: json!({ "error": format!("unknown tool `{}`", invocation.tool_name) }),
            };
        };

        tracing::info!(tool = %tool_name, invocation_id = %invocation_id, "tool invocation started");
        let result = match tool.invoke(invocation).await {
            Ok(result) => truncate_tool_result(result),
            Err(err) => ToolResult {
                invocation_id,
                ok: false,
                output: json!({ "error": format!("{err:#}") }),
            },
        };
        tracing::info!(
            tool = %tool_name,
            invocation_id = %result.invocation_id,
            ok = result.ok,
            duration_ms = started_at.elapsed().as_millis() as u64,
            "tool invocation finished"
        );
        result
    }

    fn register(&mut self, tool: impl Tool + 'static) {
        self.register_tool(Arc::new(tool));
    }

    fn register_builtin_tools(&mut self) {
        self.register(ReadFileTool);
        self.register(WriteFileTool);
        self.register(ApplyPatchTool);
        self.register(ListFilesTool);
        self.register(GrepTool);
        self.register(BashTool);
    }
}

struct ReadFileTool;
struct WriteFileTool;
struct ApplyPatchTool;
struct ListFilesTool;
struct GrepTool;
struct BashTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn definition(&self) -> ToolDefinition {
        definition(
            "read_file",
            "Read a UTF-8 text file from the current project, optionally specifying a line range.",
            ToolKind::Read,
            json_schema(
                &[
                    ("path", "Project-relative file path to read."),
                    (
                        "start_line",
                        "Line number to start reading from (1-indexed, defaults to 1).",
                    ),
                    (
                        "end_line",
                        "Line number to stop reading at (1-indexed, inclusive, defaults to start_line + 999).",
                    ),
                ],
                &["path"],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let path = required_string(&invocation.input, "path")?;
        let content =
            fs::read_to_string(&path).with_context(|| format!("failed to read {path}"))?;

        if content.is_empty() {
            return Ok(ok(
                invocation.id,
                json!({
                    "path": path,
                    "content": "",
                    "start_line": 1,
                    "end_line": 0,
                    "total_lines": 0,
                    "truncated": false,
                }),
            ));
        }

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start_line = optional_u64(&invocation.input, "start_line").unwrap_or(1);
        let end_line = optional_u64(&invocation.input, "end_line");

        let start_idx = (start_line.max(1) - 1) as usize;
        let end_idx = match end_line {
            Some(e) => (e as usize).clamp(start_idx, total_lines),
            None => (start_idx + 1000).min(total_lines),
        };

        let sliced_lines = if start_idx < total_lines {
            &lines[start_idx..end_idx]
        } else {
            &[]
        };

        let mut sliced_content = sliced_lines.join("\n");
        if !sliced_content.is_empty()
            && ((end_idx == total_lines && content.ends_with('\n')) || end_idx < total_lines)
        {
            sliced_content.push('\n');
        }

        let truncated = start_idx > 0 || end_idx < total_lines;

        Ok(ok(
            invocation.id,
            json!({
                "path": path,
                "content": sliced_content,
                "start_line": start_idx + 1,
                "end_line": end_idx,
                "total_lines": total_lines,
                "truncated": truncated
            }),
        ))
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn definition(&self) -> ToolDefinition {
        definition(
            "write_file",
            "Write full UTF-8 text content to a project file.",
            ToolKind::Write,
            json_schema(
                &[
                    ("path", "Project-relative file path to write."),
                    ("content", "Full UTF-8 content to write."),
                ],
                &["path", "content"],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let path = required_string(&invocation.input, "path")?;
        let content = required_string(&invocation.input, "content")?;
        if let Some(parent) = Path::new(&path).parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
        }
        fs::write(&path, content).with_context(|| format!("failed to write {path}"))?;
        Ok(ok(
            invocation.id,
            json!({ "path": path, "bytes": content.len() }),
        ))
    }
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn definition(&self) -> ToolDefinition {
        definition(
            "apply_patch",
            "Apply a unified diff patch to the current project.",
            ToolKind::Write,
            json_schema(&[("patch", "Unified diff patch text.")], &["patch"]),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let patch = required_string(&invocation.input, "patch")?;
        let mut child = Command::new("git")
            .args(["apply", "--whitespace=nowarn", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn git apply")?;
        child
            .stdin
            .as_mut()
            .context("failed to open git apply stdin")?
            .write_all(patch.as_bytes())
            .context("failed to send patch to git apply")?;
        let output = child
            .wait_with_output()
            .context("failed to wait for git apply")?;
        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: output.status.success(),
            output: json!({
                "status": output.status.code(),
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr),
            }),
        })
    }
}

#[async_trait]
impl Tool for ListFilesTool {
    fn definition(&self) -> ToolDefinition {
        definition(
            "list_files",
            "List project files, optionally filtering by substring.",
            ToolKind::Read,
            json_schema(
                &[
                    ("path", "Directory to list, defaults to current project."),
                    ("filter", "Optional substring filter."),
                    ("max_results", "Maximum number of files to return."),
                ],
                &[],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let root = optional_string(&invocation.input, "path").unwrap_or_else(|| ".".to_string());
        let filter = optional_string(&invocation.input, "filter");
        let max_results = optional_u64(&invocation.input, "max_results").unwrap_or(200) as usize;
        let mut files = Vec::new();
        collect_files(Path::new(&root), filter.as_deref(), max_results, &mut files)?;
        let truncated = files.len() >= max_results;
        Ok(ok(
            invocation.id,
            json!({ "files": files, "truncated": truncated }),
        ))
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn definition(&self) -> ToolDefinition {
        definition(
            "grep",
            "Search project text files for a literal pattern.",
            ToolKind::Read,
            json_schema(
                &[
                    ("pattern", "Literal text pattern to search for."),
                    (
                        "path",
                        "Directory or file to search, defaults to project root.",
                    ),
                    ("max_results", "Maximum number of matches to return."),
                ],
                &["pattern"],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let pattern = required_string(&invocation.input, "pattern")?;
        let root = optional_string(&invocation.input, "path").unwrap_or_else(|| ".".to_string());
        let max_results = optional_u64(&invocation.input, "max_results").unwrap_or(200) as usize;
        let mut matches = Vec::new();
        grep_path(Path::new(&root), &pattern, max_results, &mut matches)?;
        let truncated = matches.len() >= max_results;
        Ok(ok(
            invocation.id,
            json!({ "matches": matches, "truncated": truncated }),
        ))
    }
}

#[async_trait]
impl Tool for BashTool {
    fn definition(&self) -> ToolDefinition {
        definition(
            "bash",
            "Run a shell command in the current project.",
            ToolKind::Command,
            json_schema(
                &[
                    ("command", "Shell command to run."),
                    (
                        "description",
                        "Short reason for running this command, used for approval and trace context.",
                    ),
                    ("timeout_ms", "Timeout in milliseconds, capped internally."),
                ],
                &["command"],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let command = required_string(&invocation.input, "command")?;
        let timeout_ms = optional_u64(&invocation.input, "timeout_ms")
            .unwrap_or(30_000)
            .min(120_000);
        let output = match tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            tokio::process::Command::new("bash")
                .arg("-lc")
                .arg(command)
                .output(),
        )
        .await
        {
            Ok(output) => output.context("failed to run command")?,
            Err(_) => {
                return Ok(ToolResult {
                    invocation_id: invocation.id,
                    ok: false,
                    output: json!({ "error": "command timed out: deadline has elapsed" }),
                });
            }
        };

        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: output.status.success(),
            output: json!({
                "status": output.status.code(),
                "stdout": truncate_string(String::from_utf8_lossy(&output.stdout).to_string(), 64 * 1024),
                "stderr": truncate_string(String::from_utf8_lossy(&output.stderr).to_string(), 64 * 1024),
            }),
        })
    }
}

fn definition(
    name: &str,
    description: &str,
    kind: ToolKind,
    input_schema: Value,
) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        kind,
        input_schema,
    }
}

fn json_schema(properties: &[(&str, &str)], required: &[&str]) -> Value {
    let properties = properties
        .iter()
        .map(|(name, description)| {
            let is_integer = name.starts_with("max_")
                || *name == "timeout_ms"
                || name.ends_with("_line")
                || name.starts_with("start_")
                || name.starts_with("end_");
            (
                (*name).to_string(),
                json!({ "type": if is_integer { "integer" } else { "string" }, "description": description }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn required_string<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("missing required string `{key}`"))
}

fn optional_string(input: &Value, key: &str) -> Option<String> {
    input.get(key).and_then(Value::as_str).map(str::to_string)
}

fn optional_u64(input: &Value, key: &str) -> Option<u64> {
    input.get(key).and_then(Value::as_u64)
}

fn ok(invocation_id: String, output: Value) -> ToolResult {
    ToolResult {
        invocation_id,
        ok: true,
        output,
    }
}

fn truncate_tool_result(mut result: ToolResult) -> ToolResult {
    result.output = truncate_json(result.output, 128 * 1024);
    result
}

fn truncate_json(value: Value, max_bytes: usize) -> Value {
    let serialized = value.to_string();
    if serialized.len() <= max_bytes {
        value
    } else {
        json!({ "truncated": true, "content": truncate_string(serialized, max_bytes) })
    }
}

fn truncate_string(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    value.truncate(max_bytes);
    while !value.is_char_boundary(value.len()) {
        value.pop();
    }
    value.push_str("\n<truncated>");
    value
}

fn collect_files(
    root: &Path,
    filter: Option<&str>,
    max_results: usize,
    files: &mut Vec<String>,
) -> Result<()> {
    if files.len() >= max_results || should_skip(root) {
        return Ok(());
    }
    if root.is_file() {
        let display = root.display().to_string();
        if filter.is_none_or(|filter| display.contains(filter)) {
            files.push(display);
        }
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("failed to list {}", root.display()))? {
        if files.len() >= max_results {
            break;
        }
        collect_files(&entry?.path(), filter, max_results, files)?;
    }
    Ok(())
}

fn grep_path(
    root: &Path,
    pattern: &str,
    max_results: usize,
    matches: &mut Vec<Value>,
) -> Result<()> {
    if matches.len() >= max_results || should_skip(root) {
        return Ok(());
    }
    if root.is_file() {
        if let Ok(content) = fs::read_to_string(root) {
            for (index, line) in content.lines().enumerate() {
                if line.contains(pattern) {
                    matches.push(json!({
                        "path": root.display().to_string(),
                        "line": index + 1,
                        "text": line,
                    }));
                    if matches.len() >= max_results {
                        break;
                    }
                }
            }
        }
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("failed to list {}", root.display()))? {
        if matches.len() >= max_results {
            break;
        }
        grep_path(&entry?.path(), pattern, max_results, matches)?;
    }
    Ok(())
}

fn should_skip(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                ".git" | "target" | "node_modules" | ".cache" | ".venv" | "__pycache__"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SecurityConfig, SecurityPolicy};

    fn executor(root: &Path) -> ToolExecutor {
        let policy = SecurityPolicy::new(
            root.to_path_buf(),
            root.join(".navi-data"),
            SecurityConfig::default(),
        )
        .expect("policy");
        ToolExecutor::new(policy)
    }

    #[tokio::test]
    async fn builtins_read_write_and_grep_files() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let executor = executor(tempdir.path());
        let file = tempdir.path().join("src/lib.rs");

        let write = ToolInvocation {
            id: "write".to_string(),
            tool_name: "write_file".to_string(),
            input: json!({ "path": file.display().to_string(), "content": "pub fn marker() {}\n" }),
        };
        assert!(executor.invoke(write).await.ok);

        let read = executor
            .invoke(ToolInvocation {
                id: "read".to_string(),
                tool_name: "read_file".to_string(),
                input: json!({ "path": file.display().to_string() }),
            })
            .await;
        assert_eq!(read.output["content"], "pub fn marker() {}\n");
        assert_eq!(read.output["start_line"], 1);
        assert_eq!(read.output["end_line"], 1);
        assert_eq!(read.output["total_lines"], 1);
        assert!(!read.output["truncated"].as_bool().unwrap());

        // Test multi-line slicing
        let multiline_file = tempdir.path().join("src/multiline.rs");
        let write_multiline = ToolInvocation {
            id: "write_multiline".to_string(),
            tool_name: "write_file".to_string(),
            input: json!({ "path": multiline_file.display().to_string(), "content": "one\ntwo\nthree\nfour\n" }),
        };
        assert!(executor.invoke(write_multiline).await.ok);

        let read_slice = executor
            .invoke(ToolInvocation {
                id: "read_slice".to_string(),
                tool_name: "read_file".to_string(),
                input: json!({
                    "path": multiline_file.display().to_string(),
                    "start_line": 2,
                    "end_line": 3
                }),
            })
            .await;
        assert_eq!(read_slice.output["content"], "two\nthree\n");
        assert_eq!(read_slice.output["start_line"], 2);
        assert_eq!(read_slice.output["end_line"], 3);
        assert_eq!(read_slice.output["total_lines"], 4);
        assert!(read_slice.output["truncated"].as_bool().unwrap());

        let grep = executor
            .invoke(ToolInvocation {
                id: "grep".to_string(),
                tool_name: "grep".to_string(),
                input: json!({ "pattern": "marker", "path": tempdir.path().join("src").display().to_string() }),
            })
            .await;
        assert_eq!(grep.output["matches"][0]["line"], 1);
    }

    #[tokio::test]
    async fn bash_timeout_returns_structured_error() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let executor = executor(tempdir.path());

        let result = executor
            .invoke(ToolInvocation {
                id: "bash-timeout".to_string(),
                tool_name: "bash".to_string(),
                input: json!({ "command": "sleep 1", "timeout_ms": 1 }),
            })
            .await;

        assert!(!result.ok);
        assert_eq!(
            result.output["error"],
            "command timed out: deadline has elapsed"
        );
    }

    #[test]
    fn executor_definitions_include_input_schemas() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let executor = executor(tempdir.path());
        let read = executor.definition("read_file").expect("read_file");

        assert_eq!(read.input_schema["type"], "object");
        assert!(
            read.input_schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("path"))
        );
    }
}
