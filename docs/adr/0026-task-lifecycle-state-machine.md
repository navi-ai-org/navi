# ADR 0026 — A General Task Lifecycle State Machine, Not Mandatory Modes

## Status

Proposed

## Context

Agent tasks naturally move between investigation, specification, implementation,
verification, repair, redesign, and clarification. A static session mode such as
"vibe coding" or "spec-driven coding" cannot accurately describe this lifecycle:
one task may move through all of those activities.

Today NAVI has a goal status machine and Harness Pack `GraphSpec`, but graph
edges are intentionally soft and do not execute runtime transitions. Tool-loop
errors are tracked, but failure recovery is not yet task-aware.

## Decision

### 1. Model a task as an engine-owned state machine

The eventual task lifecycle is:

```text
INTAKE → SPECIFY → DESIGN → IMPLEMENT → VERIFY → EVALUATE → COMPLETE
                       ▲         │          │         │
                       │         │          ├─ fail → REPAIR ─┘
                       │         │          └─ inconclusive → ASK_USER / INVESTIGATE
                       └─ redesign ────────────────────────────┘
```

`EVALUATE` is optional and advisory. Verification and completion audit are the
hard gates for a mutating task. The state machine controls lifecycle,
transitions, budgets, evidence and recovery; it does not prescribe the model's
framework, files, algorithm, or detailed reasoning strategy.

### 2. Intake is repository-aware and asks only material questions

At intake the model inspects relevant project instructions, code, tests,
dependencies, and documentation before proposing requirements. It may make and
record low-impact, reversible assumptions. It asks the user only when
uncertainty, impact, and irreversibility make a choice material.

There is no mandatory questionnaire and no required user-selected session mode.
UX presets may exist later, but they are not lifecycle protocol or completion
semantics.

### 3. Classify failures before choosing recovery

The harness classifies failures as implementation, design, specification,
environment, verification, or budget failures. It routes repeated identical
verification failures to investigation or redesign rather than blindly applying
more patches. An unresolved material ambiguity routes to `ASK_USER` rather than
being silently invented.

### 4. Refresh task instructions at semantic boundaries

The runtime injects a short, private task anchor after task creation,
compaction, session resume, state transition, and verifier failure. The anchor
contains the current objective, constraints, claims, evidence status, and
completion rule.

This is not a fixed 64k-token protocol. Any token-distance trigger is
model-configurable and experimental; it complements semantic refresh rather
than replacing it.

### 5. State is observable and replayable across engine clients

Task-state transitions and failure classifications are versioned engine events.
They are persisted and redacted for replay, and are surfaced consistently through
the SDK, NAPI, CLI, and TUI when those clients expose task progress. Runtime
behaviour stays in `navi-core`; clients render and control it.

## Consequences

### Positive

- One general lifecycle supports informal feature work, debugging, research,
  and formal proof tasks without forcing a user mode.
- Hard gates apply at state boundaries while preserving agent freedom inside a
  state.
- Failure recovery becomes observable, testable, and less patch-loop-prone.

### Negative

- State persistence, event versioning, replay, and UI projection add work.
- An over-eager state machine could become bureaucratic; transition policy must
  be introduced incrementally and measured against ordinary tasks.
- Existing soft Harness Pack graphs need a compatibility path rather than an
  in-place semantic rewrite.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| Mandatory session modes | Tasks change activities naturally; modes are at most UX presets. |
| A single implement-and-test loop | Cannot distinguish missing specification, poor design, environment failure, or repeated repair failure. |
| Let the LLM narrate its own state in prose | Unstable, not enforceable, and not reliably replayable. |
| Make current `GraphSpec` hard immediately | Its current semantics are deliberately soft; contracts and evidence must exist first. |

## References

- [Agentic Engineering Harness & Verification Architecture](../agentic-engineering-harness.md)
- [Harness System Vision](../harness-system.md)
- [Goal System](../goal-system.md)
- ADR 0024 — Task contracts and evidence-backed completion
- ADR 0025 — Dynamic task harnesses

