# Plan: Modularize ACP Adapter

**Status: DONE** — Completed 2026-05-30.

## Current Problem

`crates/navi-cli/src/acp.rs` is an adapter over `navi-sdk::NaviEngine`, but it still mixes protocol handlers, session registry management, prompt task lifecycle, event forwarding, cancellation, approval requests, and schema conversion in one file.

## Goal

Keep ACP behavior unchanged while separating protocol concerns from runtime/session lifecycle concerns.

## Proposed Module Shape

- `crates/navi-cli/src/acp.rs`: server setup and module hub.
- `crates/navi-cli/src/acp/state.rs`: `AcpState`, `AcpSession`, `ActivePrompt`, `AcpSessionRegistry`.
- `crates/navi-cli/src/acp/handlers.rs`: initialize, new session, prompt, cancel handlers.
- `crates/navi-cli/src/acp/prompt_runner.rs`: `run_prompt_task`, cancellation wiring, turn lifecycle.
- `crates/navi-cli/src/acp/events.rs`: runtime event to ACP update forwarding.
- `crates/navi-cli/src/acp/permissions.rs`: approval request/response mapping.
- `crates/navi-cli/src/acp/schema.rs`: content block, tool kind, and tool result conversion helpers.
- `crates/navi-cli/src/acp/tests.rs`: existing tests moved if useful.

## Execution Steps

1. Extract pure mapping helpers first because they are low risk.
2. Extract event forwarding and permission request helpers next.
3. Introduce `AcpSessionRegistry` wrapping the session map and lock access.
4. Move prompt task execution into `prompt_runner.rs`.
5. Keep public entrypoint `run_acp_server` stable.
6. Run `cargo test -p navi-cli` after each extraction.

## Risks

- Breaking responder ownership/lifetime behavior in async prompt handling.
- Changing cancellation timing.
- Accidentally writing ACP diagnostics to stdout.

## Guardrails

- Do not change ACP protocol payloads.
- Do not replace the transport or introduce daemon/WebSocket behavior.
- Keep `navi-sdk::NaviEngine` as the only runtime boundary.
- Avoid holding a mutex guard across `.await`.

## Acceptance Criteria

- `acp.rs` mostly contains module declarations and `run_acp_server`.
- Session registry operations are centralized.
- Prompt lifecycle and event forwarding are in separate modules.
- `cargo test -p navi-cli` and `cargo check` pass with no warnings.
