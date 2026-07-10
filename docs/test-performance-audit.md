# NAVI Test Performance & Memory Audit

**Date:** 2026-07-09  
**Repo:** `/home/enrell/projects/navi`  
**Method:** Static inventory of test targets, `#[test]` / `#[tokio::test]` attributes, sleeps/timeouts/TempDir/CARGO_BIN_EXE/multi_thread patterns, Cargo/dev-deps, justfile + CI wiring. No production code changes. Counts are **static attribute counts** (a few may be behind `cfg`/features); runtime timings were not fully re-measured on a cold CI runner.

**Constraint:** Report only — do not implement fixes in this pass.

---

## Executive summary

NAVI’s suite is large (~1.7k–2.0k test functions across product crates) and **dominated by three cost centers**:

1. **Compile-time: many integration binaries + full dependency graph**  
   - `navi-cli` integration tests force linking **`CARGO_BIN_EXE_navi`** (full product binary: TUI + SDK + providers + plugins + voice stubs).  
   - `navi-tui` has **four** integration harnesses (`screenshots`, `scenarios`, `event_loop`, `terminal_corruption_repro`), each recompiling the crate as an external binary.  
   - Default features pull **candle embeddings** (`navi-core`) and **many tree-sitter grammars** (`navi-vfs`).  
   - CI already learned this: clippy uses `--lib --bins` *not* `--all-targets`; test still runs full workspace tests (minus napi/dart/server).

2. **Runtime: full `TuiApp` / `NaviEngine` construction per test**  
   - `TuiApp::new` always calls `init_registry_store` + **`build_engine` → full `NaviEngineBuilder`** (`crates/navi-tui/src/app.rs:253–276`).  
   - Unit tests call `test_app()` repeatedly (~118 tests in `src/tests.rs` alone).  
   - Screenshot/scenario harnesses do the same via `Harness::new` / `with_engine` (engine still built, then optionally replaced).

3. **Parallelism + multi-thread tokio + wall-clock waits**  
   - CI: `--test-threads=8` (justfile default 8).  
   - Many `#[tokio::test(flavor = "multi_thread")]` suites (TUI scenarios, navi-tui unit async, navi-core runtime, navi-mcp load, navi-sdk snapshot).  
   - Real sleeps: PTY settle 400ms, openai wiremock delays 300ms, MCP hang probes, SleepingTool 30ms, scenario poll loops up to 5s.  
   - **Hang/flake already documented with `#[ignore]`**: headless e2e, async compact palette, fake-wasm plugin loads.

**Peak memory risks (tests):** concurrent multi_thread runtimes × full engines; snapshot string diffs holding expected+actual screens; panic messages that dump full golden text; SQLite open/close per TempDir; wasmtime + WAT compile in plugin-runtime; optional ONNX/voice model load (feature-gated, usually skipped).

**Highest ROI (ranked):**  
1. **S** — Delete or gate dead `tests/integration/*` duplicates (not Cargo-discovered).  
2. **S** — Split PR CI: `just test-fast` (lib+bins product) always; integration/CARGO_BIN_EXE nightly/PR-label.  
3. **M** — Lightweight `TuiApp` / `Harness` path that skips real `NaviEngine` for pure UI tests.  
4. **M** — Convert sleep/poll to notify/channel/tokio::sync primitives in scenario + MCP + tool concurrency tests.  
5. **M** — Cap/share multi_thread runtimes; prefer `current_thread` unless concurrency is under test.  
6. **L** — Feature-split embeddings/tree-sitter for test builds; cargo-nextest + timing profiles.

---

## Inventory (tables)

### Workspace members & CI inclusion

| Crate | Role | In default CI `cargo test`? |
|-------|------|-----------------------------|
| navi-core | Engine, tools, memory, registry | Yes |
| navi-openai | Providers + wiremock HTTP | Yes |
| navi-providers | Facade (no tests) | Yes (empty) |
| navi-sdk | Engine facade | Yes |
| navi-tui | TUI + goldens | Yes |
| navi-cli | Binary + e2e | Yes |
| navi-vfs | Tree-sitter VFS | Yes |
| navi-mcp | MCP client | Yes |
| navi-lite | Minimal engine | Yes |
| navi-voice | ASR (default features off in cli) | Yes (light unit; onnx integration cfg) |
| navi-plugin-* | Manifest/broker/host/runtime/orchestrator | Yes |
| copland | TUI widgets | Yes |
| navi-plugin-api | Traits | Yes (no tests found) |
| navi-server | Warp server | **Excluded** in CI |
| navi-napi | Node binding | **Excluded** |
| navi-dart | FFI | **Excluded** |

