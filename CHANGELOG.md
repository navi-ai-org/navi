# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.6] - 2026-07-15

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.5...v0.2.6

### Bug Fixes

- **Edit/write path agency** — accept absolute paths (and stop hard-rejecting after path normalization); project path jail is forced only in Restricted mode (optional elsewhere via `restrict_paths_to_project`)
- **Tool error TUI** — structured errors (`error` / `error_code` / `hint`) render as plain Code/Hint text instead of raw ` ```json ` dumps
- **Global shortcuts** — Ctrl+M and other Ctrl+letter chords work across terminal encodings (ASCII control bytes / empty Ctrl+Enter → model picker)

### Bindings

- `@navi-agent/napi` 0.2.6 and platform packages
- Workspace crate versions bumped to 0.2.6

### Chores

- Version bump to 0.2.6

## [0.2.5] - 2026-07-15

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.4...v0.2.5

### New Features

- String-replace `edit` tool (with multi-edit) as the preferred coding edit path
- Lean Direct tool schema with deferred discovery via `tool_search`
- Message queue: remove items, clickable/hoverable `N queued` chip, preserve input draft on drain
- Session recap hard-capped to 3 lines

### Changes

- Remove `process`, `verifier`, and `branch_race_start` agent tools
- Redirect common bash file dumps (`sed`/`cat`/`rg`/`ls`) to native tools
- Drop rustquty quality metrics tooling from the repo

### Bindings

- `@navi-agent/napi` 0.2.5 and platform packages
- Workspace crate versions bumped to 0.2.5

### Chores

- Version bump to 0.2.5

## [0.2.4] - 2026-07-15

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.3...v0.2.4

### New Features

- Desktop notifications for finished unfocused jobs
- SDK / NAPI / Dart surface sync (remaining NaviEngine methods)
- Registry remote canonical model catalog + provider base resolution
- `repo_explore` BM25+symbol tool
- Model-specific effort options on list_models

### Bug Fixes

- Kill timed-out bash/process trees; background timeouts return ok=false (no hung “Waiting for model”)
- Stop duplicating tool error strings in TUI cards
- Grok CLI routing headers for xAI OAuth
- Charm Hyper: stop isolating prompt cache per session
- CI: do not run on bare main pushes
- TUI polish: usage, palette, settings, MCP status, diffs, streaming paste, subagent progress

### Bindings

- `@navi-agent/napi` 0.2.4 and platform packages
- `navi-dart` 0.2.4 C ABI gap-fill
- `scripts/test-bindings-docker.sh` Docker binding verifier

### Chores

- Version bump to 0.2.4

## [0.2.3] - 2026-07-09

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.2...v0.2.3

### New Features

- TUI: colored write/edit diffs, plain process/patch tool streams, boxed markdown tables
- TUI: jump-to-latest control, hover context % chip, session USD/credit spend tracking
- Session recap + per-turn context token meter; plan tools polish
- Local voice ASR surface for desktop clients
- Image hover previews (Kitty/Sixel/iTerm2) and `[Image N]` chips

### Bug Fixes

- **Prompt cache / quota**: stop double system prompt on Chat Completions; stable tool order for prefix caching; cache-aware Hyper credit estimates
- **Multimodal Grok/xAI**: treat unknown SKUs via provider defaults; fix Ctrl+R sync inheritance so new models (e.g. `grok-4.5`) get vision/context from defaults + family siblings instead of bare `NULL` rows
- Registry snapshot: add `grok-4.5` / `grok-4.20` with `supports_images`
- Context meter: include cached tokens from aggregator usage reports
- Charm Hyper credits reporting + embedded pricing fallback when SQLite pricing is empty

### Chores

- Version bump to 0.2.3
- CI runs on push/PR/tags; Release gates on tests before multi-platform publish
- ONNX voice is optional (`--features voice-onnx`) for portable musl/Windows builds

## [0.2.2] - 2026-07-09

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.0...v0.2.2

### New Features

- Add **`navi-lite`**: sealed, mission-scoped headless runtime for edge/embedded Linux prototypes (feature-gated `navi-core` without embeddings, TUI, MCP, or plugins)
- Ship **`navi-lite`** prebuilt binaries alongside full `navi` for all platforms
- Ship **portable musl Linux binaries** (Alpine toolchain) for containers and enterprise images
- Add **xAI Composer 2.5** models (`composer-2.5`, `grok-composer-2.5-fast`)
- Harden installers: strict SHA-256, single-file archives, optional Sigstore verification
- Sign `SHA256SUMS.txt` keyless with Sigstore (GitHub Actions OIDC)

### Bug Fixes

- Make `install.sh` POSIX/dash-safe (`curl | sh` on Ubuntu/Alpine)
- Reject unsafe multi-member release archives during install
- Fix Linux arm64 musl release builds (Docker Alpine on arm runners)
- Fix macOS package validation without bash `mapfile`

### Documentation

- Document `navi-lite` sealed edge runtime and mission allowlist model
- Install security controls and container/Linux portability notes
- Sample Alpine Dockerfile for agent sidecars

### Chores

- Drop OpenSSL/`native-tls` (`hf-hub` on rustls) to enable musl builds
- CI builds `navi-lite` and checks the lite binary
- Stricter multi-asset release packaging and checksum validation

## [0.2.0] - 2026-07-08

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.1.2...v0.2.0

### New Features

- First public multi-platform prebuilt binaries and one-line installer
- Plan Mode, goals, multi-provider registry, OAuth, session cost estimates

### Bug Fixes

- Registry merge, concurrent SQLite, deferred MCP tools, TUI layout

### Documentation

- Public install path and first-release notes

### Chores

- Dependency and registry snapshot updates for the binary release

## [0.1.2] - 2026-07-04

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.1.0...v0.1.2

### New Features

- Multimodal attachments and `analyze_attachment`

### Bug Fixes

- Compact image indicators; registry background tasks without Tokio runtime

### Documentation

- Multimodal release notes

### Chores

- Provider media request mapping updates

## [0.1.0] - 2026-06-29

Full changelog: https://github.com/navi-ai-org/navi/releases/tag/v0.1.0

### New Features

- Initial open-source scaffold of the NAVI agent engine and TUI

[Unreleased]: https://github.com/navi-ai-org/navi/compare/v0.2.6...HEAD
[0.2.6]: https://github.com/navi-ai-org/navi/compare/v0.2.5...v0.2.6
[0.2.5]: https://github.com/navi-ai-org/navi/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/navi-ai-org/navi/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/navi-ai-org/navi/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/navi-ai-org/navi/compare/v0.2.0...v0.2.2
[0.2.0]: https://github.com/navi-ai-org/navi/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/navi-ai-org/navi/compare/v0.1.0...v0.1.2
[0.1.0]: https://github.com/navi-ai-org/navi/releases/tag/v0.1.0
