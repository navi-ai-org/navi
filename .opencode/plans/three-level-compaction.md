# Three-Level Compaction System for NAVI

Inspired by Claude Code's leaked architecture, implementing three levels of
conversation compaction running in parallel.

## Current State

- **No compaction/summarization exists.** Only `compact_tool_observation()` in
  `harness.rs` truncates individual tool results by byte budget.
- **No context window tracking.** `ProviderModelConfig` only has `name` +
  `task_size` (Large/Small).
- **Token counting is a rough heuristic:** `word_count * 4 / 3`, displayed only
  on the welcome screen.
- **`ModelStreamEvent::Usage` is discarded** in `turn.rs:144` (`_ => {}`).
- **No timestamps on messages** — can't detect time gaps.
- **No `/compact` command** in the command palette.
- **No persistent status bar** — context % disappears after first message.

---

## Phase 1 — Foundation

### 1.1 Add `created_at` to `ModelMessage`

**File:** `crates/navi-core/src/model.rs`

Add `created_at: Option<u64>` field (unix millis, `serde(default, skip)`) to
`ModelMessage`. The `skip` ensures it's metadata-only and never sent to the
provider.

Update `ModelMessage::new()`, `ModelMessage::user()`, `ModelMessage::assistant()`,
`ModelMessage::tool_result()`, `ModelMessage::assistant_tool_call()` to set
`created_at: Some(current_unix_timestamp())` using a helper or `std::time`.

### 1.2 Add `context_window_tokens` to `ProviderModelConfig`

**File:** `crates/navi-core/src/config.rs`

Add `pub context_window_tokens: Option<u64>` to `ProviderModelConfig` with
`#[serde(default)]`.

Update the `model()` helper to accept an optional context window:
```rust
fn model(name: &str, task_size: ModelTaskSize) -> ProviderModelConfig {
    ProviderModelConfig { name: name.to_string(), task_size, context_window_tokens: None }
}

fn model_with_ctx(name: &str, task_size: ModelTaskSize, ctx: u64) -> ProviderModelConfig {
    ProviderModelConfig { name: name.to_string(), task_size, context_window_tokens: Some(ctx) }
}
```

Add context window values to all built-in providers:
- **openai**: gpt-5.5 → 1_000_000, gpt-5.4 → 1_000_000, gpt-5.4-mini → 512_000,
  gpt-4.1 → 1_000_000, gpt-4.1-mini → 512_000, gpt-4o → 128_000, o3 → 200_000, o4-mini → 200_000, etc.
- **anthropic**: claude-opus-4 → 200_000, claude-sonnet-4 → 200_000,
  claude-haiku-4 → 200_000, claude-3.5-sonnet → 200_000, claude-3.5-haiku → 200_000, etc.
- **google-gemini**: gemini-2.5-pro → 1_000_000, gemini-2.5-flash → 1_000_000,
  gemini-1.5-pro → 2_000_000, gemini-1.5-flash → 1_000_000, etc.
- **xai**: grok-4 → 256_000, grok-3 → 131_072, etc.
- **deepseek**: deepseek-chat → 128_000, deepseek-reasoner → 128_000
- **groq**: llama models → 128_000, etc.
- **ollama/lmstudio/llamacpp**: None (unknown, use fallback)

Add `DEFAULT_CONTEXT_WINDOW = 128_000` constant.

Add public function:
```rust
pub fn effective_context_window(config: &NaviConfig) -> u64 {
    // Find the selected model's context_window_tokens, fallback to DEFAULT
}
```

### 1.3 Capture `Usage` events

**File:** `crates/navi-core/src/turn.rs`

Replace the `_ => {}` catch-all in the stream loop with explicit handling:
```rust
ModelStreamEvent::Usage { input_tokens, output_tokens } => {
    if let Some(ref tx) = ctx.event_tx {
        let _ = tx.send(AgentEvent::UsageReported {
            input_tokens: input_tokens.unwrap_or(0),
            output_tokens: output_tokens.unwrap_or(0),
        });
    }
}
ModelStreamEvent::Status { .. } => {}
```

