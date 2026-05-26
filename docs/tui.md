# TUI Guide

The TUI lives in `crates/navi-tui/src/lib.rs`. It uses a synchronous ratatui/crossterm event loop and an async bridge over `tokio::spawn` tasks. The CLI already owns the Tokio runtime; do not create another runtime inside the TUI.

## Main Concepts

- `TuiApp` stores UI state, credentials, session display state, tool approval UI state, an SDK engine handle, and async channels.
- `AsyncEvent` carries SDK runtime events, tool completion events, and model-sync results back into the event loop.
- `Mode` selects modal behavior: normal chat, commands, models, API key entry, thinking, sessions, settings, provider accounts.
- `ChatMessage` is display-oriented and may contain model labels, status, usage, thinking text, tool invocation/result metadata, or normal content.
- `ui::*` is the internal TUI framework layer. It owns reusable interaction primitives such as text editing, key handling outcomes, and layout sizing. Keep it private to `navi-tui`; do not move ratatui abstractions into `navi-sdk`.

## Keybindings

Key handling uses explicit precedence layers in this order:

- approval overlay
- normal-mode cancellation
- global shortcuts
- active mode/modal handler

If a layer handles a key, lower layers must not see it. This prevents double activation such as `Esc` closing a modal and also cancelling the active chat turn.

| Shortcut | Behavior |
|---|---|
| `ctrl+p` | Command palette |
| `ctrl+m` | Model picker |
| `ctrl+n` | New session |
| `ctrl+s` | Session picker |
| `ctrl+o` | Toggle compact/full tool output view |
| `ctrl+d` | Debug modal |
| `ctrl+enter` | Send prompt |
| `enter` | Insert newline |
| `ctrl+j` | Insert newline |
| `ctrl+c` | Quit |
| `/` on empty input | Command palette |
| `?` on empty input | Shortcuts |

## Input Editing

NAVI intentionally supports CamelHumps editing:

- `ctrl` word movement/deletion stops at camel humps and special characters.
- `alt` word deletion is broader and deletes until whitespace.

When editing input behavior, keep tests around:

- camel hump boundaries
- `ctrl+backspace`
- `alt+backspace`
- `ctrl+arrow`
- multiline cursor placement

## Chat Rendering

The chat renderer supports:

- markdown-ish prose rendering for headings, bullets, ordered lists, blockquotes, links, inline code, bold, italic, and tables.
- fenced code block syntax highlighting through `syntect`.
- compact tool rows by default.
- full tool input/output view when `ctrl+o` is enabled.
- visible thinking text when `Show Thinking Text` is enabled in settings.

Rendering is cached in `ChatRenderCache`. This is important for performance because syntect highlighting is expensive. If you change message fields that affect rendered output, update `chat_render_signature`; otherwise stale UI or unnecessary frame drops can happen.

## Tool Call Display

Default view:

- One compact line per tool result.
- Green ball for success.
- Red ball for error.

Full view:

- Enabled with `ctrl+o` or settings.
- Shows tool input and output.
- `read_file` output is rendered as fenced code when possible so syntax highlighting applies.

## Modals

The model picker includes provider/model search and refresh actions:

- `tab` refreshes the selected provider.
- `ctrl+r` refreshes all providers.
- selecting a model from a provider without a stored key opens the API key entry modal.

In the normal input flow, `tab` cycles the active agent mode (`/plan`, `/edit`, `/review`) rather than moving focus inside the input.

The settings modal currently controls:

- `Show Reasoning`
- `Verbose Tool Output`

Provider configuration is now in the command palette as `Providers`. That modal lists configured providers, shows whether each one is backed by an env var, stored credential, OpenCode auth, or is missing credentials, and supports:

- `enter` / `k` for API key setup.
- `o` for OAuth on supported providers.
- `r` to sync models for the selected provider.

The Debug modal (`ctrl+d`) shows the log path, session id, project, selected model/provider, active state, and recent diagnostics. It is intentionally read-only and should not render raw payloads or secrets.

## Performance Rules

- Do not run syntax highlighting, model filtering, provider sync, file IO, or network IO in the draw path without caching.
- Keep `render_*` functions deterministic and fast.
- Use async tasks for SDK runtime/model/provider operations and report back through `AsyncEvent`.
- Avoid rebuilding full chat render output on scroll-only frames.
- Do not emit normal logs from draw functions; log state transitions and async lifecycle events instead.

## Verification

For TUI changes:

```bash
cargo test -p navi-tui
cargo check
```

For key handling or rendering changes, add focused unit tests in `crates/navi-tui/src/lib.rs`.
