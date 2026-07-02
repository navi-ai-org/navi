<p align="center">
  <img width="1025" height="829" alt="NAVI terminal UI" src="https://github.com/user-attachments/assets/1e94e1ac-ba24-4c8e-846a-fee872dad809" />
</p>

<h1 align="center">NAVI</h1>

<p align="center">
  <strong>The coding agent engine that lives in your terminal.</strong><br/>
  Built in Rust. Fast. Secure. Embeddable. Yours. Less then ~35mb of memory usage per instance.
</p>

<p align="center">
  <strong>⚠️ Alpha — under active development. APIs, config formats, and behavior may change.</strong>
</p>

<p align="center">
  <a href="#install">Install</a> · <a href="#quick-start">Quick Start</a> · <a href="docs/user-guide.md">User Guide</a> · <a href="docs/sdk-agents.md">SDK Docs</a> · <a href="AGENTS.md">Architecture</a>
</p>

---

## Why NAVI?

**NAVI is not another chat wrapper.** It's a coding agent engine with a first-class terminal UI — designed for developers who live in the terminal and want an agent that can actually read, write, test, build, and ship code.

- **Real tools, not toy wrappers** — file R/W, apply-patch, grep, bash, test runner, build runner, package manager, sub-agents. All sandboxed, all auditable.
- **Multi-provider** — OpenAI, Anthropic, Google Gemini, OpenRouter, GitHub Copilot, xAI, or any OpenAI-compatible API. Swap in config, no recompile.
- **Session-aware** — conversation compaction, session memory, secret redaction. Survives long repo sessions without losing context.
- **Embeddable** — the same engine powers the TUI, headless CLI, and your Rust or Node.js app via `navi-sdk` / `@navi-agent/napi`.
- **Extensible** — WASM plugins, MCP servers, native host tools. Install without forking.
- **Secure by default** — path scoping, command blocklist, write/command approvals, plugin sandbox, session secret redaction. All in TOML, all auditable.

---

## Install

### Homebrew (macOS / Linux)

```bash
brew install navi-ai-org/tap/navi
```

### Cargo (from source)

```bash
cargo install navi-cli
```

