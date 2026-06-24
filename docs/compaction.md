# Compaction

NAVI implements conversation compaction and memory layers that manage context window usage across long sessions. The system runs micro-compact and auto-compact before each model request in `navi-core/src/turn.rs`, persists session summaries across sessions, and maintains long-horizon project memory under the NAVI data directory.

All compaction logic lives in `navi-core/src/compact.rs`. Configuration is spread across `HarnessConfig` and `MemoryConfig` in `navi-core/src/config.rs`. The TUI mirrors compaction state via events and displays context usage in the status bar.

## Levels

| Level | Trigger | Mechanism | Data Loss |
|---|---|---|---|
| Micro-compact | Time gap > 60 min since last assistant message | Clears read-only tool result content in-place | Tool output text only |
| Auto-compact | `input_tokens + buffer >= context_window` | Summarizes full conversation via model, replaces messages with system + summary | Full conversation replaced by summary |
| Session memory | Session end with compact summary | Saves summary to `<data_dir>/memory/<project_hash>.json`, injected on next session | None (additive) |
| Long-horizon project memory | Context utilization crosses checkpoint/rebuild thresholds | Stores checkpoint, notes, project memory, and history under `<data_dir>/memory/projects/<project_hash>/` | None (additive/rebuild context replaces live messages only) |

## Micro-Compact

`micro_compact(messages, gap_threshold_minutes)` clears the `content` of tool result messages whose `tool_name` is in the read-only set:

- `read_file`
- `list_files`
- `grep`
- `bash`

Write tools (`write_file`, `apply_patch`) are never cleared. Cleared content is replaced with `[Old tool result content cleared]` and is not double-cleared on subsequent passes.

The gap threshold is configured by `HarnessConfig.micro_compact_gap_minutes` (default: 60). The gap is measured from the `created_at` timestamp of the last `Assistant` message to the current time. Messages without timestamps are ignored.

Emits `AgentEvent::MicroCompactApplied { messages_cleared }`.

## Auto-Compact

`CompactState::auto_compact()` is called when `should_autocompact()` returns true — that is, when `input_tokens + autocompact_buffer_tokens >= context_window` and the circuit breaker is not open.

### Summarization

The conversation (excluding system messages) is serialized to text via `build_conversation_text()` and sent to the model with one of two prompts:

- **`COMPACT_PROMPT`** — used for the first compaction. Produces a 9-section summary.
- **`PARTIAL_COMPACT_PROMPT`** — used when a previous summary exists. Merges new conversation into the existing summary, preserving all 9 sections.

The 9 summary sections are:

1. **Pedido e Intenção Primária** — all explicit user requests
2. **Conceitos Técnicos-Chave** — technologies and frameworks discussed
3. **Arquivos e Trechos de Código** — files examined, modified, or created
4. **Erros e Correções** — errors encountered and how they were fixed
5. **Resolução de Problemas** — problems solved and ongoing investigations
6. **Todas as Mensagens do Usuário** — verbatim user messages (prefixed with `> `)
7. **Tarefas Pendentes** — explicitly requested pending tasks
8. **Trabalho Atual** — what was being worked on immediately before compaction
9. **Próximo Passo Opcional** — suggested next step

After a successful summary, the message list is replaced with:
- The original system message
- A user message containing `"Here is a summary of the conversation so far:\n\n{summary}"`

### Thresholds

`CompactThreshold` is computed from `last_input_tokens` and `context_window`:

| Threshold | Condition | TUI Color |
|---|---|---|
| `Normal` | `remaining > 33K` tokens | `MUTED` |
| `Warning` | `remaining <= 33K` tokens | `ACCENT` |
| `Error` | `remaining <= 20K` tokens | `SIGNAL` |
| `CircuitOpen` | `consecutive_failures >= 3` | `SIGNAL` |

Constants:

| Constant | Value | Purpose |
|---|---|---|
| `AUTOCOMPACT_BUFFER_TOKENS` | 13,000 | Triggers auto-compact when `input + buffer >= window` |
| `WARNING_THRESHOLD_BUFFER_TOKENS` | 20,000 | Warning threshold for remaining tokens |
| `ERROR_THRESHOLD_BUFFER_TOKENS` | 20,000 | Error threshold for remaining tokens |
| `MAX_OUTPUT_TOKENS_FOR_SUMMARY` | 20,000 | Max output tokens for the summary request |
| `MAX_CONSECUTIVE_FAILURES` | 3 | Opens circuit breaker after this many failures |

### Circuit Breaker

After 3 consecutive auto-compact failures, the circuit breaker opens (`CircuitOpen` threshold). No further auto-compact attempts are made. The counter resets on any successful compaction.

Emits:
- `AgentEvent::AutoCompactStarted` — before the summary request
- `AgentEvent::AutoCompactCompleted { tokens_saved }` — on success
- `AgentEvent::AutoCompactFailed { reason }` — on failure

## Session Memory

Session memory persists compact summaries across sessions so a new session can continue where the previous one left off.

### Storage

Memory files are stored at `<data_dir>/memory/<project_hash>.json`, where `project_hash` is a `DefaultHasher` digest of the project directory path.