### 1.4 Add new `AgentEvent` variants

**File:** `crates/navi-core/src/event.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentEvent {
    // ... existing variants ...

    UsageReported {
        input_tokens: u64,
        output_tokens: u64,
    },
    MicroCompactApplied {
        messages_cleared: usize,
    },
    AutoCompactStarted,
    AutoCompactCompleted {
        tokens_saved: u64,
    },
    AutoCompactFailed {
        reason: String,
    },
}
```

### 1.5 Add compaction config to `HarnessConfig`

**File:** `crates/navi-core/src/config.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HarnessConfig {
    pub profile: HarnessProfile,
    pub observation_bytes_small: usize,
    pub observation_bytes_medium: usize,
    pub micro_compact_gap_minutes: u64,       // default: 60
    pub autocompact_buffer_tokens: u64,        // default: 13_000
    pub autocompact_warning_buffer_tokens: u64, // default: 20_000
    pub autocompact_error_buffer_tokens: u64,   // default: 20_000
    pub autocompact_max_output_tokens: u64,     // default: 20_000
    pub autocompact_max_consecutive_failures: u32, // default: 3
}
```

Update `Default for HarnessConfig` with these new fields.

### 1.6 Update `lib.rs` exports

**File:** `crates/navi-core/src/lib.rs`

After creating `compact.rs` (Phase 2), add:
```rust
pub mod compact;
pub use compact::{CompactState, MicroCompactConfig, AutoCompactConfig, ...};
```

---

## Phase 2 — MicroCompact (Level 1)

### 2.1 New module `crates/navi-core/src/compact.rs`

```rust
use crate::config::NaviConfig;
use crate::model::{ModelMessage, ModelRole};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_MICRO_COMPACT_GAP_MINUTES: u64 = 60;

const READ_ONLY_TOOLS: &[&str] = &[
    "read_file",
    "list_files",
    "grep",
    "bash",
];

pub struct MicroCompactConfig {
    pub gap_threshold_minutes: u64,
}

impl Default for MicroCompactConfig {
    fn default() -> Self {
        Self { gap_threshold_minutes: DEFAULT_MICRO_COMPACT_GAP_MINUTES }
    }
}

/// Clear content of old read-only tool results when the gap since the last
/// assistant message exceeds the threshold. Returns the number of messages
/// that were compacted.
pub fn micro_compact(
    messages: &mut Vec<ModelMessage>,
    config: &MicroCompactConfig,
) -> usize {
    let now = current_unix_millis();
    let gap_threshold_ms = config.gap_threshold_minutes * 60 * 1000;

    // Find the timestamp of the last assistant message
    let last_assistant_ts = messages.iter().rev()
        .find(|m| m.role == ModelRole::Assistant)
        .and_then(|m| m.created_at);

    let Some(last_ts) = last_assistant_ts else {
        return 0; // No assistant messages yet
    };

    if now.saturating_sub(last_ts) < gap_threshold_ms {
        return 0; // Gap not large enough
    }

    let mut cleared = 0;
    for msg in messages.iter_mut() {
        if msg.role == ModelRole::Tool
            && let Some(ref tool_name) = msg.tool_name
            && READ_ONLY_TOOLS.contains(&tool_name.as_str())
            && !msg.content.contains("[Old tool result content cleared]")
        {
            msg.content = "[Old tool result content cleared]".to_string();
            cleared += 1;
        }
    }
    cleared
}

fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
```

### 2.2 Integrate micro_compact in `run_turn`

**File:** `crates/navi-core/src/turn.rs`

Before constructing the `ModelRequest` in the loop:
```rust
// Micro-compact: clear old tool results if gap > threshold
let mc_config = navi_core::compact::MicroCompactConfig {
    gap_threshold_minutes: /* from HarnessConfig */,
};
let cleared = navi_core::compact::micro_compact(messages, &mc_config);
if cleared > 0 {
    if let Some(ref tx) = ctx.event_tx {
        let _ = tx.send(AgentEvent::MicroCompactApplied {
            messages_cleared: cleared,
        });
    }
    tracing::info!(cleared, "micro-compact applied");
}
```

