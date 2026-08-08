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

Providers: registry DB is [navi-ai-org/navi-registry](https://github.com/navi-ai-org/navi-registry); `navi-core` normally pins a commit in `crates/navi-core/registry.lock` and `build.rs` fetches/ embeds that snapshot, then caches SQLite and pulls remote. Release binaries build with `NAVI_REGISTRY_REF=refs/heads/main` to embed the latest snapshot. Sync lock: `just update-registry-lock` / `just fetch-registry` (offline cache). Runtime sync: `navi registry sync`. Offline builds set `NAVI_OFFLINE=1` and `NAVI_REGISTRY_DIR=<path>`. Details in code under `navi-core/src/registry/`.

## Tools & security

**Exposure:** small **Direct** core in schema; power tools **Deferred** (`tool_search` then call by name); aliases may be **Hidden**.

Core Direct (typical): `search`, `read_file`, `edit`, `write_file`, `run`, `plan`, `question`, `tool_search`, `memory`, `set_session_title`. Prefer native tools over `run` for read/edit/nav.

**Security defaults:** path jail to project; deny NAVI private storage and `.git` writes; writes/commands need approval by default; blocked destructive programs; file tools expose `path`/`file`, commands expose `program`/`command`. Modes: Restricted → AcceptEdits → Auto (guarded still) → Yolo. Session redaction on by default.

**Plan mode:** source of truth is markdown under `{data_dir}/plans/{session}.md` (design doc). Prefer `plan(write)` / `plan(submit)` or write/edit that file only — not JSON step arrays as primary content. See plan tool + `plan_store` / `plan_mode`.

## Tool testing requirements

Every tool in `navi-core/src/tool/builtin/` and every backend function in `navi-os-*/src/` that is callable from a tool **must** have three layers of tests. Coverage is enforced via `scripts/check-critical-coverage.py` (per-file floors + critical-function hits). Run `just coverage-tools` to check locally.

### Layer 1 — Unit tests (required, deterministic, no desktop)

Test pure logic: `definition()` metadata (name, kind, exposure, risk), input parsing, helper functions, error mapping, cache resolution. These must pass in CI without a desktop.

```rust
#[test]
fn my_tool_definition_is_deferred_read() {
    let tool = MyTool::new(/* minimal args */);
    let def = tool.definition();
    assert_eq!(def.name, "my_tool");
    assert_eq!(def.kind, ToolKind::Read);
}
```

### Layer 2 — Edge case tests (required, deterministic)

Test boundary and adversarial inputs. Every tool must cover at minimum:

- **Empty / null**: empty string, empty vec, `None`, missing JSON fields
- **Boundary values**: `0`, `usize::MAX`, negative numbers (where applicable), very large inputs (10k+ chars)
- **Invalid format**: malformed IDs, wrong types, garbage input
- **Unicode**: non-ASCII names, emoji, surrogate pairs, mixed scripts
- **Concurrency-relevant**: cache with 1000+ entries, poisoned mutex (if applicable)
- **Error paths**: every `bail!` / `Err` branch must have a test that verifies the error message contains a recovery hint or human-readable description (not just a raw code)

### Layer 3 — Integration tests (required, desktop-dependent, skip-on-failure)

Test the full `invoke()` path against the real backend (UIA, AX, shell). These call the actual OS APIs and must skip gracefully when the desktop is unavailable (headless CI, no foreground window, etc.).

```rust
#[tokio::test]
#[cfg(all(windows, feature = "computer-use"))]
async fn my_tool_invoke_works_against_real_backend() {
    let result = match tool.invoke(invocation).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    assert!(result.ok);
}
```

### Coverage gate

`scripts/check-critical-coverage.py` enforces per-file line-coverage floors and critical-function hit counts. Tool files are listed in `CRITICAL_FILES` with floors set at baseline. Floors ratchet up as tests improve — never lower a floor without justification.

```bash
just coverage-tools          # run coverage gate for tool crates
just coverage-core           # run coverage gate for navi-core (includes tools)
just coverage --list         # list all file coverage without gating
```

### What "100% coverage" means in practice

Line coverage is necessary but not sufficient. The real target is:

1. **Every error path** has a test that verifies the error message is model-friendly (includes a description + recovery hint, not just a code)
2. **Every public function** is called by at least one test (critical-function gate)
3. **Every edge case category** above has at least one test per tool
4. **Integration tests** verify the tool works against the real backend, not just mocks

A tool with 100% line coverage but no edge case tests or no integration tests is **not complete**.

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

### Windows

The workspace compiles and tests on Windows (CI runs `windows-latest` for fmt/test/clippy). Notes:

- **Run tool shell:** the `run` tool (renamed from `bash`) dynamically detects and uses the user's preferred shell. Resolution order: `[shell].program` in config.toml → `NAVI_SHELL` env → `SHELL` env (Unix) → `NAVI_BASH_SHELL` env (legacy) → platform default (`bash` on Unix, `pwsh`→`powershell` on Windows). The tool description is generated dynamically to tell the model which shell syntax to use. Set `[shell]` in config.toml to pin a shell: `program = "nu"` with optional `args = ["-c"]`. Supported shells: bash, zsh, pwsh (PowerShell 7+), PowerShell 5.1, nu, cmd, fish. The justfile recipes still use Git Bash or MSYS2 — ensure `bash` is on `PATH` for those.
- **Path separators:** tool outputs (search, fs_browser) normalize to forward slashes via `display_path`. Tests comparing paths should use `to_string_lossy().replace('\\', "/")` and strip the `\\?\` verbatim prefix from `canonicalize()` results.
- **Sandbox snapshots:** `SandboxManager::create_snapshot` stores raw (non-canonicalized) paths so that `PathBuf` equality works cross-platform. Non-existent file paths are added as roots (not their parent dir) so rollback doesn't try to delete locked sibling files (e.g. SQLite DBs).
- **Process termination:** background `run` tasks are assigned to a Win32 Job Object (`win_job` module in `run.rs`) so the entire process tree is killed on timeout. Unix uses `setsid` + process-group signals.
- **Shell commands in tests:** when constructing shell command strings from `PathBuf`, use forward slashes (`replace('\\', "/")`) because the shell token parser treats `\` as an escape character.
- **PTY smoke** (`pty_smoke.rs`) is Linux-only; CI gates it with `if: runner.os == 'Linux'`.
- **Coverage** (`cargo-llvm-cov`) stays Linux-only in CI.

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
- **Windows path pitfalls:** `canonicalize()` adds a `\\?\` prefix on Windows — strip it before string comparisons. Use `std::env::temp_dir()` instead of hardcoded `/tmp`. Gate Unix-only tests (symlinks, mode bits, PTY) with `#[cfg(unix)]`.