Requires [Rust](https://rustup.rs). Builds the `navi` binary with full TUI and headless support.

### Shell installer (curl — macOS / Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh
```

Detects your OS and architecture, downloads the latest release binary, and installs it to `~/.local/bin` (or `~/.navi/bin` as fallback). Add to your `PATH`:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

### PowerShell installer (Windows)

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.ps1 | iex"
```

Detects your architecture, downloads the latest release binary, and installs it to `~/.navi/bin`. Prompts to add to your user `PATH` automatically.

### From source (git)

```bash
git clone https://github.com/navi-ai-org/navi.git
cd navi
just build
# binary: target/debug/navi
```

---

## Quick start

```bash
# First run — opens the TUI, walks you through provider setup
navi

# Headless mode — run a task without the TUI
navi --no-tui "find the main entrypoint and suggest a refactor"

# Pipe-friendly — reads from stdin
cat error.log | navi --no-tui "explain this error"

# See available providers and their credential status
navi --print-providers
```

Config lives in `~/.config/navi/config.toml` (global) and `.navi/config.toml` (per-project). API keys are read from environment variables first, then the credential store. The TUI walks you through setup when a provider has no key.

---

## What you get

### A real coding harness

Not a chatbot with a file tool. NAVI runs a structured agent loop with:

| Capability | What it does |
|---|---|
| **9 built-in tools** | `read_file`, `write_file`, `apply_patch`, `grep`, `bash`, `test_runner`, `build_runner`, `package_manager`, `fs_browser` |
| **Sub-agents** | Spawn isolated agents for parallel exploration, verification, or implementation tasks |
| **Harness profiles** | `auto` / `small` / `medium` — tune observation budgets and tool-loop limits per session |
| **Compaction** | Micro-compact (clears stale tool output), auto-compact (model-summarized context), session memory (survives across sessions) |
| **Plan & execute** | Built-in planning tool. See the plan, approve the steps, watch the agent execute |

### A terminal UI built for speed

- **Model picker** (`ctrl+m`) — switch providers and models mid-session
- **Command palette** (`ctrl+p`) — providers, plugins, skills, settings
- **Session manager** (`ctrl+s`) — save, resume, browse sessions
- **Tool I/O view** (`ctrl+o`) — inspect every tool call and result
- **Thinking levels** — `max` / `high` / `medium` / `low` / `off` per model
- **Markdown + code** — syntax-highlighted code blocks, tables, links, inline code
- **Mouse support** — scroll, select, copy

### An embeddable SDK

Same engine, different surfaces:

```rust
// Rust — navi-sdk
let engine = NaviEngineBuilder::from_project(".").build()?;
let session = engine.start_session(NaviSessionRequest::default()).await?;
engine.subscribe_events(&session.id)?;  // tool.*, assistant.*, approval.*
engine.send_turn(NaviTurnRequest { session_id: session.id, message: "refactor this".into(), .. }).await?;
```

```javascript
// Node.js — @navi-agent/napi
const navi = require('@navi-agent/napi');
const session = await navi.startSession({ projectDir: '.' });
const events = navi.subscribeEvents(session.id);
await navi.sendTurn(session.id, 'run the tests and fix failures');
```

### Plugin ecosystem

```bash
navi plugin search                  # discover plugins
navi plugin install-marketplace <id> --yes  # install from registry
navi plugin install ./my-plugin     # local dev plugin
```

WASM plugins are sandboxed, lockfile-tracked, and hot-reloadable. Native host plugins can extend tools, providers, context processors, and more.

### Security you can audit

```toml
[security]
restrict_paths_to_project = true    # agents can't escape your repo
protect_git_metadata = true         # .git is read-only
redact_secrets_in_sessions = true   # API keys never hit disk
blocked_commands = ["rm", "sudo"]   # customize the blocklist

[approvals]
require_for_writes = true           # approve before files change
require_for_commands = true         # approve before commands run
```

Project config can't enable MCP servers or plugin paths — those require global config. Supply-chain safety by design.

---

## Architecture

```
navi-cli        →  binary entrypoint, CLI parsing, mode selection
navi-tui        →  ratatui terminal UI (chat, modals, rendering)
navi-sdk        →  embedding facade (sessions, turns, events, approvals)
navi-core       →  harness, tools, security, compaction, sessions, config
navi-openai     →  provider streaming (OpenAI, Anthropic, Gemini, OpenRouter, Copilot, xAI)
navi-mcp        →  MCP stdio client (remote tool registration)
navi-napi       →  Node.js bindings (@navi-agent/napi)
navi-plugin-*   →  plugin API, host loader, broker, orchestrator, runtime
```

NAVI follows one rule: **the engine is not the UI.** The TUI is a powerful frontend. The SDK is the same engine for apps. No coupling, no leaking abstractions.

---

## Providers

| Provider | Protocol | Notes |
|---|---|---|
| **OpenAI** | Responses | GPT-5.5, GPT-4o, o-series, reasoning effort |
| **Anthropic** | Messages | Claude 4.5 Sonnet, Claude 4 Sonnet, extended thinking |
| **Google Gemini** | Generate Content | Gemini 2.5 Pro/Flash |
| **OpenRouter** | Chat Completions | 100+ models, auto-routing |
| **GitHub Copilot** | Chat Completions | Device OAuth, enterprise support |
| **xAI** | Responses | Grok models, reasoning effort |
| **Custom** | OpenAI-compatible | Any API that speaks the protocol |

---

## Docs

| | |
|---|---|
| [User Guide](docs/user-guide.md) | Installation, config, TUI controls, providers, tools, security |
| [SDK Agents Guide](docs/sdk-agents.md) | Embedding NAVI, engine API, runtime events, host tools |
| [AGENTS.md](AGENTS.md) | Full architecture reference for contributors |
| [Compaction](docs/compaction.md) | How context management works |
| [TUI Internals](docs/tui.md) | State, keybindings, rendering |
| [Plugin System](docs/plugin-system.md) | WASM plugins, host tools, marketplace |

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The short version:

```bash
just setup-tools    # first time: install quality tooling
just verify         # fmt + check + test
just ci             # full pre-PR gate
```

---

## License

[Apache-2.0](LICENSE)