---

## Phase 3 — AutoCompact (Level 2)

### 3.1 Add `CompactState` to `compact.rs`

```rust
pub const AUTOCOMPACT_BUFFER_TOKENS: u64 = 13_000;
pub const WARNING_THRESHOLD_BUFFER_TOKENS: u64 = 20_000;
pub const ERROR_THRESHOLD_BUFFER_TOKENS: u64 = 20_000;
pub const MAX_OUTPUT_TOKENS_FOR_SUMMARY: u64 = 20_000;
pub const MAX_CONSECUTIVE_FAILURES: u32 = 3;

#[derive(Debug, Clone, Default)]
pub struct CompactState {
    pub last_input_tokens: Option<u64>,
    pub context_window: u64,
    pub consecutive_failures: u32,
    pub summary: Option<String>,
    pub summary_message_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactThreshold {
    Normal,
    Warning,
    Error,
    CircuitOpen,
}

impl CompactState {
    pub fn threshold_level(&self) -> CompactThreshold {
        if self.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            return CompactThreshold::CircuitOpen;
        }
        let Some(input_tokens) = self.last_input_tokens else {
            return CompactThreshold::Normal;
        };
        let remaining = self.context_window.saturating_sub(input_tokens);
        if remaining <= ERROR_THRESHOLD_BUFFER_TOKENS {
            CompactThreshold::Error
        } else if remaining <= WARNING_THRESHOLD_BUFFER_TOKENS + AUTOCOMPACT_BUFFER_TOKENS {
            CompactThreshold::Warning
        } else {
            CompactThreshold::Normal
        }
    }

    pub fn should_autocompact(&self) -> bool {
        if self.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            return false;
        }
        let Some(input_tokens) = self.last_input_tokens else {
            return false;
        };
        input_tokens + AUTOCOMPACT_BUFFER_TOKENS >= self.context_window
    }

    pub fn context_percentage(&self) -> u8 {
        let Some(input_tokens) = self.last_input_tokens else {
            return 0;
        };
        if self.context_window == 0 { return 0; }
        ((input_tokens * 100) / self.context_window).min(100) as u8
    }
}
```

### 3.2 Summarization prompts (9 sections)

```rust
pub const COMPACT_PROMPT: &str = r#"You are summarizing a conversation between a user and an AI coding assistant (NAVI). Create a detailed summary with these exact sections:

## 1. Pedido e Intenção Primária
Capture all explicit requests and intents from the user in detail.

## 2. Conceitos Técnicos-Chave
List all technical concepts, technologies, and frameworks discussed.

## 3. Arquivos e Trechos de Código
Enumerate specific files and code snippets that were examined, modified, or created. Include file paths and relevant code.

## 4. Erros e Correções
List all errors that appeared and how they were fixed.

## 5. Resolução de Problemas
Document problems solved and ongoing investigations.

## 6. Todas as Mensagens do Usuário
List ALL user messages that are not tool results. These are critical — include them verbatim, one per line, prefixed with "> ".

## 7. Tarefas Pendentes
List pending tasks that were explicitly requested.

## 8. Trabalho Atual
Describe in precise detail what was being worked on immediately before this summary request.

## 9. Próximo Passo Opcional
List the next step you would take on the current task.

Be thorough and specific. The summary must contain enough detail to continue the conversation seamlessly."#;

pub const PARTIAL_COMPACT_PROMPT: &str = r#"You are extending an existing conversation summary with new content. Preserve the existing summary sections and update them with new information. Add any new user messages to section 6. Update sections 8 and 9 based on the most recent work.

Existing summary:
{previous_summary}

New conversation to summarize:
{new_conversation}

Return the complete updated summary with all 9 sections."#;

pub const PARTIAL_COMPACT_UP_TO_PROMPT: &str = r#"Summarize the conversation up to this point. The summary will be placed at the beginning of the conversation to preserve cache hits. Follow the 9-section format precisely."#;
```

### 3.3 Auto-compact execution logic

