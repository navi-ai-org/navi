# ADR 0024 — Task Contracts and Evidence-Backed Completion

## Status

Proposed

## Context

NAVI's current `SessionGoal` models a persistent objective, budget,
auto-continuation, and an optional checklist. The model can currently update a
goal to `Complete`; checklist verification is informational rather than a
runtime completion gate. This is appropriate for the existing goal feature but
insufficient for a code-agent runtime that must distinguish an agent's claim of
completion from evidence that the environment supports it.

Open engineering requests need more structure than a single objective, but
requiring users to write a complete formal specification would defeat the
purpose of an agent. The agent needs a task-local, serializable representation
of its inferred requirements, assumptions, acceptance criteria, and evidence
policy.

## Decision

### 1. Introduce a task contract separate from `SessionGoal`

`TaskContract` is a session-scoped engine type. It may reference a `GoalId`,
but it does not replace the goal system.

It records:

- objective, requirements, constraints, and acceptance criteria;
- the provenance of each claim: user-provided, repository-derived, or agent
  inference;
- material open questions and recorded assumptions;
- required evidence, verifier policy, and completion state; and
- links to the dynamic task harness and evidence produced for the task.

Task contracts are persisted in NAVI private session storage under `{data_dir}`
and exposed through stable, serializable SDK/NAPI/CLI/TUI surfaces as needed.
They are never auto-created in a project `.navi/` directory.

### 2. Completion is an engine decision when a contract governs the task

The model may propose contract content and request that a verifier run. The
runtime alone executes verifiers, writes verifier outcomes, and evaluates the
completion contract.

When an active goal has a governing task contract, `update_goal(complete)` and
equivalent host APIs must be rejected unless required current evidence satisfies
the contract. The rejection returns structured, model-readable missing or stale
evidence so the agent can continue work.

### 3. Evidence is structured and becomes stale after relevant mutation

Evidence has status (`passed`, `failed`, `inconclusive`, or `stale`), trust
level, provenance, scope, diagnostics, verifier identity, and a workspace
fingerprint. A mutation that can affect an acceptance claim invalidates its
evidence. A past passing test is not completion evidence for a modified tree.

The initial trust taxonomy is:

| Level | Source |
| --- | --- |
| L0 | Model assertion |
| L1 | Heuristic or LLM evaluator |
| L2 | Static analysis |
| L3 | Executable test or integration check |
| L4 | External deterministic oracle |
| L5 | Formal verifier/kernel |

Trust level alone is not enough: evidence also reports whether it was supplied
by the existing project, the user, the agent, or an external oracle. This avoids
presenting self-authored tests as independent proof of unstated intent.

### 4. A mutating task always receives a completion audit

A read-only answer may finish without a task contract. Once a task changes
project state, the engine creates a minimal implicit contract and requires a
completion audit. The model may extend it with behaviour, security, integration,
or architectural claims; it cannot bypass it by responding "done".

### 5. Surface and event parity is mandatory

Task-contract updates, evidence updates, completion denials, and completion
results are versioned engine events. Any user-visible or host-visible capability
must be wired through `navi-core` → `navi-sdk` → `navi-napi` → `navi-cli` (when
user-facing) → `navi-tui` (when visible), including session replay and redaction.

## Consequences

### Positive

- Vague user intent can become explicit, inspectable, evidence-backed work.
- The model may choose its strategy while the runtime retains authority over
  completion.
- Freshness and provenance make completion claims more honest and debuggable.
- Goals retain their existing long-horizon purpose instead of becoming an
  unbounded configuration language.

### Negative

- More persisted session state and events must be versioned and replayed.
- Some work has no complete automated oracle; reports must represent partial or
  inconclusive evidence rather than manufacture certainty.
- Completion-gate changes affect model tools and every embedding surface.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| Prompt the model to run tests before finishing | The model can ignore the prompt or misinterpret weak evidence. |
| Put all contract fields directly on `SessionGoal` | Couples long-horizon scheduling to task formalization and makes the goal API brittle. |
| Let the model call `goal.mark_verified` | Circular authority: the agent would verify its own claim. |
| Require users to author complete specifications | Breaks the informal workflow that makes code agents useful. |

## References

- [Agentic Engineering Harness & Verification Architecture](../agentic-engineering-harness.md)
- [Goal System](../goal-system.md)
- [Harness System Vision](../harness-system.md)
- ADR 0025 — Dynamic task harnesses
- ADR 0026 — Task lifecycle state machine