Sources: root `Cargo.toml` members; `.github/workflows/ci.yml` test job; `justfile` `product_packages` / `test` / `test-fast`.

### Approximate unit vs integration counts (static `#[test]` / `#[tokio::test]`)

Counts are approximate (±5%) from attribute search. Integration = `crates/*/tests/*.rs` or explicit binary harnesses.

| Crate | Unit (in `src/`) | Integration (`tests/`) | Notes |
|-------|------------------|------------------------|-------|
| **navi-core** | ~750–900 | 22 (`parity_check`) | Largest suite; tools/security/memory dominate |
| **navi-tui** | ~288 | ~42 active | Plus **~28 orphan** under `tests/integration/` (see below) |
| **navi-openai** | ~144 | 0 | Wiremock HTTP tests live in `src/tests.rs` |
| **navi-plugin-broker** | ~132 | 0 | Includes `redteam_tests.rs` via `#[cfg(test)]` |
| **navi-plugin-manifest** | ~88 | 0 | |
| **navi-sdk** | ~74 | 0 | Heavy `engine/tests.rs` |
| **navi-vfs** | ~63 | 0 | Grammar/minify tests |
| **navi-core tool subsystem alone** | ~240+ | — | `tool/tests.rs` (~68) + builtins (~170+) |
| **navi-cli** | ~6 (`bench_cmd`) | 6 (1 ignored) | Full binary link |
| **copland** | ~24 | 0 | |
| **navi-plugin-runtime** | ~18 | 0 | wasmtime + wat |
| **navi-plugin-orchestrator** | ~16 | 0 | 1 ignored wasm |
| **navi-mcp** | ~15 | 0 | multi_thread + process spawn |
| **navi-plugin-host** | ~9 | 0 | |
| **navi-voice** | ~8 | 2 (cfg `onnx`) | Model path soft-skip |
| **navi-lite** | ~3 | 0 | |
| **navi-dart** | 0 | ~31 | CI excluded |
| **navi-napi** | ~14 | JS tests separate | CI excluded |
| **navi-providers / plugin-api / server** | 0 | 0 | |

**Rough CI product total:** on the order of **~1,700–2,000** runnable tests (excluding ignored and feature-gated voice e2e).

### Integration test binaries under `crates/*/tests/`

| Binary (Cargo auto-target) | Path | Links full `navi` bin? |
|----------------------------|------|------------------------|
| `parity_check` | `crates/navi-core/tests/parity_check.rs` | No |
| `screenshots` | `crates/navi-tui/tests/screenshots.rs` | No (links navi-tui lib + harness) |
| `scenarios` | `crates/navi-tui/tests/scenarios.rs` | No |
| `event_loop` | `crates/navi-tui/tests/event_loop.rs` | No |
| `terminal_corruption_repro` | `crates/navi-tui/tests/terminal_corruption_repro.rs` | No |
| `headless_e2e` | `crates/navi-cli/tests/headless_e2e.rs` | **Yes** `CARGO_BIN_EXE_navi` |
| `pty_smoke` | `crates/navi-cli/tests/pty_smoke.rs` | **Yes** |
| `plugin_install_update` | `crates/navi-cli/tests/plugin_install_update.rs` | **Yes** |
| `ffi_api` | `crates/navi-dart/tests/ffi_api.rs` | No (dart crate) |
| `transcribe_libri` | `crates/navi-voice/tests/transcribe_libri.rs` | No; `cfg(feature = "onnx")` |

Cargo discovers **only** `tests/*.rs` and `tests/*/main.rs`. Subdirectory sources without `main.rs` are **not** targets.

### Duplicate / orphan TUI tests

| Live target | Near-duplicate (not compiled) |
|-------------|-------------------------------|
| `tests/screenshots.rs` | `tests/integration/screenshots.rs` |
| `tests/scenarios.rs` | `tests/integration/scenarios.rs` |
| `tests/event_loop.rs` | `tests/integration/event_loop.rs` |

The `tests/integration/` tree is a **stale copy** (same comments, same test names, same multi_thread scenarios). It does not run under Cargo but still confuses reviewers and can drift. **Action:** delete or convert to a single intentional target (Effort **S**).

