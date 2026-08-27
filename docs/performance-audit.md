# NAVI Performance Audit

**Date:** 2026-07-09  
**Scope:** Runtime agent loop, LLM/provider path, TUI, session/persistence, memory/embeddings/voice, plugins/MCP/WASM, build/CI, data structures.  
**Method:** Static code-path analysis of `navi-core`, `navi-sdk`, `navi-tui`, `navi-openai`, `navi-cli`, and related crates. No runtime benchmarks were executed; impact estimates are qualitative from call frequency and algorithmic shape.  
**Constraint:** Report only — no fixes implemented.

---

## Implemented (Phase 1–4 performance roadmap)

Landed in tree after this audit (2026-07-09):

| ID | Change |
|----|--------|
| **P0-1** | `ModelDelta` / `ModelThinkingDelta` marked **transient** in `runtime/mod.rs` (still published to event bus / live subscribers; not pushed to `session.events`). Session title no longer recomputed on every `push_event` — lazy via `update_title_from_events` (end of turn / snapshot). |
| **P1-5 / P1-7 (roadmap)** | LLM post-turn recap is **opt-in**: `tui.llm_recap` (default `false`). Local recap always runs; extra provider call only when enabled. |
| **P1-6 / P1-8 (roadmap)** | TUI session picker uses `SessionStore::list_info()` / `SessionSnapshotInfo`. Full `list()` / snapshot load only when opening a session. |
| **P0-3** | Session-scoped `MemoryManager` shared via `Arc<Mutex<Option<Arc<MemoryManager>>>>` on `TurnContext` and `AgentRuntime`. History sync, auto-memory index, checkpoints/rebuild, extract/dream/distill/consolidate reuse one open instead of reopening 3 SQLite DBs per loop. |
| **P0-2 (partial)** | Streaming chat signature hashes finalized history by lengths; full content only for streaming tail. `ChatRenderCache` keeps a stable history prefix and re-renders only the streaming tail while receiving/thinking. |
| **P1-2 (partial)** | `rewrite_unsupported_attachments` fast path: if no user `content_parts`, single `to_vec()` without rewrite map. |
| **P1-1 (partial)** | Skip tool re-sort in turn request build and openai provider when tools are already name-sorted. |

**Config:** under `[tui]`, set `llm_recap = true` to restore the background LLM recap upgrade after each turn.

**Deferred / not in this pass:** compact/JSONL session format (P0-4), full tool-definition `Arc` cache, `Arc<NaviConfig>`, system-prefix splice, stream retry `Arc<ModelRequest>`, 30fps regional redraw-only, embeddings/onnx feature defaults.

---

## Executive summary (top 10 wins)

| Rank | Finding | Impact × Frequency | Ease | Primary benefit |
|------|---------|-------------------|------|-----------------|
| 1 | **Do not persist/stream-accumulate every `ModelDelta` in session event log** | Very high | S | RAM, disk I/O, save latency |
| 2 | **Incremental chat render during streaming** (avoid full markdown re-render of all history per delta) | Very high | M | TUI CPU / jank |
| 3 | **Reuse `MemoryManager` / SQLite connections** instead of open-3-DBs per sync | High | S–M | Turn latency, FD/CPU |
| 4 | **Cache tool definitions / schemas per request** (stop clone+sort+simplify N times) | High | M | CPU, allocs, request build time |
| 5 | **Avoid cloning full message history on every model call** (`rewrite_unsupported_attachments`) | High | M | Allocs, latency |
| 6 | **Session save: compact JSON, redact once, coalesce deltas** | High | M | Disk I/O, RAM peaks |
| 7 | **Gate or default-off LLM recap extra call every turn** | Medium–high | S | Cost, post-turn latency |
| 8 | **Session picker: use `list_info` not full `list()`** | Medium | S | Startup / modal open latency |
| 9 | **Default-off heavy features in CI/product builds** (voice `onnx` download-binaries, embeddings candle) | Medium | M | CI time, binary size |
| 10 | **Stabilize prompt-cache path further** (already partially done; drop redundant sort/serialize) | Medium | S | Cache hit rate / billing |

Already in good shape (do not regress):

- Tool list is sorted for provider prefix cache stability (`tool/registry.rs`, `turn/mod.rs`, provider builders).
- System vs developer prompt split for cache-friendly `instructions` (`prompt.rs`, Responses API path).
- Chat Completions path skips duplicate system message when `instructions` is set (`navi-openai/.../openai.rs` tests).
- TUI has dirty-draw + chat render cache with signature hashing (not full redraw every 250ms when idle).
- Session save has async `spawn_blocking` variants.
- MCP connect has overall timeout; stderr is drained not inherited.

