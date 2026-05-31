# Code Agent Guidance

This document is for coding agents working on NAVI. It complements `AGENTS.md` with more durable implementation guidance.

## First Checks

Before changing code:

```bash
rg --files
git status --short
```

The worktree may be dirty. Do not revert user changes. If unrelated files are modified, ignore them. If a user change affects your task, work with it.

## Important Local Facts

- The repo root has no active `src/`; all code lives under `crates/`.
- The TUI is split across modules under `crates/navi-tui/src/`: `view/`, `keybindings/`, `render/`, `ui/`, plus focused files like `app.rs`, `chat.rs`, `runtime.rs`, etc.
- `navi-providers` is the provider facade; `navi-openai` is the implementation crate behind it.
- `navi-core` owns harness policy, security, tools, sessions, config, provider catalog, and model abstractions.
- `test_reqwest.rs` may be present as an untracked local scratch file; do not touch it unless explicitly asked.

## Editing Rules

- Prefer small, focused changes.
- Preserve the current Lain/NAVI visual direction unless the user asks to redesign it.
- Use `apply_patch` for manual edits.
- Add tests near the behavior changed.
- Keep provider/network code explicit; avoid implicit fallback behavior that hides provider incompatibilities.

## TUI Rules

- Do not perform expensive work in the draw path.
- If rendered output depends on new message fields, update `chat_render_signature`.
- Keep scrolling cheap; avoid rebuilding or syntax-highlighting the whole conversation on scroll-only frames.
- Keybinding changes need tests.
- Markdown/code rendering changes need tests.
- Tool display changes should cover compact and full views.

## Provider Rules

- Treat “OpenAI-compatible” as a starting point, not a guarantee.
- Add provider-specific thinking fields only when supported.
- Add stream parsing tests when accepting new SSE event formats.
- Keep API key handling out of startup prompts; missing keys should surface when a provider/model is selected or when a request needs the key.
- Preserve tool transcript protocol: assistant tool calls plus tool results for Chat Completions, function-call output items for Responses.

## Harness Rules

- Use `navi-core/src/harness.rs` for prompt, profile, loop, and observation policy.
- Do not add TUI-only or CLI-only system prompts.
- Keep small-model observations compact and deterministic.
- If a rendered or persisted trace field changes, update tests around `HarnessTrace`.

## Tool And Security Rules

- Any tool that writes or runs commands must go through `SecurityPolicy`.
- Keep path inputs security-visible.
- Do not add tools that can mutate files or run commands without tests for approval/denial paths.
- Preserve session redaction for secret-bearing text.

## Logging Rules

- Use `tracing` for diagnostics; avoid `println!`/`eprintln!` except CLI user output.
- Keep logs compact by default: lifecycle, retries, errors, timings, provider/model ids, tool names, and redacted summaries.
- Do not log raw API keys, Authorization headers, credential-store values, full prompts, or full tool outputs.
- TUI logging should happen on state transitions and async events, not inside render hot paths.
- If a new diagnostic can help users debug a stuck run, add it to the Debug modal's recent diagnostics list as well as the structured log.

## Recommended Verification

Small TUI change:

```bash
cargo fmt
cargo test -p navi-tui
cargo check
```

Provider change:

```bash
cargo fmt
cargo test -p navi-openai
cargo check
```

Tool/security/session change:

```bash
cargo fmt
cargo test -p navi-core
cargo check
```

Broad change:

```bash
cargo fmt
cargo check
cargo test
```

## Documentation Updates

Update these docs when behavior changes:

- `README.md` for user-facing capabilities and commands.
- `AGENTS.md` for concise agent instructions.
- `docs/architecture.md` for crate boundaries or runtime flow.
- `docs/tui.md` for keybindings, rendering, modals, or TUI performance.
- `docs/providers.md` for provider catalog, adapters, thinking, or credential behavior.
- `docs/tools-security.md` for tools, approvals, redaction, or security policy.
