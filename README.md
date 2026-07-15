<p align="center">
  <img src="assets/brand/navi-icon.jpg" width="160" height="160" alt="NAVI logo" />
</p>

<h1 align="center">NAVI</h1>

<p align="center">
  <strong>The coding agent engine that lives in your terminal.</strong><br/>
  Same harness for TUI, headless, edge, and apps. Multi-provider. Built in Rust. Under ~35&nbsp;MB RAM.
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#quick-start">Quick start</a> ·
  <a href="#why-navi">Why NAVI</a> ·
  <a href="docs/user-guide.md">User guide</a> ·
  <a href="https://github.com/navi-ai-org/navi/releases">Releases</a>
</p>

<p align="center">
  <sub>Beta — APIs and config may still change. Pin a release for production use.</sub>
</p>

<p align="center">
  <img src="assets/brand/navi-demo.gif" width="900" alt="NAVI terminal session — tools and model response" />
</p>

---

## Install

One line. No Rust toolchain. Prebuilt binary from [GitHub Releases](https://github.com/navi-ai-org/navi/releases).

**macOS / Linux**

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh
```

```bash
export PATH="$HOME/.local/bin:$PATH"   # if needed
navi
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.ps1 | iex
```

<details>
<summary>Other install options (Homebrew, npm, pin version, source, containers)</summary>

**Pin a version**

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh -s -- --version 0.2.4
```

**Homebrew**

```bash
brew install navi-ai-org/tap/navi
```

**npm**

```bash
npm install -g @navi-agent/navi
```

**From source** (Rust 1.85+ / edition 2024)

```bash
git clone https://github.com/navi-ai-org/navi.git && cd navi
just install-release   # or: cargo build -p navi-cli --release
```

**Containers** — Linux binaries are **musl** (Alpine). One artifact runs on Alpine, Debian, Ubuntu, Amazon Linux, Rocky/RHEL, and distroless:

```dockerfile
FROM alpine:3.20
RUN apk add --no-cache ca-certificates curl tar \
 && curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh \
 && mv /root/.local/bin/navi /usr/local/bin/navi
ENTRYPOINT ["navi"]
```

**Installer security** — HTTPS only · archive must match release `SHA256SUMS.txt` · optional Sigstore when `cosign` is installed · single-file root binary only (path traversal rejected).

```bash
# high-assurance: pin script commit + release + require cosign
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/<commit-sha>/scripts/install.sh \
  | sh -s -- --version 0.2.4 --require-cosign
```

</details>

---

## Quick start

```bash
navi                                          # TUI + first-run provider setup
navi --no-tui "find the main entrypoint"      # headless task
cat error.log | navi --no-tui "explain this"  # pipe-friendly
navi --print-providers                        # credential status
```

Config: `~/.config/navi/config.toml` (global) and `.navi/config.toml` (project).
API keys: environment variables first, then the credential store. The TUI guides setup when a provider has no key.

---

## Why NAVI?

Most “coding agents” are a chat UI glued to a few tools. **NAVI is an agent engine** with a terminal UI on top — the same loop that reads your repo, edits code, runs tests, and ships changes is available headless, in containers, or embedded in your app.

| You want… | You get… |
| --- | --- |
| **Work that finishes** | Structured agent loop: tools, plans, sub-agents, verification — not endless chat |
| **Any model** | OpenAI, Anthropic, Gemini, OpenRouter, Copilot, xAI, or any OpenAI-compatible API |
| **Speed in the terminal** | Native TUI (Rust + ratatui): model picker, sessions, tool I/O, markdown — no Electron |
| **Safety you can audit** | Path scope, command blocklist, write/command approvals, secret redaction — all TOML |
| **One engine, many surfaces** | TUI · headless CLI · `navi-lite` (edge) · Rust SDK · Node (`@navi-agent/napi`) |
| **Portable binary** | ~musl Linux for containers · macOS · Windows · under ~35&nbsp;MB RAM per instance |

**Not a wrapper.** Built-in tools include file R/W, apply-patch, grep, bash, test/build runners, package manager, and sub-agents — sandboxed and auditable.

---

## Surfaces

### Terminal UI

What you open when you type `navi`:

- **Model picker** `ctrl+m` — switch provider/model mid-session
- **Command palette** `ctrl+p` — providers, plugins, skills, settings
- **Sessions** `ctrl+s` — save, resume, browse
- **Tool I/O** `ctrl+o` — every call and result
- Thinking levels, syntax-highlighted markdown, mouse scroll/select/copy

### Headless CLI

CI, scripts, and agents talking to agents:

```bash
navi --no-tui "run the tests and fix failures"
```

### navi-lite — edge / embedded

Sealed, mission-scoped runtime for edge Linux prototypes. **No** TUI, MCP, plugins, embeddings, or registry sync — only the tools you allowlist.

```bash
# binary from the same GitHub Release as navi
./navi-lite --help
```

See [`crates/navi-lite/README.md`](crates/navi-lite/README.md).

### Embed the engine

Same harness inside your product:

```rust
// Rust — navi-sdk
let engine = NaviEngineBuilder::from_project(".").build()?;
let session = engine.start_session(NaviSessionRequest::default()).await?;
engine.send_turn(NaviTurnRequest {
    session_id: session.id,
    message: "refactor this".into(),
    ..Default::default()
}).await?;
```

```javascript
// Node.js — @navi-agent/napi
const navi = require('@navi-agent/napi');
const session = await navi.startSession({ projectDir: '.' });
await navi.sendTurn(session.id, 'run the tests and fix failures');
```

More: [SDK Agents Guide](docs/sdk-agents.md).

---

## Harness highlights

| Capability | What it does |
| --- | --- |
| **Built-in tools** | Core: `read_file`, `search`, `edit`, `write_file`, `bash`, `plan`, `question`, `tool_search`, `memory` — plus deferred power tools (`code`, `browser`, `package_manager`, `apply_patch`, …) |
| **Sub-agents** | Isolated agents for explore / verify / implement in parallel |
| **Compaction** | Micro-compact, auto-compact, session memory for long repos |
| **Plan & execute** | Plan tool → approve steps → watch execution |
| **Plugins & MCP** | WASM plugins (sandboxed) + MCP servers — install without forking |

```bash
navi plugin search
navi plugin install-marketplace <id> --yes
```

---

## Providers

| Provider | Notes |
| --- | --- |
| **OpenAI** | GPT-5.x, o-series, reasoning effort |
| **Anthropic** | Claude 4.x, extended thinking |
| **Google Gemini** | Gemini 2.5 Pro / Flash |
| **OpenRouter** | 100+ models, auto-routing |
| **GitHub Copilot** | Device OAuth, enterprise |
| **xAI** | Composer 2.5 and model family |
| **Custom** | Any OpenAI-compatible endpoint |

Swap in config — no recompile.

---

## Security (defaults on)

```toml
[security]
restrict_paths_to_project = true
protect_git_metadata = true
redact_secrets_in_sessions = true
blocked_commands = ["rm", "sudo"]

[approvals]
require_for_writes = true
require_for_commands = true
```

Project config cannot enable MCP servers or plugin paths — those require global config. Details: [User Guide](docs/user-guide.md) · [SECURITY.md](SECURITY.md).

---

## Architecture (short)

```
navi-cli     binary + modes
navi-tui     terminal UI
navi-sdk     embed facade (sessions, turns, events)
navi-core    harness, tools, security, compaction
navi-openai  providers (OpenAI, Anthropic, Gemini, …)
navi-lite    sealed edge runtime
navi-napi    Node bindings
navi-plugin-*  WASM + host plugins
```

**Rule:** the engine is not the UI. TUI and SDK share one core.

Full map: [AGENTS.md](AGENTS.md).

---

## Docs

| Doc | For |
| --- | --- |
| [User Guide](docs/user-guide.md) | Install, config, TUI, providers, tools, security |
| [SDK Agents](docs/sdk-agents.md) | Embed NAVI, events, host tools |
| [navi-lite](crates/navi-lite/README.md) | Edge / mission allowlist runtime |
| [CHANGELOG](CHANGELOG.md) | What’s new per release |
| [AGENTS.md](AGENTS.md) | Architecture for contributors |
| [Contributing](CONTRIBUTING.md) | `just verify` / `just ci` |

---

## Contributing

```bash
just setup-tools   # optional tooling
just verify        # fmt + check + test
just ci            # full pre-PR gate
```

See [CONTRIBUTING.md](CONTRIBUTING.md).

---

## License

[Apache-2.0](LICENSE)