---

## Critical hotspots (P0) — with file:line evidence

### P0-1. Every stream delta is stored in the session event log

**Problem**  
`ModelDelta` / `ModelThinkingDelta` are not classified as transient. Each streamed token chunk becomes a permanent `AgentEvent` in `SessionState.events`. That vector is later cloned, redacted, and pretty-printed to disk on save.

**Evidence**

```1531:1540:crates/navi-core/src/runtime/mod.rs
        let transient = matches!(
            event,
            AgentEvent::SubagentActivity { .. } | AgentEvent::SubagentTranscript { .. }
        );
        // ...
        if !transient {
            self.session.push_event(event);
        }
```

```174:178:crates/navi-core/src/runtime/session_state.rs
    pub fn push_event(&mut self, event: AgentEvent) {
        self.events.push(event);
        self.updated_at = current_unix_timestamp();
        self.title = session_title_from_events(&self.events);
    }
```

`session_title_from_events` walks the event list looking for headings (`session.rs` ~57–69). That means **O(events)** work on **every delta**.

**Why it matters**  
A 5k-token stream with ~500–2000 SSE chunks can add thousands of events. Session JSON grows roughly with stream chunk count × message size (often worse than storing final text once). Redaction then walks every string field (`security.rs` `redact_snapshot_events`). Save becomes a multi-MB serialize + write after each turn.

**Proposed fix**

1. Mark `ModelDelta` / `ModelThinkingDelta` (and optionally high-churn `HarnessTrace`) as transient — do not push to `session.events`.
2. Persist only `ModelOutput` (final) plus tool/approval/user events.
3. Compute session title once from first user task / final model output, not on every push.
4. Optional: coalesce in-memory for live subscribers only.

**Effort:** S · **Risk:** Low (TUI already accumulates deltas into the assistant bubble in `dispatch.rs` ~280–288).  
**Benefit:** Large RAM + disk + save CPU; secondary win on title scans.

---

### P0-2. Full chat markdown re-render on every streaming delta

**Problem**  
`chat_render_signature` hashes **full content of every message** on each cache check. Streaming appends to `message.content` every delta → signature always changes → `build_chat_render_for_messages` rebuilds **all** history (markdown, tables, code highlight, tools).

**Evidence**

```662:716:crates/navi-tui/src/view/chat.rs
fn chat_render_signature(app: &TuiApp) -> u64 {
    // ...
    for msg in &app.messages {
        msg.role.hash(&mut hasher);
        msg.content.hash(&mut hasher);
        // ... thinking, status, labels, tool_result.ok, timestamps ...
    }
```

```280:288:crates/navi-tui/src/dispatch.rs
        AgentEvent::ModelDelta { text } => {
            let message = ensure_tail_model_response(app);
            message.content.push_str(&text);
            message.status = Some("receiving".to_string());
        }
```

```35:47:crates/navi-tui/src/render/markdown.rs
pub(crate) fn build_chat_render_for_messages(
    messages: &[ChatMessage],
    chat_width: usize,
    // ...
) -> ChatRenderOutput {
    // walks entire message list, markdown-renders each assistant block
```

During loading, pulse frame also forces invalidation:

```669:680:crates/navi-tui/src/view/chat.rs
    if app.is_loading || !app.running_tools.is_empty() || ... {
        running_pulse_frame(elapsed_ms).hash(&mut hasher);
        (elapsed_ms / 1000).hash(&mut hasher);
    }
```

Event loop runs ~30 fps while loading:

```433:436:crates/navi-tui/src/event_loop.rs
        let timeout = if activity_animating || composer_animating {
            Duration::from_millis(33)
```

**Why it matters**  
Long sessions: each token can re-markdown thousands of prior lines. This is the dominant TUI CPU path during agent turns. Also clones visible line slices every frame (`to_vec()` at chat.rs ~75–76).

**Proposed fix**

1. Split cache into **stable history** (immutable after finalize) + **streaming tail** (append-only partial render).
2. Signature: hash message count + lengths + last-message content only while streaming; full content hash only for finalized messages.
3. For running pulse, invalidate only tool header lines (already partially served by `tool_render_cache`) without re-rendering prose.
4. Avoid `to_vec()` of visible lines; render from slice + temporary style overlays.

**Effort:** M · **Risk:** Medium (scroll anchoring, selection hit-testing).  
**Benefit:** Large TUI CPU reduction; smoother streaming on large histories.