```rust
impl CompactState {
    /// Perform auto-compaction by requesting a summary from the model.
    /// Returns Ok(()) on success, Err on failure.
    pub async fn auto_compact(
        &mut self,
        messages: &mut Vec<ModelMessage>,
        model_provider: &dyn ModelProvider,
        model_name: &str,
        tools: &[ToolDefinition],
    ) -> Result<()> {
        if !self.should_autocompact() {
            return Ok(());
        }

        // Build the conversation text to summarize
        let conversation_text = self.build_conversation_text(messages);

        // Choose prompt variant
        let prompt = if let Some(ref prev_summary) = self.summary {
            PARTIAL_COMPACT_PROMPT
                .replace("{previous_summary}", prev_summary)
                .replace("{new_conversation}", &conversation_text)
        } else {
            format!("{}\n\nConversation to summarize:\n{}", COMPACT_PROMPT, conversation_text)
        };

        // Build summarization request
        let request = ModelRequest {
            model: model_name.to_string(),
            messages: vec![
                ModelMessage::system("You are a precise conversation summarizer."),
                ModelMessage::user(prompt),
            ],
            thinking: ThinkingConfig::Off, // Summarization doesn't need thinking
            tools: vec![], // No tools during summarization
        };

        match model_provider.complete(request).await {
            Ok(response) => {
                let summary = response.text;
                let compacted_count = messages.len().saturating_sub(2); // minus system + summary

                // Replace conversation with summary
                let system_msg = messages.first().cloned().unwrap_or_else(|| ModelMessage::system(""));
                messages.clear();
                messages.push(system_msg);
                messages.push(ModelMessage::user(format!(
                    "Here is a summary of the conversation so far:\n\n{}", summary
                )));

                self.summary = Some(summary);
                self.summary_message_count = compacted_count;
                self.consecutive_failures = 0;
                self.last_input_tokens = None; // Will be updated on next Usage event

                Ok(())
            }
            Err(e) => {
                self.consecutive_failures += 1;
                tracing::warn!(
                    failures = self.consecutive_failures,
                    error = %e,
                    "auto-compact failed"
                );
                Err(e)
            }
        }
    }

    fn build_conversation_text(&self, messages: &[ModelMessage]) -> String {
        let mut text = String::new();
        for msg in messages {
            if msg.role == ModelRole::System { continue; }
            let role_label = match msg.role {
                ModelRole::User => "User",
                ModelRole::Assistant => "Assistant",
                ModelRole::Tool => "Tool",
                ModelRole::System => unreachable!(),
            };
            text.push_str(&format!("[{}]: {}\n", role_label, msg.content));
        }
        text
    }
}
```

### 3.4 Integrate auto-compact in `run_turn`

**File:** `crates/navi-core/src/turn.rs`

Add `compact_state: &mut CompactState` parameter to `run_turn` (or make it part
of `TurnContext`). Before each request in the loop:

```rust
if compact_state.should_autocompact() {
    if let Some(ref tx) = ctx.event_tx {
        let _ = tx.send(AgentEvent::AutoCompactStarted);
    }
    match compact_state.auto_compact(
        messages,
        ctx.model_provider.as_ref(),
        &ctx.model_name,
        &ctx.tool_executor.definitions(),
    ).await {
        Ok(()) => {
            if let Some(ref tx) = ctx.event_tx {
                let _ = tx.send(AgentEvent::AutoCompactCompleted {
                    tokens_saved: /* estimate */,
                });
            }
        }
        Err(e) => {
            if let Some(ref tx) = ctx.event_tx {
                let _ = tx.send(AgentEvent::AutoCompactFailed {
                    reason: e.to_string(),
                });
            }
        }
    }
}
```

### 3.5 Update `Usage` tracking in `CompactState`

After receiving `Usage` event in `run_turn`:
```rust
compact_state.last_input_tokens = Some(input_tokens);
compact_state.context_window = effective_context_window; // from config
```

---

## Phase 4 — TUI Changes

### 4.1 Permanent status bar

**File:** `crates/navi-tui/src/lib.rs`

