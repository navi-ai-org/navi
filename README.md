# NAVI

**Coding agent engine in Rust** — SOTA harness, embeddable SDK, WASM plugins, a sharp terminal UI, and security you can audit.

One runtime. Swap providers and clients. Keep sessions and config local.

```bash
cargo run -p navi-cli -- "find the main entrypoint and suggest a refactor"
```

```bash
cargo run -p navi-cli -- --no-tui "run tests and fix the first failure"
```

---

| Pillar | In one line |
|---|---|
| **Harness** | Profiles, structured coding tools, compaction, provider-correct tool loops — built for long repo sessions, not chat demos. |
| **SDK** | [`navi-sdk`](crates/navi-sdk): sessions, turns, event stream, approvals, host tools — same engine for TUI, headless, ACP, and your Rust app. |
| **Plugins** | WASM marketplace + lockfile; extend tools without forking the engine. Hot-reload after install. |
| **TUI** | Model picker, thinking levels, command palette, markdown/code view, compact tool trace (`ctrl+o`), plugins & providers in-app. |
| **Security** | Path scope, command blocklist, write/command approvals, plugin sandbox brokers, session secret redaction — tuned in TOML, not buried in prompts. |

```txt
Harness (navi-core)  →  tool loop, prompts, compaction
SDK (navi-sdk)     →  embed · events · host tools
CLI / TUI          →  daily driver in the terminal
Plugins (WASM)     →  marketplace + local install
```

## Quick start

```bash
cargo build
cargo run -p navi-cli -- "explain this codebase"
navi --print-providers          # after installing the binary
navi --acp                      # editor / ACP clients
```

Config: `~/.config/navi/config.toml` + `.navi/config.toml` per project. Keys from env or credential store; TUI prompts when a provider has no key.

## Harness (coding-first)

- **Tools:** `test_runner`, `build_runner`, `package_manager`, `fs_browser`, `read_file`, `write_file`, `apply_patch`, `grep`, `bash`
- **Modes:** Plan, Edit, Review, Tutor, Socratic, Recall, Focus (`tab` in TUI)
- **Context:** micro-compact, auto-compact, optional session memory
- **Harness:** `auto` / `small` / `medium` profiles for observation and loop limits

## SDK

```rust
let engine = NaviEngineBuilder::from_project(".").build()?;
let info = engine.start_session(NaviSessionRequest::default()).await?;
engine.subscribe_events(&info.id)?;  // tool.*, assistant.*, approval.*
engine.send_turn(NaviTurnRequest { session_id: info.id, message: "…".into(), .. }).await?;
```

[docs/sdk-agents.md](docs/sdk-agents.md) · [AGENTS.md](AGENTS.md)

## Plugins

```bash
navi plugin search
navi plugin install-marketplace <id> --yes
navi plugin install ./my-plugin --yes   # dev
```

Artifacts under `<data_dir>/plugins/<id>/`. Registry layout: [`marketplace/README.md`](marketplace/README.md).

## TUI

`ctrl+p` palette · `ctrl+m` models · `ctrl+s` sessions · `ctrl+o` tool I/O · `ctrl+d` debug · `ctrl+enter` send

Providers, plugins, OAuth (e.g. Copilot), and skills from the palette.

## Security (defaults you can change)

```toml
[security]
restrict_paths_to_project = true
protect_git_metadata = true
redact_secrets_in_sessions = true

[approvals]
require_for_writes = true
require_for_commands = true
```

Project config cannot enable MCP/plugins paths (global only). Details: [docs/plugin-system.md](docs/plugin-system.md).

## Workspace

`navi-core` · `navi-sdk` · `navi-cli` / `navi-tui` · `navi-openai` · `navi-mcp` · `navi-plugin-*`

## Docs & dev

[docs/index.md](docs/index.md) · [docs/user-guide.md](docs/user-guide.md) · `cargo test` (see [AGENTS.md](AGENTS.md) for thread limits)

Apache-2.0 — [LICENSE](LICENSE)
