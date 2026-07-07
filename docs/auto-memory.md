# Auto-Memory

NAVI implements a persistent auto-memory system with SQLite as the source of truth. The model can save, search, and manage memories through the `memory` tool, and background processes automatically extract and consolidate memories.

## Architecture

| Component | Module | Role |
|---|---|---|
| Auto-memory store | `memory/auto_memory.rs` | SQLite database with structured fields |
| Embedding model | `memory/embedding.rs` | Qwen3-Embedding-0.6B via candle 0.11 (feature-flagged) |
| Auto-dream | `memory/auto_dream.rs` | 3-gate consolidation scheduler |
| extractMemories | `memory/extract.rs` | Per-turn background memory extraction |
| Dream maintenance | `memory/maintenance.rs` | Model-based consolidation + SQLite stale/dedup + embedding backfill |

## Memory Types

| Type | Description |
|---|---|
| `user` | Preferences, identity, working style |
| `feedback` | Behaviors to repeat or avoid |
| `project` | Non-derivable project context (deadlines, decisions) |
| `reference` | Links to dashboards, external docs |

## Memory Lifecycle

| Status | Meaning |
|---|---|
| `active` | Memory is current and injected into sessions |
| `needs_review` | Stale (last_seen > 30 days) — dream marks these |
| `obsolete` | Duplicated or contradicted — excluded from injection |

## The `memory` Tool

The model interacts with auto-memory through the `memory` tool with 6 actions:

| Action | Parameters | Description |
|---|---|---|
| `write` | `id`, `memory_type`, `name`, `description`, `body` | Creates or replaces a memory. Generates embedding if model is available. |
| `read` | `id` | Returns full memory entry. |
| `list` | `status?`, `limit?` | Lists memories, optionally filtered by status. |
| `search` | `query`, `limit?` | Semantic search (cosine similarity) if embeddings available, falls back to text matching (SQL LIKE). |
| `update` | `id`, `name?`, `description?`, `body?`, `status?` | Updates fields and/or status. Regenerates embedding if content changed. |
| `delete` | `id` | Permanently deletes a memory. |

## Semantic Search

When the `embeddings` feature is enabled and the model is present, `memory(action='search')` uses cosine similarity over 256-dim Matryoshka-truncated embeddings. Falls back to text matching (SQL LIKE) when the feature is off or the model is missing.

### Embedding Model

- **Model**: Qwen3-Embedding-0.6B (GGUF Q8_0, ~400MB)
- **Framework**: candle 0.11 (pure Rust, no C++ dependency)
- **Dimensions**: 1024 native, truncated to 256 via Matryoshka representation
- **Storage**: 256 × 4 bytes = 1KB per memory in SQLite BLOB column
- **CPU latency**: ~20-60ms per query

### Setup

```bash
navi memory init --embeddings
```

Downloads the GGUF model and tokenizer from HuggingFace to `{data_dir}/memory/{project_hash}/models/`.

## extractMemories (Per-Turn Extraction)

After each completed turn, a background `tokio::spawn` calls the model to extract durable memories from the conversation. This is fire-and-forget and does not block the agent loop.

**Mutual exclusion**: if the model already used the `memory` tool with `write` during the turn, background extraction is skipped — the model's explicit writes take priority.

## Auto-Dream (Periodic Consolidation)

Triggered after each turn via `try_auto_dream()`. Passes 3 gates before executing:

| Gate | Condition | Default |
|---|---|---|
| Time | `>= dream_interval_days * 24h` since last dream | 1 day (24h) |
| Sessions | `>= 5` sessions since last dream | 5 |
| Lock | No other process consolidating | File lock with PID + 1h stale detection |

When all gates pass, spawns a background consolidation:
1. Marks stale memories (`last_seen > 30 days` → `needs_review`)
2. Deduplicates (same type + description → older marked `obsolete`)
3. Backfills missing embeddings (if embedding model is available)

### Manual Dream

```bash
navi memory dream --apply
```

Runs the full model-based dream: reads session history, consolidates MEMORY.md + global memory via model call, then runs SQLite consolidation + embedding backfill.

## Auto-Distill

Same 3-gate pattern with `distill_interval_days` (default 30 days). Runs stale detection at 60 days + dedup. SOP extraction via model is available via `navi memory distill` (manual).

## Session End Consolidation

When a session ends (new session starts or NAVI exits), `consolidate_auto_memory()` runs a lightweight stale + dedup pass without requiring a model call.

## CLI Commands

```bash
navi memory init                  # Create SQLite DB + directories
navi memory init --embeddings     # Download Qwen3-Embedding-0.6B GGUF + tokenizer
navi memory init --embeddings --force  # Re-download model
navi memory status                # Show memory system status
navi memory dream --apply         # Run model-based dream consolidation
navi memory distill               # Run SOP distillation
navi memory doctor                # Validate memory system health
navi memory history <query>       # Search session history
navi memory checkpoint            # Run manual checkpoint writer
navi memory rebuild-preview       # Preview rebuild context
```

## Config

```toml
[memory]
enabled = true
dream_interval_days = 1           # Auto-dream interval (24h)
distill_interval_days = 30        # Auto-distill interval
embedding_model_path = ""         # Override GGUF path (empty = default)
embedding_tokenizer_path = ""     # Override tokenizer path (empty = default)
```

## Storage

All auto-memory state lives under `{data_dir}/memory/{project_hash}/`:

```
memories.db                        ← SQLite (source of truth)
models/
  qwen3-embedding-0.6b-q8_0.gguf   ← Embedding model (optional)
  tokenizer.json                    ← Tokenizer (optional)
last_dream_at                      ← Auto-dream timestamp
dream.lock                         ← Cross-process dream lock
checkpoint.md                      ← Session checkpoint (existing)
notes.md                           ← Session scratchpad (existing)
MEMORY.md                          ← Rendered index (existing, legacy)
```

## SDK API

`NaviEngine` exposes 8 methods for programmatic memory access:

```rust
engine.memory_write("redis_tests", MemoryType::Feedback, "Redis for Tests", "Need Redis", "Start Redis before tests")?;
engine.memory_read("redis_tests")?;
engine.memory_list(Some(MemoryStatus::Active))?;
engine.memory_search("redis", 10)?;
engine.memory_update("redis_tests", None, None, Some("new body"), None)?;
engine.memory_delete("redis_tests")?;
engine.memory_count()?;           // → 42
engine.memory_index();            // → markdown string for prompt injection
```

All methods are also bound in `navi-napi` as `#[napi]` methods for Node.js/Electron clients.

## Events

| Event | When |
|---|---|
| `AutoDreamStarted` | 3 gates passed, dream spawned |
| `AutoDreamCompleted` | Consolidation finished (stale, duplicates, active count) |
| `AutoDreamFailed` | Consolidation error (lock released for retry) |

The TUI shows a notification when `AutoDreamCompleted` is received.