---

### P0-3. `MemoryManager::new` opens three SQLite DBs on every history sync / prompt inject

**Problem**  
`sync_messages_to_history` runs from `maintain_context_budget` **every tool-loop model iteration** and again at end of turn. Each call constructs a new `MemoryManager`, which opens history + auto-memory + global-memory SQLite databases and runs schema init.

**Evidence**

```250:251:crates/navi-core/src/turn/mod.rs
async fn maintain_context_budget(...) {
    let _ = sync_messages_to_history(ctx, messages).await;
```

```1377:1390:crates/navi-core/src/turn/mod.rs
pub async fn sync_messages_to_history(...) {
    let manager = crate::memory::MemoryManager::new(
        ctx.project_dir.clone(),
        ctx.data_dir.clone(),
        &memory_config,
    )?;
    manager.history.record_session_start(...)?;
```

```48:67:crates/navi-core/src/memory/mod.rs
    pub fn new(...) -> Result<Self> {
        // MemoryStore init
        let history = HistoryStore::new(&resolved_sqlite_path)?;
        let auto_memory = AutoMemoryStore::open(&auto_memory_db)?;
        let global_memory = GlobalMemoryStore::open(&global_memory_db)?;
```

Same pattern in `load_auto_memory_index` (turn/mod.rs ~1355–1366) at prompt build time.

**Why it matters**  
For a turn with 20 tool→model rounds: ~20× open/migrate/close cycles. SQLite open + `CREATE TABLE IF NOT EXISTS` is expensive relative to recording a few rows. Contends with filesystem and mutexes.

**Proposed fix**

1. Hold `Arc<MemoryManager>` (or at least `HistoryStore`) on `TurnContext` / runtime for the session lifetime.
2. Lazy-init once; reuse for sync, checkpoints, auto-memory index.
3. Keep write path on blocking pool if needed, but **do not reopen**.

**Effort:** S–M · **Risk:** Low–medium (connection lifetime / multi-session).  
**Benefit:** Material turn latency and I/O reduction when memory is enabled (default path for product use).

---

### P0-4. Full session snapshot rewrite is event-log × pretty-JSON × redaction

**Problem**  
`SessionStore::save` clones/redacts **all** events, `serde_json::to_vec_pretty`, then `fs::write` the entire file. With P0-1, event logs explode.

**Evidence**

```367:391:crates/navi-core/src/session.rs
    pub fn save(&self, snapshot: &SessionSnapshot) -> Result<PathBuf> {
        // ...
        let snapshot = if self.redact_secrets {
            SessionSnapshot {
                // ...
                events: redact_snapshot_events(&snapshot.events),
                // ...
            }
        } else {
            snapshot.clone()
        };
        let data = serde_json::to_vec_pretty(&snapshot)?;
        fs::write(&path, data)?;
```

Called from runtime snapshot after turns (`session_state.rs` `snapshot` / `snapshot_async`) and TUI end-of-turn (`stream.rs` ~145).

**Why it matters**  
Pretty-print inflates size ~2–3× vs compact JSON. Full rewrite is O(total_history) per save. Redaction tokenizes every string (`redact_secrets` char-by-char).

**Proposed fix**

1. Fix P0-1 first (biggest win).
2. Use compact JSON for session files (pretty only for debug export).
3. Incremental append-only event log (JSONL) + small metadata sidecar; or write final ModelOutput only.
4. Redact at event ingress once (store already-redacted events) so save is pure serialize.
5. Debounce/coalesce saves (e.g. once per turn, not mid-stream).

**Effort:** M · **Risk:** Medium (format migration, crash recovery).  
**Benefit:** Disk I/O, peak RAM, exit latency.

---

## High impact (P1)

### P1-1. Tool definitions rebuilt, simplified, sorted multiple times per model request

**Problem**  
Each model request rebuilds the full tool list from live tools:

- `definitions()` → `visible_tool_names()` which calls `visible_definitions()` (clone+sort all) **just for names**
- then again maps every tool through `definition()` + `model_friendly_definition` (deep schema simplify)
- sorts again
- `build_model_request` filters and **sorts again**
- provider stream path **clones and sorts tools a third time** before JSON mapping

**Evidence**

```280:305:crates/navi-core/src/tool/mod.rs
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let visible_names: HashSet<String> =
            self.registry.visible_tool_names().into_iter().collect();
        // for each tool: tool.definition() + model_friendly_definition + metadata clone
        result.sort_by(|a, b| a.name.cmp(&b.name));
```

