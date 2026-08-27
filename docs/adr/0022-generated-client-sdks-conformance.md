# ADR 0022 — Generated Client SDKs and a Conformance Corpus

## Status
Proposed

## Context

ADR 0018 §8 asserts that client SDKs are generated from a schema and validated by a
conformance corpus. This ADR decides that mechanism, because the alternative is the
defect NAVI already has.

### The existing defect

One logical engine API is hand-marshalled four times:

| Surface | Lines |
|---|---|
| `navi-napi/src/lib.rs` | 2 654 (181 `#[napi]` methods) |
| `navi-dart/src/{engine,surface}.rs` | 3 069 |
| `navi-server/src/routes/` | ~2 000 |
| `navi-sdk/src/engine_api.rs` | 391 |

`engine_api.rs` is the clearest symptom: two hand-maintained arrays of ~123 method
name strings and a test that diffs them. It does not verify that a binding *works* —
only that a human remembered to add a string. Behaviour can diverge freely while the
test passes.

Writing SDKs by hand in TypeScript and Ruby would reproduce this at higher
multiplicity — N languages × M methods, with no compiler to catch drift and no
equivalent of even the string-array check.

## Decision

### 1. Layering by who maintains it

```
ergonomic facade      hand-written, ~200 lines/language, rarely changes
transport + types     GENERATED from schema, changes every release, free
schema                single source of truth
engine (Rust)
```

The generated layer absorbs release churn. The hand-written layer is where
`navi.systemPrompt(…)`-style sugar lives, and it is small enough to review.

This is the difference between *replicating* an SDK and *generating* one: identical
ergonomics for the caller, incomparable maintenance cost.

### 2. Schema ownership

- **Base ACP schema** comes from upstream. Not NAVI's to define or maintain.
- **Extension schema** — the `navi.dev` `_meta` payloads and `_navi.dev/*` methods
  from ADR 0018 — is defined as Rust types in a dedicated crate, exported to JSON
  Schema via `schemars`, and **committed**.
- CI regenerates and fails on drift. The committed schema is therefore always the
  truth, reviewable in diffs, and usable by generators that do not run Rust.

### 3. What is generated

Per target language: types, message framing, request/response correlation, and
dispatch. Nothing semantic.

Generators are chosen per language rather than mandating one universal tool;
JSON Schema is the interchange format, so a target can use whatever generator is
least bad for it. Generated output is committed, not built on the fly, so that
consumers of the SDK do not need the generator toolchain.

### 4. Conformance corpus

A language-agnostic corpus under `tests/acp-conformance/`: JSON transcripts of
message exchanges with expected outcomes, covering base ACP and the extension
profile. Each includes the adversarial cases that matter:

- unknown `sessionUpdate` variants (must be ignored **and preserved** — ADR 0018 §7)
- unknown `_navi.dev/*` request (must return `-32601`)
- unknown notification (must be ignored, not error)
- extension declared but not advertised (must not be called)
- `initialize` version rejection (ADR 0021 §2)
- interleaved streaming and engine→client requests on one connection
- cancellation mid-tool-call

Every SDK ships a runner over this corpus; CI gates on it. This is what makes
adding a fourth language safe by construction rather than by review.

### 5. Retire the parity arrays

Once the boundary is the schema, `NAVI_ENGINE_API_METHODS` and
`NAVI_NAPI_BOUND_METHODS` are deleted. The schema is the list, and the corpus checks
behaviour rather than names.

### 6. `navi-napi` and `navi-dart` shrink, they do not disappear

An in-process transport is still wanted where a process boundary is not — the TUI,
and possibly Tutor. Those crates become **transports** (connect, send, receive,
register a tool callback), not 123-method mirrors. The 181 `#[napi]` methods and the
Dart C ABI surface collapse; the crates survive at a fraction of their size.

`navi-server` keeps its HTTP surface for the human-facing web UI it already serves,
but the app boundary goes over the ACP WebSocket transport, not per-method routes.

### 7. Language priority

TypeScript first (both target applications need it), Ruby second (ERB/Rails),
others on demand. **No SDK is built for a hypothetical consumer** — an unused
generated SDK is untested surface that still has to pass CI.

### 8. When this is not worth it

Codegen is justified because the client-language count is ≥ 2. With one client
language, hand-writing the facade against the base ACP library would be cheaper and
this ADR should not be applied.

## Consequences

Positive:
- Adding a client language becomes generation plus ~200 lines, not a port
- Release churn lands in generated code at zero human cost
- The corpus catches behavioural divergence, which the string arrays never could
- `engine_api.rs`'s two arrays disappear
- `navi-napi` and `navi-dart` shrink from mirrors to transports
- Committed schema and generated output keep the generator toolchain off consumers
- Base schema maintenance is upstream's, not NAVI's

Negative:
- New build step and a new class of failure: generator bugs, and schema/generator
  version skew
- Generated code is verbose and unpleasant to debug; stack traces cross a layer
  nobody wrote
- **The facade is where drift can still hide.** It is hand-written per language, so
  the same sugar can behave differently in TS and Ruby. The corpus tests the wire,
  not the facade — facade tests remain per-language and manual
- Committing generated output creates large, noisy diffs and merge conflicts
- The corpus is only as good as its cases; a missing adversarial case is invisible
- Schema-first is a discipline change: engine changes now start in the schema, and
  anyone who edits Rust types first will fight CI
- Retiring the parity arrays removes a check before its replacement is proven, so
  ordering matters — the corpus must gate CI before the arrays are deleted

## Alternatives rejected

| Alternative | Why rejected |
|---|---|
| Hand-written SDK per language | Reproduces the existing 8 000-line defect at higher multiplicity |
| Keep the parity string arrays | Verify names, not behaviour; the test passes while bindings diverge |
| One universal generator for all languages | Universal generators are worst-in-class per language; JSON Schema as interchange lets each target pick |
| Generate at build time, not committed | Forces the generator toolchain onto every consumer and makes the schema invisible in review |
| `ts-rs`/`specta` (Rust → TS) only | Solves TypeScript, does nothing for Ruby or anything else |
| gRPC/protobuf for its codegen | Rejected in ADR 0018 on operability grounds; codegen quality does not outweigh that |
| Per-SDK integration tests instead of a shared corpus | N test suites drift; a shared corpus is the only thing that makes N implementations agree |

## References
- ADR 0018 — §8 (schema, codegen, conformance) and §7 (forward-compatibility rules the corpus tests)
- ADR 0021 §2 — version rejection case in the corpus
- `crates/navi-sdk/src/engine_api.rs` — the parity arrays retired by §5
- `crates/navi-napi/src/lib.rs`, `crates/navi-dart/src/` — the surfaces reduced by §6
- [ACP v2 — Extensibility](https://agentclientprotocol.com/protocol/v2/extensibility)
- `AGENTS.md` non-negotiable #2 — the surface-sync cost this ADR removes
