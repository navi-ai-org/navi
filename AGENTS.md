# Agent Guide for NAVI

Local agentic engine (Rust): runtime + tools + providers + sessions. TUI and Tutor are clients of the engine — not the product boundary.

**Domain docs (read only when the task touches them):** [docs/index.md](docs/index.md) · [sdk-agents](docs/sdk-agents.md) · [tui](docs/tui.md) · [auto-memory](docs/auto-memory.md) · [compaction](docs/compaction.md) · [goal-system](docs/goal-system.md) · [user-guide](docs/user-guide.md) · [ADRs](docs/adr/)

## Boundary

**Owns:** agent runtime, TUI/CLI, ACP, providers/auth, tools, security/approvals, sessions, tokens/context, plugins, events, project/code ops.

**Does not own:** Tutor visual UX (study canvas, mind maps, skill map, learning product layout).

```txt
NAVI Engine = runtime, tools, providers, sessions, events, approvals
NAVI TUI    = terminal frontend
NAVI Tutor  = visual learning frontend (same engine, no TUI deps)
```

## Non-negotiables

1. **Engine ≠ TUI.** Core behavior lives in `navi-core` (exposed via `navi-sdk`). Never put runtime logic in `navi-tui`. Never couple Tutor to TUI internals.
2. **Keep surfaces in sync** for any new engine capability (tool, API, config, event, memory):
   `navi-core` → `navi-sdk` → `navi-napi` → `navi-cli` (if user-facing) → `navi-tui` (if UI). No half-wired features.
3. **No worktree agent state.** Do not create `.navi/` or other project-local bookkeeping. State goes to `{data_dir}` (Linux: `~/.local/share/navi`), config to `~/.config/navi`, temp to OS temp. Project `.navi/config.toml` is user-authored only; never auto-create `.navi/`.
4. **No WebSocket/daemon as primary interface** unless explicitly requested; prefer stdio/headless/ACP.
5. **Plugins are WASM-only** (ADR 0013). Legacy native `[[plugins]]` paths are ignored.
6. **MCP is client-only** for now. Skills/MCP flow through `navi-sdk`.
7. **Stable, serializable engine APIs.** Small surface; events versioned for TUI and Tutor.

## Crates

| Crate | Role |
|---|---|
| `navi-cli` | Binary: CLI, config load, TUI or headless |
| `navi-core` | Runtime, config, tools, security, sessions, memory, registry |
| `navi-sdk` | Embedding facade (`NaviEngine`) for TUI/Tutor/ACP |
| `navi-tui` | Terminal UI client of the SDK |
| `navi-napi` | Node/Electron bindings of the full engine surface |
| `navi-providers` / `navi-openai` | Provider facade + OpenAI-compatible + adapters |
| `navi-mcp` | MCP stdio client → engine tools |
| `navi-plugin-*` | WASM runtime, orchestrator, manifest, brokers |

Depend on `navi-providers`, not `navi-openai` directly. `navi-sdk` is path-local (not crates.io).

## Config & state

Load order: defaults → `~/.config/navi/config.toml` → `.navi/config.toml`.

- Project config may override `model`, `harness`, `approvals`, `security`, `skills`, providers.
- **Project config cannot enable** plugins / wasm_plugins / MCP (ignored + warning). Install WASM via `navi plugin install` → `{data_dir}/plugins/`.
- Keys: env (`api_key_env`) → external auth → credential store. TUI must not prompt for keys on startup (model picker when missing).
- Sessions: `{data_dir}/sessions/` with secret redaction by default.
- Logs: `{data_dir}/logs/navi.log` — diagnostics only; never secrets, full prompts, or draw-path spam.

Providers: registry DB is [navi-ai-org/navi-registry](https://github.com/navi-ai-org/navi-registry); binary embeds `registry-snapshot/`, caches SQLite, pulls remote. Sync: `just sync-registry-snapshot` / `navi registry sync`. Details in code under `navi-core/src/registry/`.

## Tools & security

**Exposure:** small **Direct** core in schema; power tools **Deferred** (`tool_search` then call by name); aliases may be **Hidden**.

Core Direct (typical): `search`, `read_file`, `edit`, `write_file`, `bash`, `plan`, `question`, `tool_search`, `memory`, `set_session_title`. Prefer native tools over bash for read/edit/nav.

**Security defaults:** path jail to project; deny NAVI private storage and `.git` writes; writes/commands need approval by default; blocked destructive programs; file tools expose `path`/`file`, commands expose `program`/`command`. Modes: Restricted → AcceptEdits → Auto (guarded still) → Yolo. Session redaction on by default.

**Plan mode:** source of truth is markdown under `{data_dir}/plans/{session}.md` (design doc). Prefer `plan(write)` / `plan(submit)` or write/edit that file only — not JSON step arrays as primary content. See plan tool + `plan_store` / `plan_mode`.

## TUI (when editing `navi-tui`)

- No second Tokio runtime (CLI owns it). Async work → `AsyncEvent`.
- Use `crates/navi-tui/src/ui/` mini-framework; no one-off layout hacks. Extend the framework first if needed.
- Modal transitions via `UiEffect` (`OpenModal` / `ReplaceModal` / `CloseModal`…) so `Mode` and `ModalStack` stay synced.
- Key precedence: approval → cancel → global → modal. Do not log or do heavy IO in the draw path.
- Full module/key/render reference: [docs/tui.md](docs/tui.md).

## Validate

Prefer **smallest** package-scoped check. Agents use `cargo`, not full-product gates, unless shared runtime/SDK/plugins/MCP/ACP/providers or the user asks.

```bash
cargo fmt --all -- --check
cargo check -p <crate>
cargo test -p <crate> -- --test-threads=4
```

- Max **4** test threads; max **~500MB** per test process. Hanging / OOM tests are bugs.
- Humans/broad gates: `just verify`, `just ci`, `just test-crate <crate>` (see `justfile`).
- Headless: `cargo run -p navi-cli -- --no-tui TASK` (task required).

## Commits

Every commit needs a **minimal changelog** in the body (not subject-only for non-trivial work):

```text
type(scope): short imperative summary

### Changed
- Outcome for users/devs (not a file list)
```

Use `### Added` / `### Changed` / `### Fixed` / `### Removed` as needed; omit empty sections. Prefer outcomes over inventory. Conventional subject (`feat`/`fix`/`docs`/… + scope like `core`/`tui`/`sdk`).

## Gotchas

- Do not revert or mix work you did not make; treat **staged** changes as protected unless the user asks.
- Do not invent global effort lists — use model `effort_options` / `effort_binary` from registry/`list_models`.
- New session events: update `AgentEvent`, TUI load/replay if visible, and redaction.
- Leave untracked local scratch (e.g. `test_reqwest.rs`) alone unless asked.
- `target/` is gitignored; no committed rustfmt/clippy/CI config — use cargo defaults.
