# NAVI User Guide

This guide covers how to install, configure, and use NAVI as a terminal code agent, including tips for code agents running NAVI headless.

## Installation

### Recommended — prebuilt binary

**macOS / Linux:**

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.ps1 | iex
```

Pin a release: `sh -s -- --version 0.1.2` (or set `NAVI_VERSION`).

### Containers

Linux release binaries are **musl**-built (Alpine) and run on Alpine, Debian, Ubuntu,
Amazon Linux, Rocky/RHEL-class images, and distroless bases — useful for agent sidecars
and CI.

```dockerfile
FROM alpine:3.20
RUN apk add --no-cache ca-certificates curl tar
RUN curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh \
 && mv /root/.local/bin/navi /usr/local/bin/navi
ENTRYPOINT ["navi"]
```

### From source


```bash
git clone https://github.com/navi-ai-org/navi.git
cd navi
cargo build -p navi-cli --release
# binary: target/release/navi
```

### Requirements

- For the curl installer: nothing beyond a normal OS (no Rust needed)
- For building from source: Rust 1.85+ (edition 2024)
- A provider API key (OpenAI, Anthropic, Gemini, xAI, OpenRouter, etc.)
## Quick Start

```bash
# Interactive TUI
cargo run -p navi-cli -- "explain this codebase"

# Headless mode
cargo run -p navi-cli -- --no-tui "write a hello world in Rust"

# Inspect resolved config and available providers
cargo run -p navi-cli -- --print-config
cargo run -p navi-cli -- --print-providers
```

## Configuration

Config is TOML, loaded in this order. Later sources override earlier ones.

1. **Defaults**: provider `openai`, model `gpt-5.5`
2. **Global config**: `~/.config/navi/config.toml` (Linux)
3. **Project config**: `.navi/config.toml` in the current working directory

> **Security note**: Project config (`.navi/config.toml`) cannot enable `plugins` or `mcp.servers`. These are only effective in the global config.

### Example Configuration

```toml
[model]
provider = "openai"
name = "gpt-5.5"

[approvals]
allow_reads = true
require_for_writes = true
require_for_commands = true

[security]
restrict_paths_to_project = true
protect_git_metadata = true
redact_secrets_in_sessions = true
allow_external_plugins = false
blocked_commands = ["rm", "rmdir", "shred", "mkfs", "dd", "sudo", "su", "doas"]

[harness]
profile = "auto" # auto, small, or medium

[logging]
enabled = true
level = "info"
file_enabled = true
stdout_enabled = false
include_payloads = false

[skills]
enabled = false
active = []

[mcp]
enabled = false

# MCP server example (global config only)
# [[mcp.servers]]
# id = "memory"
# command = "memory-mcp-server"
# args = []
# enabled = false

# Provider overrides
# [providers.openai]
# api_key_env = "OPENAI_API_KEY"
# base_url = "https://api.openai.com/v1"

# [providers.anthropic]
# api_key_env = "ANTHROPIC_API_KEY"
# base_url = "https://api.anthropic.com/v1"

