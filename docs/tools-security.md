# Tools And Security

Tool execution is owned by `navi-core/src/tool.rs`. Security validation is owned by `navi-core/src/security.rs`.

## Built-In Tools

| Tool | Kind | Behavior |
|---|---|---|
| `read_file` | `Read` | Reads UTF-8 text files. Supports `start_line` and `end_line`. |
| `write_file` | `Write` | Writes full UTF-8 contents to a file, creating parent directories. |
| `apply_patch` | `Write` | Applies a unified diff using `git apply --whitespace=nowarn -`. |
| `list_files` | `Read` | Recursively lists files with optional substring filtering and max result cap. |
| `grep` | `Read` | Literal text search over readable text files. |
| `bash` | `Command` | Runs `bash -lc` with a timeout capped at 120 seconds. |

Tool output is truncated to avoid unbounded context/UI growth. `bash` stdout/stderr are each truncated.

The harness additionally compacts tool observations before sending them back to the model. The TUI can still show fuller tool output, but the model-facing observation should stay bounded for small and medium models.

## Skills And MCP

Skills are local `SKILL.md` directories loaded by `navi-core`. Active skills are prompt instructions only in the initial implementation; they do not execute scripts or register tools.

MCP support is a client integration in `navi-mcp` and is wired through `navi-sdk`. Stdio MCP servers configured under `[[mcp.servers]]` are spawned by the SDK, remote MCP tools are registered as NAVI tools with a prefix such as `<server_id>__<tool_name>`, and failed server connections warn without blocking session startup.

MCP tools are registered as `ToolKind::Custom`, so they require approval under the default security policy. Do not log full MCP payloads or secrets from configured server environments.

## Security Policy

`SecurityPolicy` validates:

- paths
- write intent
- patches
- command programs
- plugin paths
- tool kind

Default security config:

```toml
[security]
restrict_paths_to_project = true
protect_git_metadata = true
redact_secrets_in_sessions = true
allow_external_plugins = false
blocked_commands = ["rm", "rmdir", "shred", "mkfs", "dd", "sudo", "su", "doas"]
```

## Path Rules

By default:

- reads and writes are restricted to the project root.
- NAVI private storage is denied.
- writes into `.git` are denied.
- writes require approval.

When adding tools with file paths, ensure the path is visible through `path` or `file` input fields, or update `SecurityPolicy::path_from_invocation` so validation can see it.

## Command Rules

Commands are validated by program name against `blocked_commands`. Commands require approval by default.

The `bash` tool accepts a shell command string. This is powerful and risky; keep command approval enabled unless the user explicitly opts into a more autonomous mode.

## Approval Flow

`ToolExecutor::validate` returns:

- `Allow`
- `NeedsApproval`
- `Deny(reason)`

The TUI handles approval prompts unless YOLO/autonomous mode is enabled. Denied tools should produce a `ToolResult` with `ok = false` and a clear error.

Headless mode is approval-gated by default. Tools that require approval return an error observation instead of executing silently.

## Secret Redaction

Session persistence redacts likely secrets when `redact_secrets_in_sessions = true`.

Redaction catches:

- secret-like assignments, such as `OPENAI_API_KEY=...`
- long secret-like tokens
- common key/token naming patterns

When adding new persisted event content, run it through the redaction path or update `redact_agent_event`.

## Adding A Tool

1. Implement `Tool`.
2. Return a precise `ToolDefinition` with `ToolKind` and JSON schema.
3. Register it in `ToolExecutor::register_builtin_tools` or through plugin registration. Plugin tools must register executable `Tool` objects, not definition-only placeholders.
4. Make inputs security-visible. Path tools need `path` or `file`; command tools need `program` or `command`.
5. Add unit tests for success, failure, and security decisions.
6. Confirm UI rendering for compact and full tool views.

## Verification

```bash
cargo test -p navi-core tool security
cargo test
```