Add a persistent status bar line at the bottom of the chat area (or just above
the input area). Shows:

- `context {X}%` — colored: muted if <70%, yellow if >=70%, red if >=90%
- `compact: ok` / `compact: warning` / `compact: error` / `compact: circuit-open`
- Model name and provider (compact form)

New fields on `TuiApp`:
```rust
compact_state: navi_core::compact::CompactState,
context_window: u64,
```

Update `render()` layout:
```
[0] Min(6)    -> chat area
[1] Length(1) -> status bar (NEW)
[2] Length(1) -> breathing room
[3] Length(7) -> input area
```

### 4.2 `/compact` command

Add `CommandAction::Compact` to the `CommandAction` enum:
```rust
CommandAction::Compact,  // Manual force compact
```

Add to `COMMANDS`:
```rust
CommandItem { label: "Compact Context", shortcut: None, action: CommandAction::Compact },
```

In `run_selected_command()`:
```rust
CommandAction::Compact => {
    // Force auto-compact regardless of threshold
    // Show notification: "Compacting context..."
    app.mode = Mode::Normal;
}
```

This needs a mechanism to request compaction from the TUI side. The simplest
approach: add a flag `force_compact: bool` on `TuiApp` that is checked in
`start_streaming_request()` and passed through to `run_turn`.

### 4.3 Populate `usage_label` from Usage events

In the `AsyncEvent::Agent(AgentEvent::UsageReported { .. })` handler:
```rust
AgentEvent::UsageReported { input_tokens, output_tokens } => {
    app.compact_state.last_input_tokens = Some(input_tokens);
    // Update usage_label on the active assistant message
    if let Some(msg) = app.messages.last_mut() {
        if msg.role == ChatRole::Assistant {
            msg.usage_label = Some(format!(
                "{}k in · {}k out",
                input_tokens / 1000,
                output_tokens / 1000,
            ));
        }
    }
}
```

### 4.4 MicroCompact notification

In the `AsyncEvent::Agent(AgentEvent::MicroCompactApplied { messages_cleared })` handler:
```rust
AgentEvent::MicroCompactApplied { messages_cleared } => {
    app.notification = Some(Notification {
        title: "Micro-Compact".to_string(),
        message: format!("{} old tool results cleared (60+ min gap)", messages_cleared),
        created_at: Instant::now(),
        ttl: Duration::from_secs(5),
    });
}
```

### 4.5 Render summary message differently

In `build_chat_lines()`, detect summary user messages (those starting with
"Here is a summary of the conversation so far:") and render with a special
icon like `◆ [Context Summary]` instead of the normal user message format.

### 4.6 Update `chat_render_signature`

Add `compact_threshold` and `usage_label` to the signature computation so
the cache invalidates when these change.

---

## Phase 5 — Session Memory (Level 3, Experimental)

### 5.1 Project memory store

**File:** `crates/navi-core/src/session.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMemory {
    pub project_hash: String,
    pub entries: Vec<MemoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub created_at: u64,
    pub summary: String,
    pub session_id: String,
}
```

Add to `SessionStore`:
```rust
pub fn save_memory(&self, project_dir: &Path, memory: &ProjectMemory) -> Result<()>
pub fn load_memory(&self, project_dir: &Path) -> Option<ProjectMemory>
```

Memory files stored at `<data_dir>/memory/<project_hash>.json`.

### 5.2 Memory injection on new session

When a new session starts (or `SessionRuntime::spawn` is called), check if
project memory exists. If so, append to the system prompt:

```
Previous session context (summarized):
{latest_summary}
```

Limited to `max_memory_entries` (default: 3) most recent entries.

### 5.3 Memory update on session save

When saving a session that has been compacted (has a `summary` in
`CompactState`), also save the summary to the project memory store.

### 5.4 Config

Add `MemoryConfig` or add fields to an existing config section:
```rust
pub struct MemoryConfig {
    pub session_memory_enabled: bool,    // default: false
    pub max_memory_entries: usize,        // default: 3
}
```

Add to `NaviConfig`:
```rust
pub memory: MemoryConfig,
```