```106:111:crates/navi-core/src/tool/registry.rs
    pub fn visible_tool_names(&self) -> Vec<String> {
        self.visible_definitions()
            .into_iter()
            .map(|d| d.name)
            .collect()
    }
```

```376:384:crates/navi-core/src/turn/mod.rs
                let all_tools = ctx.tool_executor.definitions();
                let mut tools = ctx.components.harness.filter_tools(...);
                tools.sort_by(|a, b| a.name.cmp(&b.name));
```

```44:47:crates/navi-openai/src/providers/openai.rs
            let mut tools = request.tools.clone();
            tools.sort_by(|a, b| a.name.cmp(&b.name));
            body["tools"] = json!(tools.iter().map(responses_tool_to_json)...);
```

Also `ensure_system_prompt` calls `definitions()` again for the tool manifest (`turn/mod.rs` ~184).

**Why it matters**  
Tool schemas are large JSON trees. Deep clone + recursive simplify + sort on every tool-loop iteration (often 10–50× per user turn) burns CPU and allocator traffic before the network request starts. Cache-stable order is already achieved by registry sort — re-sorting is pure waste if the list is known sorted.

**Proposed fix**

1. Cache `Arc<[ToolDefinition]>` (model-facing, pre-simplified, sorted) invalidated only on register/unregister/plan-mode change.
2. `visible_tool_names()` iterate keys without cloning definitions.
3. Provider: accept already-sorted tools; skip clone+sort; optionally prebuild `Vec<Value>` tool JSON.
4. Separate “definitions for prompt manifest” from “definitions for API tools” if they diverge.

**Effort:** M · **Risk:** Low–medium (cache invalidation on MCP/plugin reload).  
**Benefit:** CPU / allocs every model call; faster time-to-first-byte.

---

### P1-2. `rewrite_unsupported_attachments` clones the entire message list every model call

**Problem**

```393:432:crates/navi-core/src/turn/mod.rs
fn rewrite_unsupported_attachments(...) -> Vec<ModelMessage> {
    messages.iter().cloned().map(|mut message| { ... }).collect()
}
```

Always allocates a full deep copy of conversation history, even when no attachments exist.

**Why it matters**  
History with large tool outputs is multi-MB. Doing this every tool loop doubles peak memory and adds GC-like allocator pressure.

**Proposed fix**  
Fast path: if no user messages have `content_parts`, return `messages.to_vec()` only when required by ownership, or change `ModelRequest` to borrow / use `Cow` / rewrite in place for the request only when attachments present. Even a single `any(|m| !m.content_parts.is_empty())` short-circuit helps.

**Effort:** S–M · **Risk:** Low.  
**Benefit:** Alloc/latency on every model call.

---

### P1-3. `active_config()` clones entire `NaviConfig` repeatedly

**Evidence**

```95:100:crates/navi-core/src/turn/mod.rs
    pub fn active_config(&self) -> crate::config::NaviConfig {
        self.config.read()....clone()
    }
```

Used in `build_model_request`, `rewrite_unsupported_attachments`, memory paths, thinking resolution, etc.

**Why it matters**  
Config includes providers catalog, models, security lists, plugins, MCP servers — not a tiny struct. Cloning several times per loop is pure waste.

**Proposed fix**  
`Arc<NaviConfig>` under the existing `RwLock`, or pass `&NaviConfig` via scoped read guard into request builders. Snapshot once per turn.

**Effort:** S · **Risk:** Low.  
**Benefit:** CPU/allocs; cleaner concurrency story.

---

### P1-4. System prompt prefix rebuild splices messages (O(n) removals)

**Evidence**

```222:248:crates/navi-core/src/turn/mod.rs
    while matches!(messages.first(), Some(m) if m.role == System || Developer) {
        messages.remove(0);  // O(n) each
    }
    // ...
    messages.splice(0..0, prefix);
```

**Why it matters**  
Called once per user turn (not every tool loop), but with large histories `remove(0)` repeatedly shifts the whole `Vec`. Also rebuilds developer blocks and may reopen memory DBs (`combined_memory_injection`).

**Proposed fix**  
Keep a logical “prefix length” or store conversation body separately from system/developer prefix. Rebuild prefix into a small buffer and concatenate only at request build time.

**Effort:** M · **Risk:** Medium (message invariants).  
**Benefit:** Latency + allocs at turn start; cleaner structure for caching.

---

### P1-5. Extra LLM call for recap after every successful turn

**Evidence**

