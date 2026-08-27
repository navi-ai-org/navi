# NAVI 0.2.0 — First public multi-platform release

**The coding agent engine that lives in your terminal.**

This is the first official GitHub Release with **prebuilt binaries** for every major desktop platform. Install in one line — no Rust toolchain required.

> ⚠️ **Alpha** — under active development. APIs, config formats, and behavior may change.

---

## Install

### macOS / Linux (recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh
```

Pin this version:

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh -s -- --version 0.2.0
```

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.ps1 | iex
```

### Homebrew

```bash
brew install navi-ai-org/tap/navi
```

After install, run `navi` and complete provider setup in the TUI.

---

## What's included

| Area | What you get |
| --- | --- |
| **TUI agent** | Chat, tools, approvals, sessions, command palette, model picker |
| **Plan Mode** | Propose a plan first; confirm before execution |
| **Providers** | OpenAI, Anthropic, Gemini, OpenRouter, xAI/Grok, Groq, Copilot, and more |
| **Auth** | API keys + OAuth (incl. xAI Grok CLI paste/device flow) |
| **Goals** | Structured checklists, verification gates, safe auto-continuation |
| **Memory** | SQLite memory + embeddings + dream consolidation |
| **Usage** | Session tokens + estimated $ cost from registry list pricing |
| **Extensibility** | MCP, WASM plugins, marketplace, sandbox |
| **Embed** | Rust `navi-sdk` and Node `@navi-agent/napi` |

---

## Binaries

| File | Platform |
| --- | --- |
| `navi-linux-x64.tar.gz` | Linux x86_64 |
| `navi-linux-arm64.tar.gz` | Linux ARM64 |
| `navi-darwin-x64.tar.gz` | macOS Intel |
| `navi-darwin-arm64.tar.gz` | macOS Apple Silicon |
| `navi-win32-x64.zip` | Windows x64 |
| `SHA256SUMS.txt` | SHA-256 checksums |

Extract `navi` / `navi.exe` and put it on your `PATH`, or use the installers above.

---

## Highlights since the alpha line

- **Release pipeline** — tag `v*` → multi-platform builds → GitHub Release assets
- **curl installer** as the primary distribution path
- **Plan Mode** with streaming plan parser and confirm UI
- **Goal system** with checklists and continuation limits
- **Registry pricing** → session spend estimates for API-key users
- **xAI OAuth** and broader multi-provider usage reporting
- **Copland** modular TUI panel framework
- Concurrent-safe SQLite memory; embeddings on by default
- Multimodal attachments + specialist analysis fallbacks

Full notes: [CHANGELOG.md](https://github.com/navi-ai-org/navi/blob/v0.2.0/CHANGELOG.md)

---

## Quick start

```bash
navi                          # interactive TUI
navi --no-tui "explain this"  # headless
navi setup                    # provider login / interview
```

Docs: [User Guide](https://github.com/navi-ai-org/navi/blob/main/docs/user-guide.md) · [Architecture](https://github.com/navi-ai-org/navi/blob/main/AGENTS.md)

---

## License

Apache-2.0 — see [LICENSE](https://github.com/navi-ai-org/navi/blob/main/LICENSE).

Thank you for trying NAVI. Issues and PRs welcome at [navi-ai-org/navi](https://github.com/navi-ai-org/navi).
