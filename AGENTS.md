# Agent Guide for NAVI

NAVI is the local agentic engine and terminal-first code agent. It is implemented in Rust, has a ratatui/crossterm TUI for interactive sessions, supports headless/scripted execution, and exposes engine capabilities to other local clients.

## Product Boundary

NAVI owns:

- agent runtime
- TUI/CLI interaction
- ACP server
- model providers and provider auth
- built-in tools and host/plugin tool execution
- tool safety and approvals
- session execution
- token/context tracking
- plugin runtime
- event emission
- project/code operations

NAVI does not own:

- visual study canvas, mind maps, drawing UI
- study blocks UI, skill map UI, tutor dashboard
- learning product UX, Tauri frontend layout
- educational workflow design (except as engine capabilities exposed to clients)

## Architecture

The TUI is not the engine.

```txt
NAVI Engine = runtime, tools, providers, sessions, events, approvals
NAVI TUI    = terminal frontend for technical builders
NAVI Tutor  = visual learning frontend using the same engine
```

Core agent behavior belongs in engine crates, primarily `navi-core`, not in `navi-tui`. The TUI is a powerful frontend/client of the engine. NAVI Tutor must be able to embed or drive the engine without depending on terminal UI internals.

### Crates

| Crate | Role |
|---|---|
| `navi-cli` | Entry binary. Parses CLI, loads config, starts TUI or headless runtime. |
| `navi-core` | Harness policy, config, provider catalog, model/tool/session abstractions, security policy, runtime. |
| `navi-mcp` | MCP stdio client integration that registers remote MCP tools with the engine. |
| `navi-napi` | N-API binding for Node.js/Electron. Wraps `navi-sdk` and exposes the full engine surface (sessions, turns, goals, credentials, skills, MCP, saved sessions, registry, plugins, events) as native TypeScript classes. Includes a panic hook for crash isolation. |
| `navi-openai` | `ModelProvider` implementation for OpenAI-compatible APIs and provider adapters. Implementation crate behind `navi-providers` facade. |
| `navi-plugin-api` | Plugin trait and `NAVI_PLUGIN_API_VERSION = 1`. |
| `navi-plugin-host` | Dynamic `.so`/`.dylib` loading via `libloading`. |
| `navi-providers` | Provider facade. Re-exports `navi-openai` public API. Downstream crates should depend on this, not `navi-openai` directly. |
| `navi-sdk` | Public embedding facade for local clients (Tutor, TUI, ACP). Wraps core runtime, provider setup, plugin loading, host tools, MCP, sessions and events. |
| `navi-tui` | Terminal UI with chat, model picker, thinking/settings/session modals, markdown/code rendering. Drives turns through `navi-sdk::NaviEngine`. |

### Runtime Flow

1. `navi-cli` loads `NaviConfig` from defaults, global config, and project config.
2. The selected provider config is resolved from the built-in catalog plus user overrides.
3. Credentials are resolved from environment variables first, then the credential store.
4. TUI mode creates a `TuiApp`; headless mode creates `AgentRuntime`; ACP mode serves JSON-RPC over stdio. TUI and ACP drive turns through `navi-sdk::NaviEngine`.
5. The harness layer selects a `small` or `medium` profile, builds the system prompt, and applies loop/observation limits.
6. A user prompt becomes a `ModelRequest` containing conversation history, selected model, thinking mode, and available tool definitions.
7. `navi-providers` (via `navi-openai`) streams `ModelStreamEvent` values back to the caller.
8. Text/thinking deltas update the active assistant message; tool calls go through `ToolExecutor` and `SecurityPolicy`.
9. Tool calls and tool results are sent back using provider tool-message protocol.
10. Completed assistant output, tool results, and harness traces are persisted as session events.

### Three Stable Surfaces

1. CLI/TUI for humans operating the agent directly in a terminal.
2. Runtime SDK / Rust API for NAVI Tutor/Tauri and other local apps embedding NAVI.
3. ACP stdio server for editors and external agent clients.
4. N-API binding (`navi-napi` / `@navi-agent/napi`) for Node.js and Electron apps. Exposes the full `NaviEngine` surface as native TypeScript classes with push-based event streaming and a panic hook for crash isolation.

Do not make WebSocket/daemon the primary interface unless explicitly requested. External process mode should prefer a stable stdio/headless runtime protocol first.

### Keep All Surfaces In Sync

When adding or modifying any engine capability (new tool, new API method, new config field, new event type, new memory feature), you **must** update all affected surfaces in the same change:

