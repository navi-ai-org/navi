# NAVI Documentation

NAVI is an opinionated, extensible code agent with a terminal UI, built in Rust. It supports multiple providers, built-in tools, plugins, MCP servers, and an SDK for embedding in other applications.

## User Guide

- [User Guide](user-guide.md) — Installation, quickstart, configuration, TUI controls, providers, tools, security, sessions, logs, and tips for code agents.
- [README](../README.md) — Project overview and capabilities.

## SDK & Integration

- [SDK Agents Guide](sdk-agents.md) — Embedding NAVI in other applications, engine API, runtime events, host tools, approval flow, provider setup, and auto-memory API.
- [AGENTS.md](../AGENTS.md) — Short constitution for agents working in this repo (boundary, non-negotiables, security, validate, commits). Domain detail lives in the topic guides below — not in AGENTS.md.

## Topic Guides

- [Conversation Compaction](compaction.md) — Micro-compact, auto-compact, and session memory behavior.
- [Auto-Memory](auto-memory.md) — Persistent SQLite memory system with semantic search, extractMemories, auto-dream, and auto-distill.
- [Goal System](goal-system.md) — Goal lifecycle, verification, and budget tracking.
- [Harness System Vision](harness-system.md) — Loop/graph engineering, harness packs, materialize & feedback jobs, capability cards, plugins/MCP evolution.
- [TUI Internals](tui.md) — TUI state, keybindings, rendering, modals, and performance rules.

## Presentations

- [Harness Vision (HTML)](presentations/harness-vision/index.html) — Visual deck for the harness / loop / graph system.

## Architecture Decisions

- [ADR Index](adr/) — Architectural Decision Records (plugin runtime, broker model, security defaults, signing, sandboxing, etc.)

## Contributing

- [Contributing](../CONTRIBUTING.md) — Development workflow, code style, testing expectations.
- [Security Policy](../SECURITY.md) — Reporting vulnerabilities and disclosure policy.
- [License](../LICENSE) — Apache License 2.0.