# [providers.gitlawb]
# api_key_env = "LAWB_API_KEY"
# base_url = "https://opengateway.gitlawb.com/v1"
```

### Configuration Reference

| Section | Key | Description |
|---|---|---|
| `[model]` | `provider` | Provider id: `openai`, `anthropic`, `google-gemini`, `openrouter`, `github-copilot`, `gitlawb`, `xai`, `groq`, or custom. |
| `[model]` | `name` | Model name. |
| `[approvals]` | `allow_reads` | Allow read-only tools without approval. |
| `[approvals]` | `require_for_writes` | Require approval for write tools. |
| `[approvals]` | `require_for_commands` | Require approval for command tools. |
| `[security]` | `restrict_paths_to_project` | Limit file operations to project root. |
| `[security]` | `protect_git_metadata` | Deny writes into `.git/`. |
| `[security]` | `redact_secrets_in_sessions` | Redact likely secrets in persisted sessions. |
| `[security]` | `allow_external_plugins` | Allow loading plugins outside trusted locations. |
| `[security]` | `blocked_commands` | Commands denied by policy. |
| `[harness]` | `profile` | `auto`, `small`, or `medium`. Controls system prompt, tool-loop limits, and observation budgets. |
| `[logging]` | `enabled` | Enable/disable logging. |
| `[logging]` | `level` | Log level: `trace`, `debug`, `info`, `warn`, `error`. |
| `[logging]` | `file_enabled` | Write logs to file. |
| `[logging]` | `stdout_enabled` | Also write logs to stdout. |
| `[logging]` | `include_payloads` | Include raw payloads in logs (debug only). |
| `[skills]` | `enabled` | Enable skill discovery (store + built-ins). |
| `[skills]` | `active` | Skill ids to activate automatically. |
| `[mcp]` | `enabled` | Enable MCP client. |
| `[[mcp.servers]]` | `id`, `command`, `args`, `enabled` | MCP server definition. Global config only. |

### API Keys

API keys are resolved in this order:

1. Environment variable declared by the provider (e.g. `OPENAI_API_KEY`)
2. Provider-specific external auth sources (e.g. GitHub Copilot OAuth)
3. NAVI's credential store at `<data_dir>/credentials.toml`

If a selected provider has no key, the TUI asks for it from the model picker. Use `Providers` in the command palette to manage keys and trigger OAuth.

## Providers

NAVI routes all providers through `navi-openai`. Two protocol kinds are supported:

- `openai-responses`
- `openai-chat-completions`

Some providers need request/stream adapters even when they expose an OpenAI-compatible surface. The thinking selector uses levels `max`, `high`, `medium`, `low`, and `off`, mapped to provider-specific fields.

| Provider | Protocol | Notes |
|---|---|---|
| `openai` | responses | Reasoning effort via response fields. |
| `anthropic` | chat completions | Anthropic Messages streaming adapter. |
| `google-gemini` | chat completions | Gemini Generate Content adapter. |
| `openrouter` | chat completions | Required headers and reasoning config. |
| `github-copilot` | chat completions | GitHub device OAuth bearer tokens. |
| `xai` | responses | Platform API key (`XAI_API_KEY`) or xAI OAuth (browser/device) via `auth.x.ai`. OAuth also reuses `~/.grok/auth.json` when present. |
| `gitlawb` | chat completions | OpenAI-compatible gateway at `https://opengateway.gitlawb.com/v1`. |
| `groq` | chat completions | OpenAI-compatible with Groq adapter. |
| `xai` | responses | Reasoning effort mapping. |
| Custom | varies | Any OpenAI-compatible endpoint via `[providers.<id>]`. |

## TUI Controls

| Shortcut | Action |
|---|---|
| `ctrl+p` | Command palette |
| `ctrl+m` | Model picker |
| `ctrl+n` | New session |
| `ctrl+s` | Sessions / memory |
| `ctrl+o` | Toggle compact/full tool output view |
| `ctrl+d` | Debug modal |
| `ctrl+enter` | Send prompt |
| `enter` | Insert newline |
| `ctrl+j` | Insert newline |
| `ctrl+c` | Quit |
| `/` with empty input | Open command palette |
| `?` with empty input | Open shortcuts |

Input editing supports CamelHumps word movement: `ctrl` stops at camel humps and special characters, `alt` deletion is broader (to whitespace).

## Tools

Built-in tools are registered by `ToolExecutor` in `navi-core`:

| Tool | Kind | Purpose |
|---|---|---|
| `read_file` | Read | Read UTF-8 project files, optionally by line range |
| `write_file` | Write | Write full UTF-8 file contents |
| `apply_patch` | Write | Apply a unified diff with `git apply` |
| `grep` | Read | Literal text search over project files |
| `bash` | Command | Run a shell command with timeout, background tasks, and truncation |
| `test_runner` | Command | Run project tests with structured output (cargo/jest/vitest/bun/pytest/go) |
| `build_runner` | Command | Build/compile with caching and structured warnings/errors |
| `fs_browser` | Read | Browse filesystem: `list`, `tree`, `find`, `stat` |
| `package_manager` | Write | Manage deps: `install`, `add`, `remove`, `update`, `check` (npm/bun/cargo/go) |