1. **`navi-core`** — implement the feature (tool, runtime, config, types).
2. **`navi-sdk`** — expose it as a method on `NaviEngine` or re-export the relevant types from `navi-core`.
3. **`navi-napi`** — add a `#[napi]` binding so Node.js/Electron clients (NAVI Tutor) can call it.
4. **`navi-cli`** — add a CLI subcommand if the feature is user-facing (e.g. `navi memory init`).
5. **`navi-tui`** — wire any UI-facing behavior (shortcuts, modals, display state).

Do not leave a feature half-wired. If a new `NaviEngine` method exists in `navi-core` but is not exposed in `navi-sdk`, not bound in `navi-napi`, and not reachable from the CLI or TUI, the change is incomplete. Every public engine surface should be able to use the feature.

### Agent Test Scope Rule

When validating agent-made changes, prefer the smallest focused test/build command that covers the touched crate and behavior. For example, TUI-only changes should usually run `cargo test -p navi-tui -- --test-threads=4` rather than compiling or testing the full product.

Only run full-product gates such as `just verify`, `just ci`, or feature-heavy checks when the change touches shared runtime, CLI, SDK, plugins, MCP, ACP, provider wiring, or when the user explicitly asks for a broader gate.

### Engine API

NAVI exposes a small, serializable, UI-agnostic runtime API:

- `NaviEngine::start_session(...)` — creates a new agent session
- `NaviEngine::send_turn(...)` — sends a user message, returns structured events
- `NaviEngine::cancel_turn(...)` — cancels an active turn
- `NaviEngine::resolve_approval(...)` — resolves tool approval prompts
- `NaviEngine::resolve_plan_review(...)` — resolves plan tool user review
- `NaviEngine::resolve_sudo_password(...)` — resolves sudo password prompts (secret never in chat)
- `NaviEngine::add_context_packet(...)` — injects context from external sources
- `NaviEngine::set_model(...)` — changes model on an active session
- `NaviEngine::snapshot_session(...)` — takes a persistence snapshot
- `NaviEngine::list_models(...)` — lists available models
- `NaviEngine::set_model(...)` — selects a model
- `NaviEngine::subscribe_events(...)` — streams `RuntimeEvent`s
- `NaviEngine::list_provider_accounts(...)` — lists provider credentials
- `NaviEngine::set_session_skills(...)` — activates skills
- `NaviEngine::list_mcp_servers(...)` — lists MCP servers
- `NaviEngine::memory_write(...)` — saves a persistent memory
- `NaviEngine::memory_read(...)` — reads a memory by id
- `NaviEngine::memory_list(...)` — lists memories, optionally filtered by status
- `NaviEngine::memory_search(...)` — searches memories by text query
- `NaviEngine::memory_update(...)` — updates memory fields and/or status
- `NaviEngine::memory_delete(...)` — deletes a memory
- `NaviEngine::memory_count(...)` — returns count of active memories
- `NaviEngine::memory_index()` — returns markdown index for prompt injection
- `NaviEngine::voice_status()` — config, install path, recorders, streaming flag
- `NaviEngine::voice_doctor()` — mic tools + model files + checksum diagnostics
- `NaviEngine::voice_engine_installed(...)` — whether an ASR package is on disk
- `NaviEngine::voice_init(...)` — download + verify a local ASR engine package
- `NaviEngine::voice_transcribe_file(...)` — offline WAV → text (16 kHz mono path)
- `NaviEngine::voice_start_stream(...)` / `voice_push_pcm(...)` / `voice_end_stream()` / `voice_cancel_stream()` — client-fed streaming dictation (16 kHz mono f32)
- `NaviEngine::subscribe_voice_events()` — engine-global `VoiceEvent` stream (partial/final/error; not session-bound)
- Host tools through `SdkHostTool` and `HostToolHandler`

Structured events are serializable, versioned, and suitable for both TUI and Tutor: `session.started`, `turn.started`, `assistant.delta`, `assistant.thinking_delta`, `tool.requested`, `approval.required`, `tool.started`, `tool.completed`, `context.updated`, `tokens.updated`, `session.saved`, `turn.completed`, `session.finished`, `error`, `auto_dream.started`, `auto_dream.completed`, `auto_dream.failed`.

## Configuration

Config is TOML, loaded in this order:

1. Defaults: provider `openai`, model `gpt-5.5`
2. Global config: `~/.config/navi/config.toml` on Linux
3. Project config: `.navi/config.toml`

Key sections:

- `model`: provider id and model name.

