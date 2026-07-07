# TUI Guide

The TUI lives in `crates/navi-tui/src/`. Crate root `lib.rs` is now mostly module declarations, crate-local re-exports, and remaining integration-style tests. Supporting logic lives in sibling modules:

| Module | Role |
|---|---|
| `lib.rs` | Small crate entry/glue |
| `app.rs` | `TuiApp` aggregate state and constructor |
| `state.rs` | `ChatMessage`, `ChatRole`, `Mode`, `ModalKind`, selection, tool state |
| `theme.rs` | Color palette, logo, spacing constants, theme definitions |
| `commands.rs` | Command palette model and filtering |
| `keybindings.rs` | Key routing and modal handlers |
| `keybindings/` | Keybinding submodules for specific modes |
| `input.rs` | TuiApp input-field adapter helpers |
| `mouse.rs` | Mouse scrolling, text selection, and clipboard copy |
| `event_loop.rs` | Crossterm/ratatui terminal lifecycle and polling loop |
| `dispatch.rs` | `AsyncEvent` handling and runtime-event-to-UI mutations |
| `chat.rs` | Chat message/history mutations and assistant response lifecycle |
| `tools.rs` | TUI-side tool rows, approval state, and cancel flow |
| `providers.rs` | Model picker/provider account UI helpers |
| `view.rs` | TuiApp-dependent Ratatui rendering entry |
| `view/` | View submodules for specific UI areas |
| `stream.rs` | SDK turn spawning and streaming request bridge |
| `notifications.rs` | Notification and diagnostic state helpers |
| `render.rs` | Markdown rendering, syntax highlighting, tool formatting, input formatting |
| `render/` | Render submodules for specific content types |
| `runtime.rs` | SDK bridge (`NaviEngine` construction, `forward_runtime_event_to_tui`, OAuth) |
| `session.rs` | Saved-session listing, timestamp formatting, title extraction |
| `persistence.rs` | Current session save/load and preference persistence |
| `errors.rs` | Retry logic, error classification, delay parsing, `human_duration` |
| `plugins.rs` | Plugin listing, install, update, and reload |
| `plugin_approval.rs` | Plugin install/update approval UI |
| `testing/` | Test utilities and fixtures |
| `tests.rs` | Integration and cross-module tests |
| `ui/` | Internal Ratatui framework: `TextInput`, `ModalStack`, `SelectListState`, layout |

The event loop is synchronous ratatui/crossterm with an async bridge over `tokio::spawn` tasks. The CLI already owns the Tokio runtime; do not create another runtime inside the TUI.

## Main Concepts

- `TuiApp` stores UI state, credentials, session display state, tool approval UI state, an SDK engine handle, and async channels.
- `AsyncEvent` carries SDK runtime events, turn completion, retry triggers, OAuth completions, and model-sync results back into the event loop.
- `Mode` selects modal behavior: normal chat, commands, models, API key entry, thinking, sessions, settings, provider accounts, help, skills, plugins, plugin approval, questions, theme picker, and message actions.
- `ChatMessage` is display-oriented and may contain model labels, status, usage, thinking text, tool invocation/result metadata, or normal content.
- `ui::*` is the internal TUI framework layer. It owns reusable interaction primitives such as `TextInput`, `KeyOutcome`, `ModalStack`, `SelectListState`, `UiEffect`, and layout sizing. Keep it private to `navi-tui`; do not move ratatui abstractions into `navi-sdk`.

### Mode Enum

The `Mode` enum defines all modal states:

| Mode | Description |
|---|---|
| `Normal` | Default chat mode |
| `Commands` | Command palette |
| `Models` | Model picker |
| `ApiKeyEntry` | API key input |
| `Thinking` | Thinking level selector |
| `Sessions` | Session picker |
| `Settings` | Settings modal |
| `Providers` | Provider account management |
| `Debug` | Debug information modal |
| `Help` | Keyboard shortcuts help |
| `Skills` | Skill management |
| `Plugins` | Plugin marketplace |
| `PluginApproval` | Plugin install/update approval |
| `Question` | Interactive question modal |
| `ThemePicker` | Theme selector |
| `MessageActions` | Message action menu |

## Keybindings

Key handling uses explicit precedence layers in this order:

- approval overlay
- normal-mode cancellation
- global shortcuts
- active mode/modal handler

If a layer handles a key, lower layers must not see it. This prevents double activation such as `Esc` closing a modal and also cancelling the active chat turn.

Modal transitions should go through `UiEffect` helpers (`OpenModal`, `ReplaceModal`, `CloseModal`, `CloseAllModals`) so `Mode` and `ModalStack` stay synchronized. Do not set modal `Mode` directly in production code.

| Shortcut | Behavior |
|---|---|
| `ctrl+p` | Command palette |
| `ctrl+m` | Model picker |
| `ctrl+n` | New session |
| `ctrl+s` | Session picker |
| `ctrl+o` | Toggle compact/full tool output view |
| `ctrl+d` | Debug modal |
| `ctrl+h` | Help modal (keyboard shortcuts) |
| `ctrl+k` | Skills modal |
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

If the assistant needs a decision mid-turn, NAVI opens a question modal. Use `up`/`down` or `1-9` to choose, `space` to toggle multi-select answers, type any text to use a plain-text answer, `enter` to answer, and `n` to deny. `Esc` only closes the modal; `ctrl+enter` reopens a pending question.

The settings modal currently controls:

- `Show Reasoning` — toggle thinking text visibility
- `Verbose Tool Output` — toggle full tool input/output view
- `Thinking Level` — select thinking effort (adaptive/max/high/medium/low/off)
- `YOLO Mode` — auto-approve tools without confirmation
- `Theme` — select color theme

Provider configuration is now in the command palette as `Providers`. That modal lists configured providers, shows whether each one is backed by an env var, stored credential, OpenCode auth, or is missing credentials, and supports:

- `enter` / `k` for API key setup.
- `o` for OAuth on supported providers.
- `r` to sync models for the selected provider.

The Debug modal (`ctrl+d`) shows the log path, session id, project, selected model/provider, active state, and recent diagnostics. It is intentionally read-only and should not render raw payloads or secrets.

### Plugin Marketplace

The Plugins modal (`ctrl+p` → "Plugins") provides:

- Browse available plugins from the configured registry
- Install plugins with approval workflow
- Update installed plugins
- Reload WASM plugins without restarting

Plugin install/update requires approval via the `PluginApproval` modal, which shows:

- Plugin capabilities and tools
- Risk assessment (LOW/MEDIUM/HIGH/CRITICAL)
- Publisher and version information
- Warnings and security notes

### Message Actions

The Message Actions modal (accessible via right-click or keyboard shortcut on a message) provides:

- `Revert to here` — move message content back to input
- `Copy text` — copy message to clipboard
- `Fork from here` — start a new session from this point

## Performance Rules

- Do not run syntax highlighting, model filtering, provider sync, file IO, or network IO in the draw path without caching.
- Keep `render_*` functions deterministic and fast.
- Use async tasks for SDK runtime/model/provider operations and report back through `AsyncEvent`.
- Avoid rebuilding full chat render output on scroll-only frames.
- Do not emit normal logs from draw functions; log state transitions and async lifecycle events instead.

## Verification

For TUI changes:

```bash
just test-crate navi-tui
just check
```

For key handling or rendering changes, add focused unit tests in the corresponding module
(`render.rs`, `keybindings.rs`, `errors.rs`, etc.) or in `lib.rs` when testing cross-module
`TuiApp` behavior directly.