```769:847:crates/navi-tui/src/dispatch.rs
fn maybe_emit_session_recap(...) {
    // local recap immediately
    // then spawn_runtime_task { llm_recap(provider, model, ...) }
}
```

`llm_recap` builds a full `ModelRequest` and `provider.complete` (`recap.rs` ~45–97).

**Why it matters**  
Adds one network round-trip + tokens billed after **every** turn, even simple ones. Competes with the next user input for provider rate limits. Local recap already provides UI text.

**Proposed fix**  
Config flag defaulting to local-only; opt-in LLM upgrade; or upgrade only when local recap is low-confidence / long turns.

**Effort:** S · **Risk:** Low (UX preference).  
**Benefit:** Cost + post-turn latency; less rate-limit pressure.

---

### P1-6. Session list loads full snapshots (including all events)

**Evidence**

```3:4:crates/navi-tui/src/session.rs
pub(crate) fn load_saved_sessions(store: &SessionStore) -> Vec<SessionSnapshot> {
    tokio::task::block_in_place(|| store.list())
}
```

`SessionStore::list` reads every `*.json`, full deserialize (`session.rs` ~406–425).  
`list_info` exists (~428–446) but TUI uses full list at startup (`app.rs` ~284) and when opening sessions modal (`keybindings.rs` ~124).

**Why it matters**  
With bloated event logs (P0-1), opening the sessions modal or starting the TUI can stall on multi-MB parse work on the blocking pool.

**Proposed fix**  
Use `list_info` / `list_info_async` for the picker; load full snapshot only on open.

**Effort:** S · **Risk:** Low.  
**Benefit:** Startup + modal open latency.

---

### P1-7. Stream retry clones the full `ModelRequest`

**Evidence**

```194:206:crates/navi-openai/src/provider.rs
    fn stream(&self, request: ModelRequest) -> ModelStream {
        // ...
        let mut inner_stream = provider.stream_inner(request.clone());
```

**Why it matters**  
Request includes all messages + all tool definitions. On retry (idle timeout, 5xx), another full deep clone is paid. Acceptable rare path, but couples poorly with P1-1/P1-2.

**Proposed fix**  
`Arc<ModelRequest>` or rebuild body once and retry the HTTP body bytes.

**Effort:** S · **Risk:** Low.  
**Benefit:** Retry path RAM/latency.

---

### P1-8. TUI animation forces ~30 fps full draw path while tools run

**Evidence**  
`event_loop.rs` ~407–420: `activity_animating` → draw every 33ms; `advance_tick` + pulse invalidates chat cache (P0-2).

**Why it matters**  
Even with idle 250ms poll, a long-running bash tool keeps the UI at 30 fps re-hashing signatures and often re-rendering chat.

**Proposed fix**  
Redraw only status/tool header regions when only the pulse frame changes; keep chat buffer; use ratatui differential buffer (already default) more effectively by not regenerating line vectors.

**Effort:** M · **Risk:** Medium.  
**Benefit:** Idle CPU during long tool runs.

---

## Medium (P2)

### P2-1. Path mentions rescans the project tree on filter updates

**Evidence**  
`path_mentions.rs`: `filtered_path_candidates` → `list_candidates` + `walk_fuzzy` with `read_dir` recursion (skips common dirs). Called from key handler for palette (~93–94).

**Impact:** Large repos can hitch on each keystroke in `@` palette.  
**Fix:** Cache flat index at open; filter in-memory; debounce; cap walk budget with incremental BFS.  
**Effort:** M · **Risk:** Low.

### P2-2. `history_message_key` serializes JSON to hash

**Evidence**  
`turn/mod.rs` ~1441–1455: `serde_json::to_string` on `content_parts` and `tool_calls` for every pending message each sync.

**Fix:** Hash fields directly or store stable message ids.  
**Effort:** S · **Risk:** Low.

### P2-3. Prompt cache clones file contents on hit

**Evidence**  
`prompt.rs` ~59: `return Ok(cached.content.clone());` — hits still allocate full AGENTS.md string.

**Fix:** `Arc<str>` in cache.  
**Effort:** S · **Risk:** Low.

### P2-4. Secret redaction is per-character tokenization

**Evidence**  
`security.rs` `redact_secrets` (~787–803) walks every char; applied to every event field on save.

**Fix:** With P0-1 + redact-at-ingress, cost collapses. Optionally use aho-corasick / precompiled regexes once (some `Regex::new` still appear later in security for path deny patterns ~1298).  
**Effort:** S · **Risk:** Low–medium (false positives).

### P2-5. MCP server process spawn cost