### Already `#[ignore]` (hang / fixture / flake)

| Location | Reason (from attribute) |
|----------|-------------------------|
| `navi-cli/tests/headless_e2e.rs:9` | `hangs on CI mock multi-turn (headless e2e)` |
| `navi-tui/src/tests.rs:666` | `flaky async compact palette on CI` |
| `navi-plugin-orchestrator/.../orchestrator.rs:523` | `needs real wasm fixture, not placeholder bytes` |
| `navi-sdk/src/tooling.rs:378` | `needs real wasm fixture` |

### How tests are invoked

| Command | What it runs |
|---------|----------------|
| `just test` | `cargo test --workspace --exclude navi-napi --exclude navi-dart --exclude navi-server -- --test-threads=8` |
| `just test-fast` | lib+bins only for `product_packages` (no integration binaries) |
| `just verify` | fmt-check + check + **test-fast** |
| CI `Test` job | Same as `just test` (full product workspace tests, 25m timeout) |
| CI `Clippy` | lib+bins only; **explicitly avoids `--all-targets`** (comment: #1 wall-time cost) |
| `just snapshot-update` | `UPDATE_SNAPSHOTS=1 cargo test -p navi-tui --test screenshots` |

---

## Compile-time bottlenecks

### 1. Full `navi` binary for three CLI integration crates (High)

Files:

- `crates/navi-cli/tests/pty_smoke.rs:42` — `env!("CARGO_BIN_EXE_navi")`
- `crates/navi-cli/tests/headless_e2e.rs:101` — same
- `crates/navi-cli/tests/plugin_install_update.rs:43` — same

`navi-cli` depends on `navi-tui`, `navi-sdk`, `navi-core`, plugins, voice, etc. Each `[[test]]` harness waits on a complete binary rebuild when CLI or any deep dep changes. That is correct for smoke/e2e, but expensive on every PR when mixed with unit suites.

**Effort S–M:** gate under `#[cfg]` or cargo feature / separate CI job so PR default is lib+bins.

### 2. Multiple navi-tui integration binaries (High)

Four separate crates-as-tests all depend on `navi_tui::testing` (pub harness) + ratatui TestBackend:

- screenshots (~23 tests, 23 golden files)
- scenarios (5 multi_thread async)
- event_loop (4)
- terminal_corruption_repro (10)

Each forces an integration-test compile of the full TUI graph (image/ratatui-image, copland, sdk, core). CI clippy already refuses `--all-targets` for this reason (`.github/workflows/ci.yml:81–83`).

**Effort M:** merge into one `tests/tui_integration/main.rs` binary, or keep screenshots PR-always and move scenarios/corruption to nightly.

### 3. Heavy default features

| Feature / dep | Where | Test impact |
|---------------|-------|-------------|
| `embeddings` (candle-core/nn/transformers, tokenizers, hf-hub) | `navi-core` default | Compiles even if most tests never load models |
| tree-sitter × ~15 languages | `navi-vfs` build.rs | Long C compile; always in product path |
| `wasmtime` + cranelift | `navi-plugin-runtime` | Large dep; WAT tests compile modules |
| `wiremock` | navi-openai, navi-cli dev-dep | Fine for unit; pulls hyper stack |
| `portable-pty` | navi-cli dev-dep | PTY smoke only |
| `image` / `ratatui-image` | navi-tui | Always for any tui test compile |
| `ort` download-binaries | navi-voice feature `onnx` | CLI defaults voice **off**; still a footgun if enabled in CI |

### 4. Workspace / `--all-targets` rebuild behavior

- `cargo test --workspace` builds every member’s lib tests **and** integration binaries.  
- Changing a leaf in `navi-core` invalidates navi-sdk, navi-tui, navi-cli, openai, plugins…  
- `just test-fast` avoids integration binaries — **preferred local loop** (already documented).  
- Clippy CI correctly avoids `--all-targets`; **test CI does not** split unit vs integration.

### 5. Giant unit modules that recompile as one unit

Notable large test modules (recompile cost on any edit in same crate):

| File | ~tests | Character |
|------|--------|-----------|
| `navi-tui/src/tests.rs` | ~118 | Monolithic TUI unit suite |
| `navi-openai/src/tests.rs` | ~100+ | Mapping + wiremock |
| `navi-core/src/tool/tests.rs` | ~68 | Tool executor async |
| `navi-core/src/security.rs` tests | ~69 | TempDir-heavy |
| `navi-core/src/tool/builtin/*` | ~170+ | Per-tool suites |
| `navi-plugin-broker/src/redteam_tests.rs` | ~36 | Security redteam |
| `navi-sdk/src/engine/tests.rs` | ~70 | Full engine builder |

---

## Runtime bottlenecks (with file:line)

### Wall-clock sleeps / long timeouts

| Location | Behavior | Impact |
|----------|----------|--------|
| `navi-cli/tests/pty_smoke.rs:87` | `sleep(400ms)` before Ctrl-C | **Fixed 0.4s** every PTY run (already reduced from 2s) |
| `navi-cli/tests/headless_e2e.rs:119–120` | `timeout(180s)` on process | Hang risk; currently **ignored** |
| `navi-tui/tests/scenarios.rs:16–24` | Poll loop up to **5s**, 2ms sleep | Worst-case wall time × 5 scenarios |
| `navi-tui/tests/scenarios.rs:30–37` | `flush_events`: up to 20× **5ms** sleeps | Up to ~100ms per flush even when idle |
| `navi-tui/tests/integration/scenarios.rs` | Same as above (orphan) | Dead code |
| `navi-openai/src/tests.rs:1427, 1469` | wiremock / stream delay **300ms** | Multiplied by timeout-oriented tests |
| `navi-core/src/turn/tests.rs:108` | `SleepingTool` sleeps **30ms** | Concurrency tests intentionally wait |
| `navi-core/src/runtime/tests.rs:494` | `sleep(50ms)` | Ordering / cancel tests |
| `navi-core/src/session.rs:598` | `thread::sleep(50ms)` in session code under test path | Retries |
| `navi-mcp/src/lib.rs:687–719` | Spawns `sleep 10`, expects finish **&lt;5s** with 200ms server timeout | Process spawn + multi_thread runtime |
| `navi-plugin-runtime/src/runtime.rs:534–570` | WASM busy loop + **1ms** timeout / fuel | CPU-bound, can be flaky under load |
| `navi-tui/src/tests.rs:1791` | poll with 1ms sleep × 50 | Minor |
| `navi-tui/src/mouse.rs:1283` | sleep 1ms in async test | Minor |

### Full engine / session / turn loops

| Location | Pattern |
|----------|---------|
| `navi-core/src/runtime/tests.rs:117–283` | Real `AgentRuntime::new` + `submit_task` / `start_session` + `send_turn` + event timeouts (1s) |
| `navi-sdk/src/engine/tests.rs:10–31, 795–864` | `NaviEngineBuilder` + `start_session` / `snapshot_session` per test; multi_thread on snapshot |
| `navi-tui/tests/scenarios.rs:46–84` | Real submit path: `start_session` + `subscribe_events` + `send_turn` via MockEngine |
| `navi-core/src/turn/tests.rs` | Multi-turn tool loops with mock providers |
| `navi-core/src/memory/tests.rs:645+` | Full continuity rebuild with mock LLM checkpoint markdown |

### TempDir thrash

Widespread `tempfile::tempdir` / `TempDir::new` in:

- `navi-sdk/src/engine/tests.rs` (`test_engine` helper every test group)
- `navi-core` security, session, credentials, memory, plan_store, eval, tools
- `navi-cli` e2e (3–4 TempDirs per test)
- `navi-plugin-broker` fs/git/redteam
- `navi-dart/tests/ffi_api.rs` (nearly every test)

Cost: mkdir, SQLite file create/delete, fsync pressure under 8-way parallel tests.

### SQLite open/close

- Memory/history tests open fresh SQLite files under TempDir (`navi-core/src/memory/tests.rs:14–18`, MemoryManager constructors).  
- `TuiApp::new` → `init_registry_store` → `RegistryStore::open` (`navi-tui/src/runtime.rs:72–77`) **per app construction**.  
- Credential store on `data_dir` per `TuiApp` / engine.

No shared in-memory registry fixture for unit tests.

### Network: mostly mocked (good)

- Provider HTTP: **wiremock** in `navi-openai/src/tests.rs` and headless_e2e.  
- Registry updates disabled in SDK test config (`engine/tests.rs:86–87`).  
- CLI tests set `NAVI_NO_REGISTRY_UPDATE=1`.  
- Soft risk: any test that leaves `registry.update_enabled` default **true** without mocking could hit network; production audit already flags registry fetch timeouts.

### Parallelism / global state

| Issue | Evidence |
|-------|----------|
| CI `--test-threads=8` | justfile + ci.yml |
| multi_thread tokio | navi-tui scenarios; `src/tests.rs` async; runtime tests; mcp; sdk snapshot |
| Thread-local registry store | `set_registry_store` / `init_registry_store` — concurrent tests may race if they open different data_dirs but share TLS |
| Shared `/tmp/navi-test` paths | `test_app` uses `PathBuf::from("/tmp/navi-test")` (`navi-tui/src/tests.rs:59`) — **not isolated TempDir** |
| No `serial_test` crate | Global env / credential paths rely on luck |

### Serial-looking work that could be cheaper (not more parallel)

Many pure pure-function tests (markdown, minify, classifier, oauth URL builders) already parallelize fine. Bottleneck is **heavy constructors**, not lack of threads. Increasing `test-threads` beyond 8 risks **memory peaks** more than wall-time wins on 2-core CI runners.

---

## Peak memory risks (with file:line)

### 1. Full `NaviEngine` / `TuiApp` per test (High)

```253:276:crates/navi-tui/src/app.rs
    pub fn new(...) -> Result<Self> {
        init_registry_store(&loaded_config);
        let models = available_model_options(&loaded_config.config);
        // ...
        let engine: Arc<dyn EngineDriver> =
            Arc::new(build_engine(&loaded_config, project_dir.clone())?);
```

Even `Harness::with_engine` first builds a real app then replaces the engine (`testing/mod.rs:102–116`). Under 8 parallel tests, expect **8× engine + tool registry + session store + model catalog** resident.

### 2. Snapshot / golden string holding (Medium)

```204:238:crates/navi-tui/src/testing/mod.rs
    pub fn assert_screen(&self, name: &str) {
        let actual = self.buffer_text();
        // ...
        let expected = std::fs::read_to_string(&path)...;
        if actual != expected.trim_end_matches('\n') {
            panic!(... full expected + actual ...);
        }
    }
```

- `buffer_text` walks every cell (up to 120×40 = 4800 cells).  
- On failure, panic message holds **two full screens** as `String`.  
- 23 goldens under `tests/snapshots/` (welcome 120×40 is the largest).  
- Not catastrophic, but multiplies under parallel screenshot tests + panic dumps.

### 3. Model catalog / embedded registry in every TUI test (Medium)

`available_model_options` + provider catalog load embedded/registry snapshot into each app. Large JSON snapshot tree: `crates/navi-core/registry-snapshot/providers/*.json` (27 providers). Tests often then clear models for stable goldens (`clear_models` in harness) — **pay cost then discard**.

### 4. Embeddings / candle (Medium compile, Low runtime if paths empty)

`navi-core` default `embeddings` compiles candle; runtime load is `OnceLock` cached (`memory/embedding.rs:27–60`). Memory tests set empty embedding paths (`memory/tests.rs:662–663`) → no model load. Risk if a test points at real model paths or CI caches HF models.

### 5. Voice ONNX (High if feature on)

`transcribe_libri.rs` loads Nemotron engine when model dir exists (`navi-voice/tests/transcribe_libri.rs:45–56`). Soft-skips otherwise. **Do not enable `onnx` on CI** without RAM budget.

### 6. Concurrent multi_thread runtimes (Medium–High)

Each `#[tokio::test(flavor = "multi_thread")]` can spin a multi-thread scheduler. Combined with test-threads=8:

- TUI scenarios (`worker_threads = 2`)  
- navi-tui unit async tests (~9)  
- navi-mcp hang/allowlist tests  
- navi-core runtime lifecycle  

→ thread stack + runtime overhead multiplies.

### 7. WASM runtime tests (Medium)

`navi-plugin-runtime` instantiates wasmtime, compiles WAT, runs fuel/timeout loops (`runtime.rs:495–570`). Transient peak during Cranelift compile.

### 8. SDK clone-heavy helpers (Medium)

`test_engine()` builds full config + engine; many tests call it independently without reuse (`engine/tests.rs:10–31`). Same for `test_engine_with_key`.

### 9. Session event logs / ModelDelta in tests (Low–Medium)

Tests generally inject **small** numbers of deltas (e.g. scenarios 2 deltas; screenshot injects few). Production ModelDelta accumulation is a **product** memory issue (`docs/performance-audit.md`); test suite does not currently synthesize thousands of deltas. Worth avoiding if anyone writes “stress” stream tests later.

---

## Flaky / hang catalog

| ID | Test / area | Symptom | Status | Root cause hypothesis |
|----|-------------|---------|--------|------------------------|
| H1 | `headless_cli_runs_engine_provider_and_read_tool` | Hang / never completes multi-turn mock | **`#[ignore]`** | Process e2e + wiremock multi-turn under load; 180s timeout still insufficient historically |
| H2 | `command_palette_compact_submits_immediate_summary_request` | Flaky on CI | **`#[ignore]`** | Async turn task race; multi_thread |
| H3 | `load_succeeds_with_approved_lockfile_entry` (orchestrator) | Would fail without real wasm | **`#[ignore]`** | Placeholder bytes rejected |
| H4 | `build_local_tooling_loads_installed_wasm_plugin_store` (sdk) | Same | **`#[ignore]`** | Fake wasm |
| H5 | `pty_smoke_renders_welcome_then_quits_cleanly` | Intermittent empty PTY / slow start | Active | 400ms settle may be tight on cold binary |
| H6 | TUI scenarios (`wait_for_calls` 5s) | Timeout panic under starvation | Active | Polling + multi_thread + MockEngine scheduling |
| H7 | MCP `load_times_out_when_server_hangs` | False fail if spawn slow | Active | Expects &lt;5s; spawns real `sleep` process |
| H8 | plugin-runtime `execute_timeout` | Timeout vs fuel race | Active | Busy loop under scheduler pressure |
| H9 | Shared `/tmp/navi-test` data_dir | Cross-test credential/session interference | Latent | Not TempDir-isolated in unit helper |
| H10 | Registry TLS + parallel RegistryStore::open | Catalog inconsistency | Latent | Thread-local store + parallel apps |
| H11 | Voice e2e | RAM/hang if model present | Soft-skip | Full ONNX load |
| H12 | Dead `tests/integration/*` | Human confusion only | Orphan | Not executed |

---

## Recommended fixes ranked by ROI

| Rank | Fix | Effort | Expected win | Notes |
|------|-----|--------|--------------|-------|
| 1 | **Delete or relocate orphan `navi-tui/tests/integration/`** | **S** | Clarity; avoid accidental double-maintenance | Zero runtime win until someone wires it by mistake |
| 2 | **CI matrix: always `test-fast`; integration/bin e2e separate** | **S** | Large PR wall-time (skip CARGO_BIN_EXE + tui integration rebuilds on default path) | Mirror clippy’s “no all-targets” philosophy |
| 3 | **`TuiApp::new_for_test` / harness without real engine** | **M** | Runtime+memory: drop NaviEngine × hundreds of unit/screenshot tests | Inject `MockEngine` *before* build; skip registry SQLite when unused |
| 4 | **Isolate unit `data_dir` with TempDir** | **S** | Correctness + less /tmp collision | Replace `/tmp/navi-test` in `test_app` |
| 5 | **Replace scenario sleep polls with oneshot/notify** | **M** | Cut scenario flakiness + up to 5s waits | MockEngine `complete_turn` already signals; bridge drain should wake on channel |
| 6 | **Prefer `current_thread` tokio unless concurrency is the subject** | **S** | Fewer threads / less RAM under test-threads=8 | Keep multi_thread only for runtime_session_lifecycle, MCP spawn, etc. |
| 7 | **Gate CLI integration tests behind feature or `--ignored` job** | **S** | Compile time for PR | Keep one `pty_smoke` on main CI if startup regressions matter |
| 8 | **Merge tui integration binaries into one target** | **M** | Compile time | Single `tests/tui/main.rs` modules |
| 9 | **`navi-core` test profile: `default-features = false` for embeddings in test-only builds** | **M–L** | Compile time (candle) | Careful: product default stays embeddings |
| 10 | **Shared `OnceLock` test fixtures for registry catalog / default config** | **M** | Runtime allocs | Avoid re-parse provider JSON |
| 11 | **Snapshot asserts: hash or size-limited diffs** | **S** | Failure-path memory/log spam | Don’t dump dual 120×40 on every mismatch in CI logs |
| 12 | **Real wasm fixture pack for ignored plugin tests** | **M** | Coverage without hang | Or keep ignored on nightly only |
| 13 | **Re-enable headless_e2e with deterministic mock protocol** | **L** | Product confidence | Fix hang before PR gate |
| 14 | **cargo-nextest + slow-test quarantine** | **S** | Better timing data; retries only flaky set | justfile already installs nextest in setup-tools |
| 15 | **Cap CARGO_TEST_THREADS=2 on low-RAM; document RAM budget** | **S** | OOM avoidance | justfile already comments this |

---

## Proposed CI test matrix

### Always (PR + main) — target ≤10–15 min warm cache

| Job | Command | Rationale |
|-----|---------|-----------|
| fmt | `cargo fmt --check` | Unchanged |
| clippy | lib+bins product excludes (current) | Unchanged |
| **unit** | `just test-fast` or equivalent `cargo test --lib --bins {{product_packages}} -- --test-threads=8` | Fast signal; no integration binaries |
| **tui goldens** | `cargo test -p navi-tui --test screenshots -- --test-threads=4` | Visual regressions; no process spawn |
| optional smoke | `cargo test -p navi-cli --test pty_smoke` | One binary link; Linux only |

### Nightly / label `full-tests` / main post-merge

| Job | Command | Rationale |
|-----|---------|-----------|
| full product | current `cargo test --workspace --exclude napi/dart/server` | Catch integration drift |
| cli e2e | `plugin_install_update` + **ignored** `headless_e2e` once fixed | Full binary paths |
| tui scenarios + event_loop + corruption | remaining navi-tui `--tests` | Async races |
| bindings | navi-dart / navi-napi | Heavy toolchains |
| coverage | `just coverage` | llvm-cov slow |
| voice onnx | only if models cached + high RAM runner | Optional |

### Explicitly not on PR by default

- `navi-voice` onnx transcription with real model  
- WASM plugin load tests without fixtures (stay ignored or nightly with fixtures)  
- `cargo test --all-targets` / clippy `--all-targets`  
- Workspace members navi-server unless server job added  

### Local developer loop (document in CONTRIBUTING)

```text
just test-fast          # default
cargo test -p navi-core --lib
cargo test -p navi-tui --lib
just test               # before PR if touching CLI/TUI integration
```

---

## Appendix

### A. Integration binary inventory (absolute paths)

```
/home/enrell/projects/navi/crates/navi-core/tests/parity_check.rs
/home/enrell/projects/navi/crates/navi-tui/tests/screenshots.rs
/home/enrell/projects/navi/crates/navi-tui/tests/scenarios.rs
/home/enrell/projects/navi/crates/navi-tui/tests/event_loop.rs
/home/enrell/projects/navi/crates/navi-tui/tests/terminal_corruption_repro.rs
/home/enrell/projects/navi/crates/navi-cli/tests/headless_e2e.rs
/home/enrell/projects/navi/crates/navi-cli/tests/pty_smoke.rs
/home/enrell/projects/navi/crates/navi-cli/tests/plugin_install_update.rs
/home/enrell/projects/navi/crates/navi-dart/tests/ffi_api.rs
/home/enrell/projects/navi/crates/navi-voice/tests/transcribe_libri.rs
```

Orphan (not Cargo targets):

```
/home/enrell/projects/navi/crates/navi-tui/tests/integration/screenshots.rs
/home/enrell/projects/navi/crates/navi-tui/tests/integration/scenarios.rs
/home/enrell/projects/navi/crates/navi-tui/tests/integration/event_loop.rs
```

### B. Dev-dependencies of note

| Crate | Dev-deps |
|-------|----------|
| navi-cli | wiremock, portable-pty |
| navi-openai | wiremock |
| navi-tui | tempfile |
| navi-core | tempfile |
| navi-sdk | tempfile, toml |
| navi-plugin-runtime | wat |
| navi-voice | tempfile, tokio macros |

### C. Measurement notes (for follow-up)

Suggested commands (not fully executed in this audit pass):

```bash
# Counts
cargo test -p navi-core --lib -- --list 2>/dev/null | tail -1
cargo test -p navi-tui --lib -- --list 2>/dev/null | tail -1
cargo test -p navi-tui --tests -- --list 2>/dev/null | tail -1

# Timing (warm)
time cargo test -p navi-core --lib -- --test-threads=8
time cargo test -p navi-tui --lib -- --test-threads=8
time cargo test -p navi-tui --test screenshots -- --test-threads=4
time cargo test -p navi-cli --tests -- --test-threads=1

# Peak RSS (Linux)
/usr/bin/time -v cargo test -p navi-tui --lib -- --test-threads=8
```

Install `cargo-nextest` (`just setup-tools`) and run `cargo nextest run --profile ci` with a `slow-timeout` profile for quarantine.

### D. Cross-reference: product performance audit

Runtime product issues that **mirror** test cost (not re-litigated here):

- ModelDelta persistence in session event log  
- Full chat re-render on stream  
- MemoryManager / SQLite open patterns  

See: `/home/enrell/projects/navi/docs/performance-audit.md`.

### E. Key file index for implementers

| Concern | Primary paths |
|---------|----------------|
| CI policy | `.github/workflows/ci.yml`, `justfile` |
| Full binary e2e | `crates/navi-cli/tests/*.rs` |
| TUI harness | `crates/navi-tui/src/testing/mod.rs`, `src/app.rs` |
| Monolithic TUI unit | `crates/navi-tui/src/tests.rs` |
| Goldens | `crates/navi-tui/tests/screenshots.rs`, `tests/snapshots/` |
| Engine unit | `crates/navi-sdk/src/engine/tests.rs` |
| Runtime e2e unit | `crates/navi-core/src/runtime/tests.rs` |
| Tool async | `crates/navi-core/src/tool/tests.rs` |
| Wiremock | `crates/navi-openai/src/tests.rs` |
| Ignored hangs | headless_e2e, tui compact command test |

---

*End of audit. No code changes recommended in this document were applied beyond writing this report.*

---

## Remediation applied 2026-07-09

High-ROI fixes from this audit (implement pass):

| # | Change | Status |
|---|--------|--------|
| 1 | **Deleted orphan** `crates/navi-tui/tests/integration/` (screenshots/scenarios/event_loop duplicates — not Cargo-discovered) | Done |
| 2 | **CI PR fast path**: `Test` job runs `cargo test --lib --bins` product packages + TUI screenshots + CLI `pty_smoke`; full workspace integration moved to `test-full` (main push / tags / `workflow_dispatch` only). No `CARGO_BUILD_JOBS=2`. | Done |
| 3 | **justfile**: documented `test` vs `test-fast`; added `test-smoke` (screenshots + pty). `verify` stays on `test-fast`. | Done |
| 4 | **Harness / unit tests skip real `NaviEngine`**: `TuiApp::new_with_engine`; `Harness::new` / `with_engine` / `test_app` inject `MockEngine` (no build-then-replace). | Done |
| 5 | **Isolated test data dirs** (no shared `/tmp/navi-test` races); disable `updates.check_enabled` + `registry.update_enabled` in test configs. | Done |
| 6 | **`spawn_update_check`**: no-op without tokio runtime (unit tests constructing `TuiApp` outside `#[tokio::test]`). | Done |
| 7 | **pty_smoke**: poll shared buffer up to 2s for welcome controls instead of fixed 400ms sleep. | Done |
| 8 | **scenarios**: poll deadline 5s→3s; yield + short sleeps; clear construction mock calls before turn waits; multi_thread kept (submit uses `block_in_place`). | Done |
| 12 | **SQLite `block_in_place`**: session/stream helpers only block on multi_thread runtimes (safe under `current_thread` unit tests). | Done |
| 9 | **Snapshot mismatch**: truncate expected/actual dumps to 2k chars. | Done |
| 10 | **Tokio flavor**: prefer default/`current_thread` for navi-tui unit async tests that do not need multi_thread. | Done |
| 11 | **Ignored hang/flake (unchanged, still skipped)**: `headless_e2e`, compact palette, wasm fake plugin loads. | Kept |

### Still deferred (not in this pass)

- Merge TUI integration binaries into one target
- Feature-split embeddings/tree-sitter for test builds
- `cargo-nextest` CI profile / slow quarantine
- Re-enable `headless_e2e` with deterministic mock protocol
- Real wasm fixtures for ignored plugin tests
- Shared OnceLock registry catalog fixtures
- Mass-convert remaining multi_thread suites outside navi-tui (runtime/mcp/sdk)
- Gate screenshots behind `RUN_TUI_SNAPSHOTS` env (still always on PR CI)