Project config (`.navi/config.toml`) can override `model`, `harness`, `approvals`, `security`, `skills`, and provider entries. For supply-chain safety, **project config cannot enable** native `plugins`, `wasm_plugins`, or `mcp` servers — those are ignored with a warning. Install WASM plugins via `navi plugin install` (stored under `{data_dir}/plugins/`, loaded automatically). Configure extra WASM scan roots and native plugins in the **global** `~/.config/navi/config.toml`.
- `harness`: `auto`, `small`, or `medium` profile plus tool-loop and observation budgets.
- `approvals`: read/write/command approval behavior.
- `security`: path restrictions, `.git` protection, session redaction, plugin trust, blocked commands.
- `logging`: structured diagnostics, log level, file/stdout logging, debug payload opt-in.
- `providers`: built-in provider overrides or custom providers.
- `plugins`: native plugin library paths.

API keys are read from env vars first, provider-specific external auth sources next, then the credential store. The TUI must not ask for API keys on startup; it prompts from the model picker when selecting a provider without a resolved key.

### No Workspace Metadata

NAVI must not create agent metadata, lock files, caches, session state, progress files, generated registries, plugin state, or other internal bookkeeping inside the project/worktree. Do not create `.navi/` or any other hidden project directory for agent-owned state as a side effect of running tools.

Use platform/user locations instead:

- Durable app state: `{data_dir}` (Linux default: `~/.local/share/navi`)
- User configuration: `~/.config/navi`
- Ephemeral coordination and temporary files: `/tmp` or another OS temp directory
- Cacheable data: the platform cache directory

Project-local `.navi/config.toml` is allowed only as explicit user-authored project configuration. Other project-local `.navi` assets may be read only when explicitly supplied by the user or configuration; NAVI must never create `.navi/` automatically for internal state.

## Providers

All providers are routed through `navi-openai`. Configured protocol kinds:

- `openai-responses`
- `openai-chat-completions`

Some provider ids have special adapters:

- `anthropic` uses Anthropic Messages streaming.
- `google-gemini` uses Gemini Generate Content streaming.
- `openrouter` adds required OpenRouter headers and reasoning config.
- `github-copilot` uses GitHub device OAuth bearer tokens and Copilot request headers.
- `openai` and `xai` use Responses-style reasoning effort.

The UI effort picker is labeled **Effort Level** (not "thinking mode"). Levels shown are model-specific from registry `reasoning_levels` (mapped to `max`/`high`/`medium`/`low`/`off`/`adaptive`). When a model has no configured levels, the picker is binary: **thinking on** / **thinking off**. `ThinkingConfig::to_thinking_request` produces a normalized `ThinkingRequest` with `effort` and `budget_tokens` fields. Each provider converts these to its own wire format in the stream layer.

Tool transcripts must remain provider-correct. Chat Completions uses assistant `tool_calls` plus role `tool` results. Responses uses `function_call` and `function_call_output` input items.

Provider keys are resolved in this order:

1. Environment variable declared by `ProviderConfig.api_key_env`
2. Provider-specific external auth sources
3. Credential store under NAVI's data directory

### Provider Registry Database

