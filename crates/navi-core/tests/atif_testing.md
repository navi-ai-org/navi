# ATIF Exporter — Testing Strategy

This document records the testing techniques applied to the ATIF v1.7 exporter
(`navi-core/src/atif.rs`) and explains which techniques were **not** applied and
why.

## Applied techniques

### 1. Property-Based Testing (PBT)
**File:** `tests/atif_pbt.rs` — 13 properties, 256 cases each.

Properties verified:
- **Determinism**: same events → byte-identical JSON.
- **step_id sequential**: 1..=n, no gaps/duplicates.
- **observation↔tool_call correlation**: every `source_call_id` matches a
  `tool_call_id` in the same step.
- **final_metrics = Σ step metrics**: token sums are exact.
- **tool_call.arguments is always a JSON object** (ATIF requirement).
- **only agent steps carry agent-only fields** (tool_calls, observation,
  metrics, reasoning_content).
- **JSON roundtrip idempotency**: serialize → deserialize → serialize is stable.
- **schema_version pinned** to `ATIF-v1.7`.
- **redaction is idempotent**: double-redaction = single-redaction.
- **non-secret text is preserved** by redaction.
- **multimodal user content** produces `Parts` arrays.
- **no-panic** on arbitrary event orderings (incl. orphan tool results).
- **arbitrary JSON no-panic**: any JSON that deserializes as `AgentEvent` folds
  without panic.

**Bug found by PBT:** orphan `ToolCompleted` events (no matching
`ToolRequested`) produced observations with a `source_call_id` that didn't
match any `tool_call_id` in the step — violating the ATIF correlation
invariant. **Fixed** by setting `source_call_id: None` for orphan results.

### 2. Contract Testing (JSON Schema)
**File:** `tests/atif_contract.rs` — 7 tests.

A JSON Schema (Draft 2020-12) encodes the ATIF v1.7 consumer contract. Every
trajectory produced by `build_trajectory` is validated against it using the
`jsonschema` crate. Includes negative tests (wrong version, non-object
arguments must be rejected).

This is the Rust-idiomatic equivalent of a **Consumer-Driven Contract** (Pact):
the schema is the consumer's expectation, and the test enforces it on every
export. A Pact-broker is not used because the boundary is a library API
(`build_trajectory`), not an HTTP service.

### 3. Fuzzing (cargo-fuzz)
**Directory:** `fuzz/` — 2 libFuzzer targets.

- `fold_arbitrary_events`: feeds arbitrary bytes as JSON `AgentEvent` arrays
  to `build_trajectory`. Any input that deserializes must not panic.
- `roundtrip_json`: feeds arbitrary bytes as JSON `Trajectory`, confirms
  serialize → deserialize → serialize is idempotent.

**Build:** `cargo check` passes. **Running** requires `cargo +nightly fuzz`
(the `cargo-fuzz` binary and a nightly toolchain, neither of which is installed
in this environment).

### 4. Mutation Testing (cargo-mutants)
**File:** `cargo-mutants.toml` — scoped to `src/atif.rs`.

cargo-mutants mutates the fold logic and confirms the test suite (PBT +
contract + unit tests) catches every viable mutant. Run with:
```bash
cargo mutants -p navi-core --file crates/navi-core/cargo-mutants.toml
```

## Techniques NOT applied (and why)

### Loom (concurrency testing)
**Not applicable.** `build_trajectory` is a pure, single-threaded fold over
`&[AgentEvent] → Trajectory`. There is no shared state, no locks, no async,
no spawning. Loom tests concurrent access to `Arc`/`Mutex`/channels — none of
which exist in this code path.

### Toxiproxy / Fault Injection (network)
**Not applicable.** The ATIF export path has no network calls, no I/O, no
database. It reads from an in-memory `SessionSnapshot` and writes to a
`String`. Toxiproxy injects network failures (latency, dropped connections)
into TCP connections — there are none to inject into.

### Pact-broker (Consumer-Driven Contracts over HTTP)
**Not applicable.** Pact tests HTTP/messaging boundaries between services. The
ATIF exporter is a library function (`build_trajectory`), not a service. The
JSON Schema contract test (`tests/atif_contract.rs`) serves the same purpose
(consumer defines expected shape, producer is validated against it) without
requiring a broker or HTTP transport.

### Schemathesis (API fuzzing)
**Deferred.** Schemathesis fuzzes HTTP APIs against an OpenAPI schema. The
ATIF export is currently CLI/SDK/N-API only — there is no HTTP endpoint to
fuzz. Adding `GET /sessions/:id/export?format=atif` to `navi-server` would
enable Schemathesis; this is a follow-up task (requires a new HTTP surface in
sync with AGENTS.md).