### 5.5 Session snapshot update

Add to `SessionSnapshot`:
```rust
#[serde(default)]
pub memory: Option<ProjectMemory>,
```

---

## Updated `CompactState` for TUI and Session

The `CompactState` struct needs to be shareable between `turn.rs` (async
context) and `TuiApp` (sync context). Options:

1. **Clone-based**: `CompactState` is `Clone`, `run_turn` returns the updated
   state, TUI picks it up.
2. **Event-based**: All state changes flow through `AgentEvent`, TUI maintains
   its own mirror of `CompactState`.

Recommended: **Event-based** — consistent with NAVI's existing architecture.
The TUI mirrors `CompactState` from events:
- `UsageReported` → update `last_input_tokens` and `context_window`
- `AutoCompactCompleted` → update `summary`, reset `consecutive_failures`
- `AutoCompactFailed` → increment `consecutive_failures`

---

## Implementation Order

| Step | Files Changed | Description |
|------|--------------|-------------|
| 1.1 | `navi-core/src/model.rs` | Add `created_at` to `ModelMessage` |
| 1.2 | `navi-core/src/config.rs` | Add `context_window_tokens` to `ProviderModelConfig` |
| 1.3 | `navi-core/src/turn.rs` | Capture `Usage` events, emit `UsageReported` |
| 1.4 | `navi-core/src/event.rs` | Add 4 new `AgentEvent` variants |
| 1.5 | `navi-core/src/config.rs` | Add compaction config fields to `HarnessConfig` |
| 2.1 | `navi-core/src/compact.rs` | NEW — micro_compact logic |
| 2.2 | `navi-core/src/turn.rs` | Integrate micro_compact before request |
| 3.1 | `navi-core/src/compact.rs` | Add `CompactState`, threshold logic |
| 3.2 | `navi-core/src/compact.rs` | Add 9-section summarization prompts |
| 3.3 | `navi-core/src/compact.rs` | Add `auto_compact()` execution |
| 3.4 | `navi-core/src/turn.rs` | Integrate auto_compact in turn loop |
| 3.5 | `navi-core/src/turn.rs` | Wire `Usage` to `CompactState` |
| 4.1 | `navi-tui/src/lib.rs` | Add permanent status bar |
| 4.2 | `navi-tui/src/lib.rs` | Add `/compact` command |
| 4.3 | `navi-tui/src/lib.rs` | Populate `usage_label` from events |
| 4.4 | `navi-tui/src/lib.rs` | MicroCompact notification |
| 4.5 | `navi-tui/src/lib.rs` | Summary message rendering |
| 4.6 | `navi-tui/src/lib.rs` | Update `chat_render_signature` |
| 5.1 | `navi-core/src/session.rs` | Add `ProjectMemory`, memory store |
| 5.2 | `navi-core/src/turn.rs` / `session.rs` | Memory injection on new session |
| 5.3 | `navi-core/src/session.rs` | Memory update on session save |
| 5.4 | `navi-core/src/config.rs` | Add `MemoryConfig` |
| 5.5 | `navi-core/src/session.rs` | Update `SessionSnapshot` |
| final | `navi-core/src/lib.rs` | Export `compact` module |
| final | `docs/` | Update architecture.md, tui.md |

---

## Testing Strategy

### Unit tests (navi-core)

- `micro_compact` only clears read-only tools after gap threshold
- `micro_compact` preserves write tools (write_file, apply_patch)
- `micro_compact` doesn't double-clear already cleared messages
- `micro_compact` returns 0 when gap < threshold
- `CompactState::should_autocompact()` respects buffer and circuit breaker
- `CompactState::threshold_level()` returns correct levels
- `effective_context_window()` returns model-specific or default values
- Summarization prompt formatting

### Integration tests

- Full turn with micro_compact triggered by time gap
- Full turn with auto_compact triggered by context threshold
- Circuit breaker stops after 3 failures
- TUI status bar shows correct context percentage and threshold

### Verification

```bash
cargo fmt
cargo check
cargo test
cargo test -p navi-core
cargo test -p navi-tui
```