**Evidence**  
`navi-mcp/src/lib.rs` `load_configured_mcp_servers` / `connect_server`: spawns child processes, handshake, list tools, overall timeout. Happens at tooling load time, not every turn.

**Impact:** Startup only; still painful with many MCP servers.  
**Fix:** Parallel connect (already sequential loop), lazy connect on first tool use, cache tool definitions.  
**Effort:** M · **Risk:** Medium.

### P2-6. WASM plugin load instantiates modules at tooling build

**Evidence**  
`navi-sdk/src/tooling.rs` scans plugin roots and loads WASM; `navi-plugin-runtime` `Module::new` + `instantiate` per plugin.

**Impact:** Startup / reload path.  
**Fix:** Lazy instantiate on first invoke; keep compiled module cache.  
**Effort:** M · **Risk:** Medium.

### P2-7. Embeddings model load (feature default on in navi-core)

**Evidence**  
`navi-core/Cargo.toml` `default = ["embeddings", "code-vfs"]`; `embedding.rs` OnceLock cache + candle GGUF load on first use; test embed on load.

**Impact:** First memory semantic search: multi-second + ~hundreds of MB RAM. Binary/deps weight from candle.  
**Fix:** Keep feature optional; document; load off hot path; skip test embed.  
**Effort:** S · **Risk:** Low.

### P2-8. Voice ONNX downloads ORT binaries at build time

**Evidence**  
`navi-voice/Cargo.toml`: default feature `onnx` with `ort` `download-binaries`.

**Impact:** Clean CI/build network dependency and longer compile when voice is in the workspace build.  
**Fix:** Default-off `onnx` for workspace default-members; enable in release product features only.  
**Effort:** S · **Risk:** Low (feature matrix).

### P2-9. Background command poller every 1s

**Evidence**  
`navi-tui/src/background.rs` `BG_POLL_INTERVAL = 1s`.

**Impact:** Minor; acceptable. Could use event-driven completion from runtime.  
**Effort:** M · **Risk:** Low.

### P2-10. `session_title_from_events` on every event push

Covered under P0-1; even after delta fix, still re-scans full event list for every tool event. Fix: set title once when first user/model text arrives.

### P2-11. Duplicate conversation state in TUI

TUI keeps `messages`, `conversation_history`, and `events` in parallel (`dispatch.rs` tool completed pushes to all). Memory triplication of tool I/O.

**Fix:** Single source of truth with views (longer-term architecture).  
**Effort:** L · **Risk:** High.

### P2-12. OAuth / ad-hoc `reqwest::Client::new()` 

**Evidence**  
`navi-openai/src/oauth.rs` multiple `Client::new()` — connection pool not reused for device-flow polling.

**Impact:** Auth flows only.  
**Fix:** Shared client.  
**Effort:** S · **Risk:** Low.

### P2-13. Tree-sitter / navi-vfs weight

Workspace pulls many tree-sitter language crates (`Cargo.toml` workspace.deps). Feature-gated via `code-vfs` but default-on in core.

**Impact:** Compile time + binary size.  
**Effort:** M · **Risk:** Medium.

---

## Build / CI performance

### Workspace shape

- **~20 crates** under `crates/` (core, tui, openai, plugins, voice, vfs, napi, dart, server, …).
- `justfile` already optimizes:
  - `test-fast`: product packages only, lib+bins
  - `test`: exclude napi/dart/server
  - Uncapped `CARGO_BUILD_JOBS` (good)
- Default features pull **embeddings (candle)** and **code-vfs (tree-sitter stack)** into `navi-core`.

### Heavy CI / test surfaces

| Surface | Path | Cost driver |
|---------|------|-------------|
| PTY smoke | `navi-cli/tests/pty_smoke.rs` | Spawns full `navi` binary in PTY |
| TUI goldens | `navi-tui/tests/screenshots.rs` + integration | Many full render passes + snapshot I/O |
| Headless e2e | `navi-cli/tests/headless_e2e.rs` | Full binary paths |
| Plugin install test | `navi-cli/tests/plugin_install_update.rs` | FS + plugin pipeline |
| Voice libri | `navi-voice/tests/transcribe_libri.rs` | Needs model on disk; feature-gated |

### Remaining build wins

1. **Default-off** `navi-voice/onnx` and/or exclude `navi-voice` from default CI check when not needed.
2. **Split** `navi-core` default features: `embeddings` off in CI; optional product feature.
3. **Keep** `test-fast` as PR gate; full workspace + PTY on main/nightly only (if not already).
4. **Cargo sparse registry / sccache** (infra) — not code, but high leverage.
5. Reduce tree-sitter languages to those actually used by vfs minify/code tools if all 15 are always linked.

