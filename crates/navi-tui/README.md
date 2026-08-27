# navi-tui

[![Crates.io](https://img.shields.io/crates/v/navi-tui)](https://crates.io/crates/navi-tui)
[![License](https://img.shields.io/crates/l/navi-tui)](../LICENSE)

Terminal UI for [NAVI](https://github.com/navi-ai-org/navi) — a fast, keyboard-driven chat interface built with [ratatui](https://crates.io/crates/ratatui) and [crossterm](https://crates.io/crates/crossterm).

## Features

- **Chat view** — markdown rendering, fenced code blocks with syntax highlighting, and inline thinking display
- **Model picker** — fuzzy search across providers, with OAuth and API key setup inline
- **Command palette** — quick actions for new session, compact, retry, and more (`ctrl+p`)
- **Tool approval** — per-tool approve/deny overlay with security risk labels
- **Permission modes** — cycle through Restricted → AcceptEdits → Yolo with `shift+tab`
- **Session management** — save, load, and browse past sessions (`ctrl+s`)
- **Debug modal** — inspect active provider, model, session id, and diagnostics (`ctrl+d`)
- **Mouse support** — scroll, select text, and copy to clipboard
- **Compact/full tool output** — toggle detailed tool I/O with `ctrl+o`

## Architecture

The TUI is a **client** of the NAVI engine — it drives turns through `navi-sdk::NaviEngine` and never owns runtime logic.

```text
┌─────────────────────────────────────┐
│  navi-tui (ratatui + crossterm)     │
│  ┌───────────┐  ┌────────────────┐  │
│  │  view.rs   │  │ keybindings/   │  │
│  │  render.rs │  │ dispatch.rs    │  │
│  └───────────┘  └────────────────┘  │
│              ↕                       │
│         navi-sdk::NaviEngine         │
└─────────────────────────────────────┘
```

## Key shortcuts

| Shortcut | Action |
|----------|--------|
| `ctrl+p` | Command palette |
| `ctrl+m` | Model picker |
| `ctrl+n` | New session |
| `ctrl+s` | Session picker |
| `ctrl+o` | Toggle compact/full tool output |
| `ctrl+d` | Debug modal |
| `ctrl+g` | Toggle YOLO mode |
| `shift+tab` | Cycle permission mode |
| `ctrl+enter` | Send prompt |
| `ctrl+c` | Quit |

## Part of the NAVI workspace

This crate depends on [`navi-sdk`](https://crates.io/crates/navi-sdk) and [`navi-core`](https://crates.io/crates/navi-core).

**Full project:** <https://github.com/navi-ai-org/navi>
**License:** Apache-2.0