Provider and model definitions live in a standalone repository: **[navi-ai-org/navi-registry](https://github.com/navi-ai-org/navi-registry)**. This is the single source of truth for provider configs — no hardcoded provider list exists in the NAVI binary.

How it works:

1. **Build-time embedding**: `crates/navi-core/registry-snapshot/` contains a vendored copy of the DB repo's `manifest.json` + `providers/*.json`. `build.rs` embeds these into the binary via `include_str!`. The embedded snapshot is the offline fallback and first-run seed.
2. **SQLite cache**: On first run, `RegistryStore::open()` seeds `<data_dir>/registry.db` from the embedded snapshot. Subsequent startups load from the cache.
3. **Remote pull**: If the cache is stale (> 24h) or the remote manifest version is newer, NAVI fetches the latest manifest and provider JSONs from `raw.githubusercontent.com/navi-ai-org/navi-registry/main`, verifies SHA-256 hashes, and updates the cache. This lets users on old NAVI versions get new providers without upgrading.
4. **Fallback chain**: SQLite cache → embedded snapshot → minimal hardcoded fallback (OpenAI only).

Key files:

| File | Role |
|---|---|
| `crates/navi-core/build.rs` | Embeds `registry-snapshot/` into the binary at build time |
| `crates/navi-core/src/registry/embedded.rs` | Parses the embedded snapshot into `RegistryProvider` values |
| `crates/navi-core/src/registry/fetcher.rs` | HTTP fetcher + SHA-256 integrity check |
| `crates/navi-core/src/registry/store.rs` | SQLite cache + `registry_provider_to_config()` conversion |
| `crates/navi-core/registry-snapshot/` | Vendored snapshot (updated by `just sync-registry-snapshot`) |

CLI commands:

```bash
navi registry sync   # Force-sync from the remote DB repo
navi registry list   # List providers and model counts from the cache
navi --print-providers  # Print full provider catalog as JSON
```

Update the embedded snapshot:

```bash
just sync-registry-snapshot
cargo check -p navi-core
```

To add a new provider, submit a PR to `navi-ai-org/navi-registry` with a `providers/<id>.json` file. Run `python scripts/validate.py` in the DB repo to validate and regenerate the manifest.

## Tools And Security

Built-in tools:

| Tool | Kind | Purpose |
|---|---|---|
| `read_file` | Read | Read UTF-8 project files, optionally by line range |
| `write_file` | Write | Write full UTF-8 file contents |
| `apply_patch` | Write | Apply a unified diff with `git apply` |
| `grep` | Read | Literal text search over project files |
| `bash` | Command | Run a shell command with timeout, background tasks, and truncation |
| `test_runner` | Command | Run project tests with structured output. Auto-detects cargo/jest/vitest/bun/pytest/go |
| `build_runner` | Command | Build/compile with caching. Returns structured warnings/errors. Skips rebuild if no source changed |
| `fs_browser` | Read | Browse filesystem: `list`, `tree`, `find`, `stat`. Replaces `list_files` |
| `package_manager` | Write | Manage deps: `install`, `add`, `remove`, `update`, `check`. Auto-detects npm/bun/cargo/go |
| `memory` | Write | Persistent auto-memory system with semantic search. Actions: `write`, `read`, `list`, `search`, `update`, `delete`. SQLite-backed with optional embedding model |
| `append_note` | Write | Append temporary observations to the session notes scratchpad (SQLite) |
| `history_ops` | Read | Query SQLite session history: `search`, `recent`, `get`, `summaries` |

`ToolExecutor` validates invocations through `SecurityPolicy` before execution. Reads are allowed by default, writes and commands require approval by default, blocked commands are denied, paths are restricted to the project by default, NAVI private storage is denied, and writes to `.git` are denied.

When adding tools, make security-sensitive inputs visible to policy validation. File tools should expose `path` or `file`; command tools should expose `program` or `command`.

### Security Policy

Default security config:

```toml
[security]
restrict_paths_to_project = true
protect_git_metadata = true
redact_secrets_in_sessions = true
allow_external_plugins = false
blocked_commands = ["rm", "rmdir", "shred", "mkfs", "dd", "sudo", "su", "doas"]
```

Path rules:
- reads and writes are restricted to the project root
- NAVI private storage is denied
- writes into `.git` are denied
- writes require approval

Command rules:
- commands are validated by program name against `blocked_commands`
- commands require approval by default
- long-running commands use `background = true` with `wait_ms` and `timeout_ms`

Approval flow:
- `ToolExecutor::validate` returns `Allow`, `NeedsApproval`, or `Deny(reason)`
- the TUI handles approval prompts unless YOLO/autonomous mode is enabled
- headless mode is approval-gated by default

Permission modes:
- `Restricted` — every tool call requires approval
- `AcceptEdits` — reads and writes auto-approved; commands require approval
- `Auto` — reads, writes, and commands auto-approved; `guarded_commands` (default: `git`) still require approval
- `Yolo` — everything auto-approved, no exceptions (most permissive)

`guarded_commands` (default: `["git"]`) — commands that always require approval in `Restricted`, `AcceptEdits`, and `Auto` modes. In `Yolo` mode, guarded commands are allowed like everything else.

Secret redaction:
- session persistence redacts likely secrets when `redact_secrets_in_sessions = true`
- catches secret-like assignments, long secret-like tokens, common key/token naming patterns

## TUI

The TUI lives in `crates/navi-tui/src/`. The event loop is synchronous ratatui/crossterm with an async bridge over `tokio::spawn` tasks. The CLI already owns the Tokio runtime; do not create another runtime inside the TUI.

### Modules

| Module | Role |
|---|---|
| `app.rs` | `TuiApp` aggregate state and constructor |
| `state.rs` | `ChatMessage`, `ChatRole`, `Mode`, `ModalKind`, selection, tool state |
| `theme.rs` | Color palette, logo, spacing constants |
| `commands.rs` | Command palette model and filtering |
| `keybindings.rs` | Key routing and modal handlers |
| `input.rs` | TuiApp input-field adapter helpers |
| `mouse.rs` | Mouse scrolling, text selection, and clipboard copy |
| `event_loop.rs` | Crossterm/ratatui terminal lifecycle and polling loop |
| `dispatch.rs` | `AsyncEvent` handling and runtime-event-to-UI mutations |
| `chat.rs` | Chat message/history mutations and assistant response lifecycle |
| `tools.rs` | TUI-side tool rows, approval state, and cancel flow |
| `providers.rs` | Model picker/provider account UI helpers |
| `view.rs` | TuiApp-dependent Ratatui rendering |
| `stream.rs` | SDK turn spawning and streaming request bridge |
| `notifications.rs` | Notification and diagnostic state helpers |
| `render.rs` | Markdown rendering, syntax highlighting, tool formatting, input formatting |
| `runtime.rs` | SDK bridge (`NaviEngine` construction, `forward_runtime_event_to_tui`, OAuth) |
| `session.rs` | Saved-session listing, timestamp formatting, title extraction |
| `persistence.rs` | Current session save/load and preference persistence |
| `errors.rs` | Retry logic, error classification, delay parsing, `human_duration` |
| `ui/` | Internal Ratatui framework: `TextInput`, `ModalStack`, `SelectListState`, layout |

### Main Concepts

- `TuiApp` stores UI state, credentials, session display state, tool approval UI state, an SDK engine handle, and async channels.
- `AsyncEvent` carries SDK runtime events, turn completion, retry triggers, OAuth completions, and model-sync results back into the event loop.
- `Mode` selects modal behavior: normal chat, commands, models, API key entry, thinking, sessions, settings, provider accounts.
- `ChatMessage` is display-oriented and may contain model labels, status, usage, thinking text, tool invocation/result metadata, or normal content.
- `ui::*` is the internal TUI framework layer. Keep it private to `navi-tui`; do not move ratatui abstractions into `navi-sdk`.

### TUI Mini-Framework Rule

Do not add ad hoc layout/rendering hacks in `navi-tui` to make one feature fit. New TUI features must use the internal mini-framework in `crates/navi-tui/src/ui/` and the shared render/layout helpers instead of bypassing them with one-off `ratatui::Layout`, manual viewport math, or hard-coded overflow/padding fixes.

If the existing mini-framework cannot support the feature cleanly, propose and implement the smallest reusable addition or modification to the mini-framework first, then build the feature on top of that. This keeps viewport bounds, modal state, list scrolling, input behavior, and rendering constraints consistent across the TUI.

### Keybindings

Key handling uses explicit precedence layers:

1. approval overlay
2. normal-mode cancellation
3. global shortcuts
4. active mode/modal handler

If a layer handles a key, lower layers must not see it.

Modal transitions should go through `UiEffect` helpers (`OpenModal`, `ReplaceModal`, `CloseModal`, `CloseAllModals`) so `Mode` and `ModalStack` stay synchronized.

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

### Input Editing

NAVI supports CamelHumps editing:

- `ctrl` word movement/deletion stops at camel humps and special characters.
- `alt` word deletion is broader and deletes until whitespace.

### Chat Rendering

The chat renderer supports:

- markdown-ish prose rendering for headings, bullets, ordered lists, blockquotes, links, inline code, bold, italic, and tables.
- fenced code block syntax highlighting through `syntect`.
- compact tool rows by default.
- full tool input/output view when `ctrl+o` is enabled.
- visible thinking text when `Show Thinking Text` is enabled in settings.

Rendering is cached in `ChatRenderCache`. If you change message fields that affect rendered output, update `chat_render_signature`.

### Tool Call Display

Default view: one compact line per tool result. Green ball for success, red ball for error.

Full view: enabled with `ctrl+o` or settings. Shows tool input and output.

### Modals

The model picker includes provider/model search and refresh actions:

- `tab` refreshes the selected provider.
- `ctrl+r` refreshes all providers.
- selecting a model from a provider without a stored key opens the API key entry modal.

Provider configuration is in the command palette as `Providers`. That modal lists configured providers, shows credential status, and supports:

- `enter` / `k` for API key setup.
- `o` for OAuth on supported providers.
- `r` to sync models for the selected provider.

The Debug modal (`ctrl+d`) shows the log path, session id, project, selected model/provider, active state, and recent diagnostics.

### Performance Rules

- Do not run syntax highlighting, model filtering, provider sync, file IO, or network IO in the draw path without caching.
- Keep `render_*` functions deterministic and fast.
- Use async tasks for SDK runtime/model/provider operations and report back through `AsyncEvent`.
- Avoid rebuilding full chat render output on scroll-only frames.
- Do not emit normal logs from draw functions.

## Compaction

NAVI implements a three-level conversation compaction system:

| Level | Trigger | Mechanism | Data Loss |
|---|---|---|---|
| Micro-compact | Time gap > 60 min since last assistant message | Clears read-only tool result content in-place | Tool output text only |
| Auto-compact | `input_tokens + buffer >= context_window` | Summarizes full conversation via model, replaces messages with system + summary | Full conversation replaced by summary |
| Session memory | Session end with compact summary | Saves summary to `<data_dir>/memory/<project_hash>.json`, injected on next session | None (additive) |

### Micro-Compact

`micro_compact(messages, gap_threshold_minutes)` clears the `content` of tool result messages whose `tool_name` is in the read-only set (`read_file`, `fs_browser`, `grep`, `bash`). Write tools are never cleared. Cleared content is replaced with `[Old tool result content cleared]`.

### Auto-Compact

`CompactState::auto_compact()` is called when `input_tokens + autocompact_buffer_tokens >= context_window` and the circuit breaker is not open.

The conversation is serialized to text and sent to the model with a prompt that produces a 9-section summary:

1. Pedido e Intenção Primária
2. Conceitos Técnicos-Chave
3. Arquivos e Trechos de Código
4. Erros e Correções
5. Resolução de Problemas
6. Todas as Mensagens do Usuário
7. Tarefas Pendentes
8. Trabalho Atual
9. Próximo Passo Opcional

After 3 consecutive failures, the circuit breaker opens. No further auto-compact attempts are made.

### Session Memory

When `MemoryConfig.session_memory_enabled` is true and a new session starts, the TUI loads the project memory and injects recent entries into the system prompt.

## Auto-Memory

NAVI implements a persistent auto-memory system with SQLite as the source of truth. The model can save, search, and manage memories through the `memory` tool, and a background extraction process automatically captures durable facts after each turn.

### Architecture

| Component | Module | Role |
|---|---|---|
| Auto-memory store | `memory/auto_memory.rs` | SQLite database with structured fields: id, type, name, description, body, embedding, confidence, status, evidence, timestamps |
| Embedding model | `memory/embedding.rs` | Qwen3-Embedding-0.6B via candle 0.11 (compiled by default). Generates 256-dim Matryoshka-truncated embeddings for semantic search |
| Auto-dream | `memory/auto_dream.rs` | 3-gate consolidation scheduler triggered after each turn |
| extractMemories | `memory/extract.rs` | Per-turn background memory extraction via model call |
| Dream maintenance | `memory/maintenance.rs` | Model-based consolidation + SQLite stale/dedup + embedding backfill |
| Memory tool | `tool/builtin/memory.rs` | Tool with 6 actions: write, read, list, search, update, delete |

### Memory Types

| Type | Description |
|---|---|
| `user` | Preferences, identity, working style |
| `feedback` | Behaviors to repeat or avoid |
| `project` | Non-derivable project context (deadlines, decisions) |
| `reference` | Links to dashboards, external docs |

### Memory Lifecycle

| Status | Meaning |
|---|---|
| `active` | Memory is current and injected into sessions |
| `needs_review` | Stale (last_seen > 30 days) — dream marks these |
| `obsolete` | Duplicated or contradicted — excluded from injection |

### Semantic Search

When the embedding model is present on disk, `memory(action='search')` uses cosine similarity over 256-dim embeddings. Falls back to text matching (SQL LIKE) when the model file is missing.

### extractMemories (Per-Turn Extraction)

After each completed turn, a background `tokio::spawn` calls the model to extract durable memories from the conversation. This is fire-and-forget and does not block the agent loop. Mutual exclusion: if the model already used the `memory` tool with `write` during the turn, background extraction is skipped.

### Auto-Dream (Periodic Consolidation)

Triggered after each turn via `try_auto_dream()`. Passes 3 gates before executing:

| Gate | Condition | Default |
|---|---|---|
| Time | `>= dream_interval_days * 24h` since last dream | 1 day (24h) |
| Sessions | `>= 5` sessions since last dream | 5 |
| Lock | No other process consolidating | File lock with PID + 1h stale detection |

When all gates pass, spawns a background consolidation: marks stale memories (>30 days), deduplicates, and backfills missing embeddings. The model-based dream (`navi memory dream --apply`) additionally uses a model call to consolidate the SQLite auto-memory index + global memory. When applied, the model receives all active SQLite memories with full body text and returns consolidation actions (mark obsolete, merge duplicates, update confidence) that are applied directly to the SQLite store.

### Auto-Distill

Same 3-gate pattern with `distill_interval_days` (default 30 days). Runs stale detection at 60 days + dedup. SOP extraction via model is available via `navi memory distill` (manual).

### CLI Commands

```bash
navi memory init                  # Create SQLite DB + directories
navi memory init --embeddings     # Download Qwen3-Embedding-0.6B GGUF + tokenizer
navi memory init --embeddings --force  # Re-download model
navi memory status                # Show memory system status
navi memory dream --apply         # Run model-based dream consolidation
navi memory distill               # Run SOP distillation
navi memory doctor                # Validate memory system health
navi memory history <query>       # Search session history
```

### Config

```toml
[memory]
enabled = true
dream_interval_days = 1           # Auto-dream interval (24h)
distill_interval_days = 30        # Auto-distill interval
embedding_model_path = ""         # Override GGUF path (empty = default)
embedding_tokenizer_path = ""     # Override tokenizer path (empty = default)
```

### Storage

All auto-memory state lives under `{data_dir}/memory/{project_hash}/`:

```
memories.db           ← SQLite (source of truth: memories, session_checkpoint, session_notes tables)
models/
  qwen3-embedding-0.6b-q8_0.gguf  ← Embedding model (optional)
  tokenizer.json                   ← Tokenizer (optional)
last_dream_at         ← Auto-dream timestamp
dream.lock            ← Cross-process dream lock
```

Global (cross-project) memory lives at `{data_dir}/memory/global-memory.db` (SQLite).

### SDK API

`NaviEngine` memory surface: CRUD (`memory_write` … `memory_index`) plus ops (`memory_status`, `memory_doctor`, `memory_init`, `memory_history_search`, `memory_dream`, `memory_distill`, `memory_checkpoint`). Voice: 10 methods (`voice_status` … `subscribe_voice_events`). Plugins: `plugin_list` / `plugin_info` / `plugin_search` / `plugin_install_*` / `plugin_update_*` / `plugin_remove`. Auth: `provider_supports_device_oauth`, `start_device_oauth`. Registry: `sync_registry`, `list_registry`. Effort: `list_models()` returns each `NaviModelInfo` with resolved `effort_options` + `effort_binary` (use these for pickers; do not invent a global effort list). Helpers: `effort_options_for_model`, `thinking_levels_for_model`, `is_binary_effort_model`, `effort_display_label`. All are also bound in `navi-napi` for Node.js/Electron (NAV Desktop) — `listModels()` JSON includes `effortOptions` / `effortBinary`; `sendTurn(..., { thinking })` accepts model levels plus binary `on`. Voice is engine-scoped (client-fed 16 kHz PCM + dedicated event stream).

### Events

| Event | When |
|---|---|
| `AutoDreamStarted` | 3 gates passed, dream spawned |
| `AutoDreamCompleted` | Consolidation finished (stale, duplicates, active count) |
| `AutoDreamFailed` | Consolidation error (lock released for retry) |

## Sessions

`SessionStore` saves `SessionSnapshot` JSON under `<data_dir>/sessions/`. Secret redaction is enabled by default through `SecurityConfig.redact_secrets_in_sessions`.

When adding event types:

- Add them to `AgentEvent`.
- Update session load/replay logic in the TUI if the event affects user-visible history.
- Confirm redaction still handles secret-bearing text.

## Plugins

Plugins are native libraries exporting `navi_plugin_entrypoint`. The host loads them with `libloading`, rejects incompatible `api_version` values, and registers executable plugin tools into the same `ToolExecutor` used by built-in tools.

Trusted plugin locations are enforced by `SecurityPolicy` unless `allow_external_plugins = true`. Failed plugins are reported as warnings and skipped.

Plugin scope:

- Engine plugins: providers, tools, context processors, routing, memory/session hooks, approval policies. Usable by TUI and Tutor.
- TUI plugins: terminal UI widgets, ratatui panels, keybindings, terminal commands, themes. Usable only by NAVI TUI.
- Tutor plugins: visual blocks, canvas tools, study behaviors, tutor widgets. Usable only by NAVI Tutor.

## Skills And MCP

Skills are built-ins plus rows in `data_dir/skills.sqlite`, managed by `navi-core` and injected as active prompt instructions. Active skills may also restrict tools via allow/deny lists. Do not implement marketplace/remote install unless explicitly requested.

MCP support starts as a client only. `navi-mcp` connects to configured stdio MCP servers and maps remote tools into `ToolExecutor`; do not make NAVI an MCP server yet. MCP and skill support should flow through `navi-sdk` so NAVI Tutor can consume them without TUI dependencies.

## Logging

NAVI uses `tracing` through `navi-core::logging`. File logs default to `<data_dir>/logs/navi.log` with private permissions on Unix. Logs are diagnostics, not session history. Keep them compact and redacted by default; raw payload logging is only for explicit debug mode. Do not log secrets, Authorization headers, credential-store values, full prompts, or full tool output. Avoid logging from TUI draw paths.

## Commands

### Cargo For Agents

Agents should use direct `cargo` commands for focused validation. Prefer package-scoped commands that match the touched crate and behavior:

```bash
cargo fmt --all -- --check
cargo check -p navi-tui
cargo test -p navi-tui -- --test-threads=4
cargo clippy -p navi-tui --all-targets
```

When testing, keep `--test-threads=4` unless debugging a single test. Use broader workspace checks only when the change touches shared runtime, CLI, SDK, plugins, MCP, ACP, provider wiring, or when explicitly requested.

### Just For Humans And Broad Gates

The root [`justfile`](justfile) provides convenient human-facing recipes and full-product gates.

First time on a machine: `just setup-tools` (installs rustquty collectors; see `just quality-doctor`).

| Task | Use |
|------|-----|
| List recipes | `just` |
| Build | `just build` |
| Format / check formatting | `just fmt` / `just fmt-check` |
| Typecheck | `just check` |
| All tests | `just test` |
| One crate | `just test-crate <crate>` (e.g. `navi-core`, `navi-tui`, `navi-openai`) |
| Clippy | `just clippy` |
| Fast gate (fmt + check + test) | `just verify` |
| Pre-PR gate | `just ci` |
| Quality (rustquty full) | `just analyze` or `just quality` |
| Quick quality (fmt + clippy) | `just quality-fast` |
| Coverage LCOV | `just coverage` |
| Coverage HTML | `just coverage-html` |
| Sync registry snapshot | `just sync-registry-snapshot` |

**Exceptions** (no `just` recipe yet — `cargo` is OK):

```bash
cargo run -p navi-cli -- TASK
cargo run -p navi-cli -- --no-tui TASK
cargo run -p navi-cli -- --print-config
cargo run -p navi-cli -- --print-providers
cargo run -p navi-cli -- registry sync
cargo run -p navi-cli -- registry list
```

Headless mode requires a task argument.

## Testing Expectations

Use focused tests while iterating and broader checks before handoff:

```bash
cargo fmt --all -- --check
cargo check -p navi-tui
cargo test -p navi-tui -- --test-threads=4
```

For human checkups or intentional full-product gates, `just test`, `just verify`, and `just ci` are still appropriate.

For targeted changes:

- TUI/key/rendering: `cargo test -p navi-tui -- --test-threads=4`
- provider/request/stream parsing: `cargo test -p navi-openai -- --test-threads=4`
- tools/security/session/config: `cargo test -p navi-core -- --test-threads=4`
- N-API binding: `cargo test -p navi-napi -- --test-threads=4`

### Resource Limits

Tests MUST respect resource constraints to avoid starving the host machine:

- **CPU**: Maximum 4 test threads. With `cargo test`, pass `-- --test-threads=4` unless debugging a single test.
- **Memory**: Maximum 500MB per test process. Use `ulimit -v 512000` (virtual memory) before running tests, or wrap commands with `systemd-run --scope -p MemoryMax=500M` if available.

```bash
cargo test -p navi-core -- --test-threads=4
cargo test -p navi-tui mouse::tests::mouse_drag -- --test-threads=4
```

If a single test exceeds 500MB or hangs for more than 60 seconds, it is a bug and must be fixed.

## KISS Rules

- Do not couple NAVI Tutor to NAVI TUI.
- Do not make NAVI Tutor depend on terminal UI internals.
- Do not force WebSocket/daemon before needed.
- Do not make plugin scope ambiguous.
- Do not put core runtime logic inside `navi-tui`.
- Keep engine APIs small, serializable, and stable.
- Keep TUI as a powerful frontend, not the product boundary.

## Gotchas

- The worktree may be dirty; do not revert changes you did not make.
- Treat staged changes as protected user/agent intent. Do not overwrite, unstage, amend, stash-pop over, or otherwise mix new work into staged changes unless the user explicitly asks. Before applying a stash/pop or any operation that can replay changes over the index, inspect `git status --short`, `git diff`, and `git diff --cached`, then preserve staged content or stop and ask.
- `target/` is gitignored.
- `test_reqwest.rs` may exist as an untracked local scratch file. Leave it alone unless the user explicitly asks.
- No CI, clippy, or rustfmt config is committed; use default cargo behavior.
- `navi-sdk` exists locally and is not published to crates.io. NAVI Tutor consumes it by path dependency.