---

## Architectural notes (agent loop shape)

### Hot path (each tool-loop iteration)

```
run_turn
  ensure_system_prompt          // once per user turn: spawn_blocking prompt.build, DB open for memory, splice messages
  loop:
    maintain_context_budget     // MemoryManager::new + SQLite sync; micro_compact scan; maybe auto_compact LLM
    build_model_request         // clone config; definitions×N; clone all messages
    provider.stream             // serialize tools+messages JSON; HTTP; SSE
    execute tools               // parallel with RwLock exclusivity
  sync_messages_to_history      // again
```

### What is already optimized

- Parallel tool execution with shared/exclusive modes (`turn/mod.rs` execution_lock).
- Prompt file mtime cache (`PromptCache`).
- Tool order stability for provider prefix caching (documented in registry).
- Separated `instructions` vs developer messages for cache hygiene.
- Chat Completions de-duplication of system when instructions set.
- TUI dirty flag + 250ms idle poll (not a busy spin when idle).
- Async session save helpers.

### Cache billing vs cache key stability

- Tools sorted in registry, turn request, and provider (redundant but stable).
- `prompt_cache_key` / `prompt_cache_retention` supported from provider config options.
- Risk: dynamic developer messages (AGENTS.md, skills, memory index) after the base instructions are intentional; keep base instructions stable across turns when config/tools unchanged. Rebuilding tool manifest only when tool set hash changes is already cached in `PromptCache::render_tool_manifest`.

---

## Recommended optimization roadmap (phased PR plan)

### Phase 0 — Quick wins (1–2 PRs, &lt;1 day each)

1. Mark stream deltas transient; stop title recompute on every event.
2. Session picker → `list_info`.
3. Fast path in `rewrite_unsupported_attachments` when no attachments.
4. `visible_tool_names` without full definition clone.
5. Config flag: LLM recap opt-in / default local-only.
6. Skip tool re-sort in provider if already sorted (or document single sort owner).

### Phase 1 — Session & memory I/O (1 week)

1. Session-scoped `MemoryManager` / SQLite handles on runtime.
2. Compact JSON session format + redact-at-ingress.
3. Save once per turn; never mid-stream.
4. Optional: JSONL event append for crash-safe incremental persist.

### Phase 2 — Request build path (1 week)

1. Cached sorted model-facing tool definitions (`Arc`).
2. `Arc<NaviConfig>` or turn-local config snapshot.
3. Avoid full message clone; borrow-friendly `ModelRequest` or in-place attachment rewrite.
4. Pre-serialize tools JSON when tool set stable (big win for many MCP tools).

### Phase 3 — TUI streaming (1–2 weeks)

1. Incremental chat cache: frozen prefix + live tail.
2. Pulse animation without full signature rehash of history.
3. Reduce dual history (`messages` vs `conversation_history` vs `events`) over time.

### Phase 4 — Build/CI & optional weight (ongoing)

1. Feature flags: embeddings, onnx, code-vfs granularity.
2. PR CI = `test-fast` + fmt/clippy; full PTY/screenshots on main.
3. Lazy MCP connect / WASM instantiate.

---

## Metrics to add (what to measure)

Instrument with `tracing` spans + counters (or a small `navi_metrics` module):

| Metric | Where | Why |
|--------|-------|-----|
| `turn.request_build_ms` | `build_model_request` | Catch definition/message clone cost |
| `turn.definitions_ms` / `definitions_calls` | `ToolExecutor::definitions` | Cache effectiveness |
| `turn.history_sync_ms` / `sqlite_open_count` | `sync_messages_to_history` | Prove MemoryManager reuse |
| `turn.prompt_build_ms` | `ensure_system_prompt` | spawn_blocking cost |
| `provider.serialize_ms` / `request_bytes` | openai stream body build | Payload size & CPU |
| `provider.ttfb_ms` / `stream_chunk_count` | stream | Network vs local |
| `session.event_count` / `session.json_bytes` / `session.save_ms` | `SessionStore::save` | Persistence health |
| `session.model_delta_events` | push_event | Should go to ~0 after fix |
| `tui.chat_cache_hit` / `tui.chat_rebuild_ms` / `tui.draw_ms` | ensure_chat_cache / draw | Streaming jank |
| `tui.signature_hash_ms` | chat_render_signature | Hash vs render split |
| `memory.embed_load_ms` / `memory.search_ms` | embedding path | First-use cost |
| `mcp.connect_ms` per server | load_configured_mcp_servers | Startup |
| `wasm.instantiate_ms` | plugin runtime | Startup/reload |
| `recap.llm_ms` / `recap.invoked` | maybe_emit_session_recap | Cost control |