All tools execute with the project root as working directory. Relative paths are resolved against the project root, not the process CWD.

## Security

The security layer validates tool kind, paths, commands, plugin paths, and approval requirements before execution.

- **Paths** are restricted to the project root by default.
- **Commands** are checked against `blocked_commands`.
- **Writes** require approval by default.
- **`.git`** writes are denied.
- **NAVI private storage** (`<data_dir>`) is denied.
- **Secrets** are redacted from persisted sessions by default.

See [SECURITY.md](../SECURITY.md) for reporting vulnerabilities.

## Sessions

NAVI persists sessions as JSON snapshots under `<data_dir>/sessions/`. Each session captures events (messages, tool calls, tool results, thinking text, deltas, approvals, and context packets).

### Compaction

NAVI implements these conversation management layers:

| Level | Trigger | Effect |
|---|---|---|
| Micro-compact | Time gap > 60 min since last assistant message | Clears read-only tool result content in-place |
| Auto-compact | `input_tokens + buffer >= context_window` | Summarizes conversation, replaces messages with summary |
| Session memory | Session end with compact summary | Saves summary to `<data_dir>/memory/`, injected on next session |
| Long-horizon memory | Context checkpoints/rebuilds | Stores checkpoint, notes, project memory, and history under `<data_dir>/memory/projects/<project_hash>/` |

See [compaction.md](compaction.md) for details.

Dream maintenance can synthesize existing memory and recent sessions into a separate reviewed memory store:

```bash
navi memory dream
navi memory dream --sessions 25 --instructions "Preserve implementation decisions"
navi memory dream --apply
```

Review-only dreams write to `<data_dir>/memory/projects/<project_hash>/dreams/dream-<timestamp>/`. `--apply` writes the review copy first, then replaces the active project and global memory files.

## Logs

NAVI writes structured diagnostics to `<data_dir>/logs/navi.log`. The log directory is private (`0700`) and the file is restricted (`0600`). Secrets are redacted by default.

```bash
navi --print-log-path
navi --log-level debug
navi --no-log-file
navi --debug-payloads --no-tui "inspect the project"
```

In the TUI, `ctrl+d` opens the Debug modal with the current log path, session id, provider/model, and recent diagnostics.

## Data Locations

| Type | Path |
|---|---|
| Global config | `~/.config/navi/config.toml` |
| Project config | `.navi/config.toml` |
| Credentials | `<data_dir>/credentials.toml` |
| Sessions | `<data_dir>/sessions/*.json` |
| Session memory | `<data_dir>/memory/<project_hash>.json` |
| Long-horizon memory | `<data_dir>/memory/projects/<project_hash>/` |
| Logs | `<data_dir>/logs/navi.log` |

`<data_dir>` is platform-dependent via the `directories` crate. On Linux it is typically `~/.local/share/navi/`.

## Headless Mode

Headless mode runs a single task and exits:

```bash
navi --no-tui "refactor the auth module to use async"
```

Headless mode requires a task argument. Approval is gated by default (no interactive prompt). Use environment variables for API keys in scripts and CI.

## ACP Server Mode

For editors and clients that speak the Agent Client Protocol:

```bash
navi --acp
```

ACP uses stdout/stdin for JSON-RPC. Diagnostics go to the log file. ACP cannot be combined with `--no-tui` or a task argument.

## Interactive Questions

NAVI runs as one agent. When it needs a user decision mid-turn, it can request an interactive question in the TUI with selectable options, a plain-text answer row, and an explicit deny action. `Esc` only closes the modal; use `ctrl+enter` to reopen a pending question.

## Tips for Code Agents

When running NAVI headless or embedding it:

- Always provide the task as a CLI argument in headless mode.
- Set API keys via environment variables; avoid interactive prompts.
- Use `--print-config` to verify resolved configuration before running tasks.
- Use `--print-providers` to list available providers and their credential status.
- Sessions are persisted automatically; use `--no-tui` for stateless scripted runs.
- Project config (`.navi/config.toml`) is untrusted: it cannot enable plugins or MCP servers.
- All tools run in the project root. Ensure your working directory is correct.
- Tool approval in headless mode is gated by default. Configure `[approvals]` to relax if needed.