```rust
pub struct ProjectMemory {
    pub project_hash: String,
    pub entries: Vec<MemoryEntry>,
}

pub struct MemoryEntry {
    pub created_at: u64,      // unix seconds
    pub summary: String,       // compact summary text
    pub session_id: String,    // source session id
}
```

### Injection

When `MemoryConfig.session_memory_enabled` is true and a new session starts (empty `conversation_history`), the TUI loads the project memory and calls `ProjectMemory::format_injection(max_memory_entries)`. This returns a formatted string containing the N most recent entries, which is appended to the system prompt via `build_system_prompt_with_memory()`.

### Saving

When a session ends and the `CompactState` has a `summary`, the TUI calls `SessionStore::add_memory_entry()` to append the summary to the project memory file.

## Long-Horizon Project Memory

NAVI's checkpoint memory is stored outside the project tree. By default, each project gets a private memory directory under:

```txt
<data_dir>/memory/projects/<project_hash>/
```

The directory contains:

- `checkpoint.md`
- `notes.md`
- `MEMORY.md`
- `history.sqlite`
- optional archive/maintenance subdirectories, including dream review copies

If an older project-side `.agent-memory/` directory exists and the new data-dir memory directory is empty, NAVI copies the legacy contents into the data-dir location once. It does not delete the legacy project-side directory.

## Dream Maintenance

Dream maintenance is an offline memory synthesis pass inspired by Claude's managed-agent Dreams. It reads the existing project memory, global memory, checkpoint, notes, and recent session history, then asks the selected model to produce a separate reviewed memory store.

By default, dreams do not modify active memory. NAVI writes each dream result under:

```txt
<data_dir>/memory/projects/<project_hash>/dreams/dream-<timestamp>/
```

Each dream directory contains:

- `MEMORY.md`
- `global-memory.md`
- `dream-report.md`

Use the CLI to run a review-only dream:

```bash
navi memory dream
```

Useful options:

```bash
navi memory dream --sessions 25
navi memory dream --instructions "Preserve active implementation details and remove stale debugging notes"
navi memory dream --apply
```

`--sessions` controls how many recent sessions are mined, capped at 100. `--instructions` steers synthesis. `--apply` first writes the review copy, then replaces the active project and global memory files with the dream output.

## Configuration

### HarnessConfig (compaction fields)

| Field | Default | Purpose |
|---|---|---|
| `micro_compact_gap_minutes` | 60 | Minutes of inactivity before micro-compact runs |
| `autocompact_buffer_tokens` | 13,000 | Buffer that triggers auto-compact |
| `autocompact_warning_buffer_tokens` | 20,000 | Warning threshold for remaining tokens |
| `autocompact_error_buffer_tokens` | 20,000 | Error threshold for remaining tokens |
| `autocompact_max_output_tokens` | 20,000 | Max output tokens for summary generation |
| `autocompact_max_consecutive_failures` | 3 | Consecutive failures before circuit breaker opens |

### MemoryConfig

| Field | Default | Purpose |
|---|---|---|
| `session_memory_enabled` | false | Enable project memory across sessions |
| `max_memory_entries` | 3 | Number of recent summaries to inject |
| `enabled` | true | Enable long-horizon checkpoint/rebuild memory |
| `root` | `memory/projects` | Data-dir-relative base directory for per-project long-horizon memory |

Example `.navi/config.toml`:

```toml
[memory]
session_memory_enabled = true
max_memory_entries = 5
root = "memory/projects"

[harness]
micro_compact_gap_minutes = 30
autocompact_buffer_tokens = 13000
```

## TUI Integration

The TUI mirrors `CompactState` locally from events:

- `UsageReported` → updates `compact_state.last_input_tokens` and populates `ChatMessage.usage_label`
- `MicroCompactApplied` → shows notification
- `AutoCompactCompleted` → shows notification, resets `consecutive_failures`, adds compact summary `ChatMessage` with `is_compact_summary: true`
- `AutoCompactFailed` → pushes diagnostic, increments `consecutive_failures`

### Status Bar

The shortcut tips line (below the input) shows a right-aligned context indicator:

```
 ? for shortcuts · ctrl+p commands · ctrl+c quit    ctx:72% ~compact
```

Color coding: `MUTED` for normal, `ACCENT` for warning, `SIGNAL` for error or circuit-open.

### Compact Context Command

`ctrl+p` → "Compact Context" sets `compact_state.last_input_tokens = context_window`, forcing `should_autocompact()` to return true on the next request.

### Compact Summary Rendering

Messages with `is_compact_summary: true` are rendered with a special header:

```
 ◈ compacted ────────────────────────────────────────
```

## Turn Flow

Each iteration of the turn loop in `navi-core/src/turn.rs` runs compaction before sending a request to the model:

1. **Micro-compact** — clears stale tool results if the time gap exceeds the threshold
2. **Auto-compact** — summarizes the conversation if the context threshold is reached
3. **Model request** — sends the (possibly compacted) messages to the provider
4. **Usage update** — on `ModelStreamEvent::Usage`, updates `CompactState.last_input_tokens` via `AgentEvent::UsageReported`