Optional: microbench for `definitions()`, `build_chat_render_for_messages` with synthetic 100/500/2000 message histories, and session save with 1k/10k/50k events.

---

## Finding index (effort / risk / benefit)

| ID | Problem | Effort | Risk | Benefit |
|----|---------|--------|------|---------|
| P0-1 | Persist all ModelDeltas + title scan | S | Low | RAM, disk, save |
| P0-2 | Full chat re-render per delta | M | Med | TUI CPU |
| P0-3 | SQLite reopen every sync | S–M | Low–Med | Turn latency |
| P0-4 | Full pretty JSON rewrite | M | Med | Disk, RAM |
| P1-1 | Tool def clone/sort/simplify ×N | M | Low–Med | CPU per request |
| P1-2 | Clone all messages each request | S–M | Low | Allocs |
| P1-3 | Clone NaviConfig often | S | Low | Allocs |
| P1-4 | System prefix splice remove(0) | M | Med | Turn start |
| P1-5 | LLM recap every turn | S | Low | Cost, latency |
| P1-6 | Session list full load | S | Low | Startup/modal |
| P1-7 | Stream retry request clone | S | Low | Retry RAM |
| P1-8 | 30fps redraw while tools run | M | Med | Idle CPU |
| P2-* | Path walk, hash serialize, prompt Arc, MCP, WASM, features | S–L | varies | Niche/startup |

---

## Appendix: files surveyed

### navi-core
- `src/turn/mod.rs`, `src/turn/tests.rs`
- `src/runtime/mod.rs`, `src/runtime/session_state.rs`, `src/runtime/event_bus.rs`
- `src/session.rs`, `src/security.rs`, `src/compact.rs`, `src/recap.rs`
- `src/prompt.rs`, `src/harness.rs`, `src/repetition.rs`
- `src/tool/mod.rs`, `src/tool/registry.rs`, `src/tool/metadata.rs`
- `src/memory/mod.rs`, `src/memory/history_store.rs`, `src/memory/embedding.rs`, `src/memory/maintenance.rs`
- `src/config/types.rs`, `src/config/providers/mod.rs`
- `Cargo.toml` (features)

### navi-openai
- `src/provider.rs`, `src/transport.rs`, `src/oauth.rs`
- `src/providers/openai.rs`, `src/providers/commandcode.rs`, `src/mapping.rs`, `src/sse.rs`

### navi-tui
- `src/event_loop.rs`, `src/dispatch.rs`, `src/stream.rs`, `src/chat.rs`
- `src/view/chat.rs`, `src/view.rs`, `src/render/markdown.rs`, `src/render/syntax.rs`, `src/render/status.rs`
- `src/persistence.rs`, `src/session.rs`, `src/background.rs`, `src/path_mentions.rs`, `src/app.rs`
- `tests/screenshots.rs`, `tests/integration/*`

### navi-sdk
- `src/engine.rs`, `src/tooling.rs`, `src/plugins.rs`, `src/engine_driver.rs`

### Other
- `navi-mcp/src/lib.rs`
- `navi-plugin-runtime/src/runtime.rs`
- `navi-voice/Cargo.toml`, `src/download.rs`
- `navi-cli/tests/pty_smoke.rs`
- Root `Cargo.toml`, `justfile`

### Docs consulted (context only)
- `docs/compaction.md`, `docs/tui.md`, `docs/auto-memory.md` (not re-audited line-by-line)

---

## Skeptical caveats

1. **No wall-clock profiles** were collected in this audit. Rankings assume a typical multi-tool coding turn with streaming and memory enabled.
2. **Network LLM latency** will still dominate end-to-end turn time after local wins; local optimizations matter most for UI jank, multi-step tool loops, and large sessions.
3. Some “clones” are necessary for async task boundaries; measure before large refactors to `Arc` everything.
4. Prompt-cache monetary savings depend on the provider; order stability is necessary but not sufficient without stable prefix bytes.
5. Dropping `ModelDelta` from persistence must preserve any external API consumers that reconstruct streams from session JSON — verify SDK/NAPI/session reload paths only need final outputs (TUI reload path reconstructs from events in `persistence.rs`; confirm it prefers `ModelOutput` over deltas).

---

*End of report.*
