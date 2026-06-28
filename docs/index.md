# NAVI Documentation

NAVI is an opinionated, extensible code agent with a terminal UI, built in Rust. It supports multiple providers, built-in tools, plugins, MCP servers, and an SDK for embedding in other applications.

## User Guide

- [User Guide](user-guide.md) — Installation, quickstart, configuration, TUI controls, providers, tools, security, sessions, logs, and tips for code agents.
- [README](../README.md) — Project overview and capabilities.

## SDK & Integration

- [SDK Agents Guide](sdk-agents.md) — Embedding NAVI in other applications, engine API, runtime events, host tools, approval flow, and provider setup.
- [Runtime Customization Plan](runtime-customization-plan.md) — Plan for turning NAVI core into a composable runtime with custom security, harness, prompts, compaction, hooks, plugins, and SDK/NAPI integration.
- [AGENTS.md](../AGENTS.md) — Full technical reference for agents working on the NAVI codebase: architecture, crates, runtime flow, configuration, providers, tools, security, sessions, plugins, skills, MCP, and testing.

## Topic Guides

- [Conversation Compaction](compaction.md) — Micro-compact, auto-compact, and session memory behavior.
- [TUI Internals](tui.md) — TUI state, keybindings, rendering, modals, and performance rules.
- [Vision-Based Desktop Control Pipeline](vision-desktop-control.md) — Linux desktop-control architecture for vision-capable models using Wayland portals, PipeWire, libei/EIS, compositor context, and safety gates.

## Contributing

- [Contributing](../CONTRIBUTING.md) — Development workflow, code style, testing expectations.
- [Security Policy](../SECURITY.md) — Reporting vulnerabilities and disclosure policy.
- [License](../LICENSE) — Apache License 2.0.
