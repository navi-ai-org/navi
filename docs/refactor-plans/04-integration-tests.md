# Plan: Add Boundary Integration Tests

**Status: DONE** — Completed 2026-05-30.

## Current Problem

The workspace has many useful unit tests, but the most important architectural boundaries are under-tested as boundaries. Recent refactors improved crate and module separation, but regressions could still slip through if TUI, SDK, ACP, provider adapters, host tools, or session lifecycle drift apart.

## Goal

Add integration tests that lock in the intended boundaries and protocol behavior without depending on live providers.

## Target Test Areas

### TUI -> SDK Boundary

- Assert `navi-tui` has no direct dependency on provider implementation crates.
- Exercise model/provider sync through SDK helpers with mocked provider behavior where practical.

### SDK Runtime Lifecycle

- Start session, send turn, add context packet, snapshot session.
- Register a host tool and verify it is exposed/executed through runtime.
- Verify cancellation and approval handles stay usable during active turns.

### ACP Adapter

- New session maps to SDK session id correctly.
- Prompt request forwards assistant deltas.
- Tool approval request maps to ACP permission request and back.
- Cancel notification cancels active turn.

### Provider Payload Goldens

- OpenAI Responses payload with tool calls.
- Chat Completions payload with assistant tool call and tool result.
- Anthropic thinking/text streaming parse.
- Gemini text streaming parse.
- OpenRouter/GitHub Copilot required headers.

### Session Persistence

- Redaction behavior for saved events.
- Replay/snapshot compatibility after new event fields.

## Execution Steps

1. Add a small test utility module for fake model providers and fake tools.
2. Add SDK lifecycle integration tests first because they are lowest risk.
3. Add provider golden tests using existing `wiremock` where HTTP is involved.
4. Add ACP tests around pure mapping helpers before async full-loop tests.
5. Add dependency-boundary checks using `cargo tree` only if stable enough, otherwise use a small metadata parser script/test.

## Risks

- Brittle tests around exact streaming order.
- Slow test suite if every test uses process-level integration.
- Over-mocking internals instead of testing boundaries.

## Guardrails

- No live API calls.
- Prefer fake providers/tools over sleeps/timeouts.
- Keep golden payloads minimal and intentional.
- Keep tests deterministic and local.

## Acceptance Criteria

- New tests fail if TUI directly depends on provider implementation crates again.
- SDK host tool flow is covered.
- ACP cancel/approval mapping has direct tests.
- Provider request/stream protocol has golden coverage.
- Full `cargo test` stays fast and deterministic.
