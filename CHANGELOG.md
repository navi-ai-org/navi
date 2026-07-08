# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-07-08

**First public multi-platform release of NAVI** — the coding agent engine that lives in your terminal.

This is the first tagged release that ships **prebuilt binaries** for Linux, macOS, and Windows, with a one-line installer and full GitHub Releases automation. NAVI is still **alpha**: APIs and config may change, but the core experience is ready to try.

### Highlights

- **One-line install** — download a prebuilt `navi` binary (no Rust toolchain required)
- **Terminal-first coding agent** — TUI chat, tools, approvals, sessions, and multi-provider models
- **Plan Mode** — collaborate on a plan before the agent writes code
- **Goals & memory** — structured goal checklists, SQLite memory, and dream consolidation
- **Extensible** — WASM plugins, MCP servers, registry-driven providers, embeddable SDK

### Install

```bash
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh

# Windows (PowerShell)
irm https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.ps1 | iex
```

Pin this version:

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh -s -- --version 0.2.0
```

### Binaries

| Asset | Platform |
| --- | --- |
| `navi-linux-x64.tar.gz` | Linux x86_64 |
| `navi-linux-arm64.tar.gz` | Linux ARM64 |
| `navi-darwin-x64.tar.gz` | macOS Intel |
| `navi-darwin-arm64.tar.gz` | macOS Apple Silicon |
| `navi-win32-x64.zip` | Windows x64 |
| `SHA256SUMS.txt` | Checksums |

### Added

#### Install & distribution

- Tag-triggered GitHub Actions **Release** workflow that builds `navi` for five targets and publishes a GitHub Release with archives + `SHA256SUMS.txt`
- Shell installer (`scripts/install.sh`) and PowerShell installer (`scripts/install.ps1`) as the **primary** install path
- Optional checksum verification against release `SHA256SUMS.txt`
- `just install-bin` for the curl-based installer from a checkout

#### Terminal UI & agent experience

- Full interactive TUI: chat, command palette, model/provider pickers, thinking controls, sessions
- **Plan Mode** with streaming `<proposed_plan>` parser, tool filtering (read-only while planning), and Confirm Plan UI
- **Auto** permission mode with guarded commands
- Multimodal attachments (images, audio, video, documents) with per-modality fallback analysis models
- Usage modal with session token totals and **estimated session cost** from registry list pricing (API-key / non-OAuth)
- Dreaming indicator and memory/dream commands in the palette
- Modular **copland** UI framework (panels, key routing, plugin-registered UI surfaces)

#### Providers & auth

- Multi-provider catalog via embedded registry + SQLite cache (OpenAI, Anthropic, Gemini, OpenRouter, xAI, Groq, GitHub Copilot, and more)
- Aggregator sync (e.g. OpenRouter model list) with capability tags and pricing conversion
- OAuth flows including **xAI / Grok CLI** login (browser + device + paste-code), OpenAI OAuth usage windows, OpenRouter usage
- Registry model **list pricing** (`input_per_1m` / `output_per_1m`) seeded into config for cost estimates

#### Goals, memory & harness

- Structured **goal** system with checklist pipeline, verification gates, auto-continuation limits, and feature flag
- SQLite-only memory architecture with embeddings by default and model-based dream consolidation
- Concurrent-safe SQLite (WAL, busy timeout) for multi-instance use
- Stable system instructions vs dynamic developer messages for better prompt caching behavior

#### Extensibility & embeddability

- Built-in tools: read/write/patch, search, bash, test/build runners, package manager, sub-agents, questions, and more
- MCP client integration (including deferred tool promotion when under threshold)
- WASM plugin host, broker, marketplace, and lockfile approvals
- **navi-sdk** Rust API and **@navi-agent/napi** Node bindings with compile-time SDK ↔ N-API parity checks
- Experimental **navi-server** remote surface and Dart bindings

#### Security

- Path scoping, command blocklists, write/command approvals
- Session secret redaction
- Plugin capability model and sandbox defaults

### Changed

- Install docs lead with curl/PowerShell binaries; Cargo install is for development from source
- Provider config merge no longer dumps full registry model lists into user `config.toml`
- Registry cache migration and re-seed when the embedded snapshot is newer
- Case-insensitive merge of user model overrides with registry metadata

### Fixed

- Registry: preserve model metadata during aggregator sync; avoid embedded snapshot clobbering synced models
- Registry: migrate older SQLite caches with non-null `task_size`
- Concurrent memory/dream race conditions across instances
- Compact image indicators and small-terminal TUI layout clipping
- Empty tool name parsing and deferred MCP tool visibility edge cases

### Notes

- **Alpha software.** Expect breaking changes in config, APIs, and provider metadata.
- OAuth-backed usage windows depend on provider support; session cost estimates use public list prices and may not match your invoice.
- Building from source requires a recent Rust toolchain (edition 2024).

## [0.1.2] - 2026-07-04

### Added

- Multimodal `ContentPart` support for images, audio, video, and documents across the engine and SDK-facing APIs
- Per-modality attachment fallback model configuration
- `analyze_attachment` host tool for specialist model analysis
- Registry attachment metadata (`defaults.attachments` / per-model overrides)

### Changed

- Unsupported attachments are rewritten into model-readable tool instructions when needed
- Gemini and Anthropic request mapping for native media parts where available
- TUI layout clips the footer/composer on small terminals

### Fixed

- Compact image indicators for user messages with images
- Registry background tasks without an active Tokio runtime

## [0.1.0] - 2026-06-29

### Added

- Initial open-source scaffold of the NAVI agent engine and TUI

[Unreleased]: https://github.com/navi-ai-org/navi/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/navi-ai-org/navi/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/navi-ai-org/navi/compare/v0.1.0...v0.1.2
[0.1.0]: https://github.com/navi-ai-org/navi/releases/tag/v0.1.0
