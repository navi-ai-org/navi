# NAVI Goal System (Thread Goals)

## Overview

Thread goals attach a persistent objective to a session. While the goal is
`Active`, NAVI auto-continues turns after the thread goes idle, injects
steering prompts, and tracks token/time budget until the goal is complete,
blocked, budget-limited, or cleared by the host.

## Data model

```rust
pub struct SessionGoal {
    pub session_id: String,
    pub goal_id: GoalId,
    pub objective: String,
    pub short_description: Option<String>, // compact UI label
    pub status: GoalStatus,
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    pub consecutive_blocked_turns: u32,
    pub block_reason: Option<String>,
    pub checklist: Vec<GoalTask>, // optional host-side structure; not required for complete
    pub created_at: u64,
    pub updated_at: u64,
}
```

### Status machine

```text
Active ───────────────────────────────► Complete
  │                                       ▲
  ├──► Paused (host/user only)            │
  ├──► Blocked (model or host)            │
  ├──► UsageLimited (API usage limit)     │
  └──► BudgetLimited (token budget) ──────┘
```

Terminal: `Complete`, `BudgetLimited`. Auto-continue only while `Active`.

## Model tools (when `goals.enabled`)

| Tool | Role |
|---|---|
| `get_goal` | Read objective, status, budget, usage |
| `create_goal` | Create a goal **only** when the user/system explicitly asks. Fails if an unfinished goal exists |
| `update_goal` | `status`: `complete` \| `blocked` only. Pause/resume/budget/usage-limit are host/system |

Do not use goals for ordinary one-pass work. Prefer `plan` for multi-step visibility.

## Host / SDK API

- `set_goal` / `clear_goal` / `update_goal_status` (pause, resume, complete, blocked, …)
- Optional checklist helpers for UIs (`update_goal_checklist`) — not on the model schema
- Events: `GoalUpdated` for live chips/notifications

## Auto-continuation (idle lifecycle)

After each successful `AgentRuntime::send_turn_with_parts`:

1. If `goals.enabled` and the goal is `Active` and auto-continue is on
2. And the agent is not in plan mode and there is no pending user input
3. And under `max_auto_continue_turns` (default 50; 0 = unlimited)
4. Inject the **continuation steering** prompt and start another turn

Continuation content is the internal steering template (objective, budget,
completion/blocked audit). It is not a host user chat message from the human.

## Steering templates

| Template | When |
|---|---|
| Continuation | Thread idle with active goal |
| Budget limit | Token budget exceeded |
| Objective updated | Host edits the objective |

## Config

```toml
[goals]
enabled = true
max_auto_continue_turns = 50
```

## Components

| Piece | Role |
|---|---|
| `GoalService` | Registry of session runtimes; host set/get/clear |
| `GoalRuntimeHandle` | Per-session state, accounting, continue_if_idle |
| `GoalExtension` | Session/turn hooks |
| `steering` | Prompt builders |
| `tools` | Model get/create/update |

## Notes

- Plan mode suppresses auto-continue so the user can approve a plan first.
- Creating a goal replaces only a **terminal** prior goal; unfinished goals must be completed or host-cleared first.
- Checklist fields remain on the snapshot for hosts that want structure; completion does **not** require a checklist.
