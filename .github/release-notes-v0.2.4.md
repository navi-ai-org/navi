## Highlights

Large feature release after **0.2.3**: WASM plugins + marketplace, browser automation, remote voice, modular skills, full bindings parity, registry catalog, and substantial TUI/reliability work.

### Plugins & marketplace

- WASM-only plugins via `wasmtime` + host brokers (native plugin host removed)
- Marketplace catalog, example packages, validator, signed `hello-echo`, Discord MCP package
- Install side effects for skills and MCP (with merge confirmation)
- Host-mediated TUI extensions via `tui.json`; WASM runtime on by default

### Browser

- Built-in `browser` tool (Cloak/CDP backends)
- `navi browser status|doctor|install`; server routes and TUI hubs

### Voice

- Remote dictation clients: OpenAI, Groq, Wispr Flow
- `[voice]` config + registry transcription provider catalog
- SDK/CLI wire-up and remote doctor

### Skills

- Modular SQLite skill store with manage tools and CRUD APIs
- Drop filesystem `SKILL.md` discovery and deprecated `skills.dirs`

### Registry & models

- Remote canonical model catalog sync and provider base resolution
- Model-specific effort levels; remove adaptive thinking / learning tutor mode
- xAI Grok Build OAuth routing; Charm Hyper prefix-cache restored

### Tools & runtime

- `repo_explore` as BM25 + symbol search (not a subagent)
- Kill timed-out bash/process trees; background timeouts return `ok=false`
- Harden subagents; live progress after background spawn

### Sessions

- Rewind history when editing a past user message
- Persist partial model output on turn error; mid-stream prefill resume

### TUI

- Desktop notifications for finished unfocused jobs
- Self-update + About modal; setup wizard (approvals + marketplace tip)
- Plan as modal + live progress strip; Ctrl+Down jump to latest
- Paste while streaming; image lightbox; usage/palette/settings/MCP polish

### SDK / NAPI / Dart

- Full engine surface for voice, memory, MCP, skills, plugins, accounts, rewind, updates
- Docker binding verifier: `scripts/test-bindings-docker.sh`
- `@navi-agent/napi` **0.2.4** and platform packages; `navi-dart` **0.2.4**

### Performance & CI

- Cut session bloat, SQLite thrash, and streaming TUI cost
- CI no longer runs on bare pushes to `main`
- Multi-agent tool-quality benchmark suite

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh -s -- --version 0.2.4
```

## Changelog

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.3...v0.2.4
