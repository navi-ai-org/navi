# Plan: Reframe Provider Crate Boundary

**Status: DONE** — Completed 2026-05-30.

## Current Problem

The package is still named `navi-openai`, but it now contains multiple provider families: OpenAI Responses, Chat Completions, Anthropic Messages, Gemini Generate Content, OpenRouter headers, GitHub Copilot OAuth, and opencode routing. Internal modules are better organized, but the crate name and public type `OpenAiProvider` understate the actual responsibility.

## Goal

Create a clearer provider boundary while avoiding unnecessary downstream breakage.

## Candidate End States

### Option A: Rename Only

Rename the crate package to `navi-providers` and keep compatibility aliases temporarily.

Pros: simplest conceptual fix.
Cons: touches workspace dependencies and path imports.

### Option B: New Facade Crate

Add `navi-providers` as a facade and keep `navi-openai` as an implementation crate for now.

Pros: safer migration path.
Cons: one extra crate.

### Option C: Split Providers By Family

Create separate crates such as `navi-provider-openai`, `navi-provider-anthropic`, `navi-provider-gemini`.

Pros: cleanest long-term boundary.
Cons: more boilerplate and coordination.

## Recommended Plan

Use Option B first.

- Add `crates/navi-providers` as the public provider facade.
- Re-export current provider types from the facade.
- Move `navi-sdk` to depend on `navi-providers` instead of `navi-openai`.
- Leave `navi-openai` in place as an implementation crate until provider APIs settle.

## Execution Steps

1. Add `crates/navi-providers` to the workspace.
2. Give it dependency on `navi-openai` and re-export the current public API.
3. Update `navi-sdk` to import provider symbols from `navi-providers`.
4. Update docs to describe `navi-providers` as the intended integration boundary.
5. Keep tests unchanged initially.
6. Later, move implementation modules from `navi-openai` into `navi-providers` or split per family.

## Risks

- Workspace churn if the facade is introduced before the implementation boundary is stable.
- Duplicate naming while both crates exist.
- Confusion around `OpenAiProvider` still being the concrete multi-provider adapter.

## Guardrails

- Do not remove `navi-openai` in the first migration.
- Do not rename public types in the same PR.
- Keep `navi-sdk` public API unchanged.

## Acceptance Criteria

- `navi-sdk` no longer depends directly on `navi-openai`.
- New `navi-providers` crate exists as the provider-facing boundary.
- Workspace tests pass.
- Docs mention `navi-providers` as the provider facade.

## Status

Completed. `navi-providers` facade crate added, `navi-sdk` migrated, all 257 workspace tests pass.
