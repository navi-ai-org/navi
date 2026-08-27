# NAVI — Agentic Engineering Harness & Verification Architecture

**Status:** Proposal  
**Audience:** engine, SDK, CLI, NAPI, and UI maintainers  
**Related:** [Harness System Vision](harness-system.md) · [Goal System](goal-system.md) · [Task Contracts ADR](adr/0024-task-contracts-and-evidence-backed-completion.md) · [Dynamic Harness ADR](adr/0025-dynamic-task-harnesses.md) · [Task Lifecycle ADR](adr/0026-task-lifecycle-state-machine.md)

## Objective

NAVI is an agent runtime, not a prompt wrapper. Its job is to make a
probabilistic model useful for real engineering by moving facts that the
environment can answer out of the model's self-assessment.

> Never trust the agent when the environment can answer.

The model remains free to interpret a request, explore a repository, select an
implementation strategy, and recover from failures. The runtime owns the
control loop: policy, state, budgets, verification, evidence, and completion.

This applies equally to narrowly specified work and informal "vibe coding".
The user should be able to ask for a feature in natural language; NAVI should
turn that request into progressively explicit, evidence-backed work without
requiring the user to write a formal specification first.

## Authority model

The LLM is a probabilistic problem solver, not the authoritative verifier of
its own output.

| Question | Preferred authority |
| --- | --- |
| Does the code compile? | Compiler / type checker |
| Do declared tests pass? | Test runner |
| Does an endpoint behave as claimed? | Integration or HTTP verifier |
| Does a proof hold? | Proof-assistant kernel |
| Does the dependency graph obey a rule? | Structural verifier |
| Is the solution maintainable or idiomatic? | Static analysis, evaluator, then human judgment |
| Did subjective product intent land? | Acceptance criteria and, where needed, human judgment |

The ordering is deliberate: hard, repeatable evidence is stronger than a model
assessment. An LLM evaluator is useful for heuristic quality feedback but is
never an unconditional completion oracle.

## Components

```text
Goal (optional, long-running objective)
        │
        ▼
Task contract ──► Dynamic task harness ──► Verifiers ──► Evidence
        │                    │                  │             │
        └────────────────────┴── Harness runtime ┴─────────────┘
                                      │
                                      ▼
                              LLM + tools + environment
```

### LLM

The LLM interprets intent, investigates project context, proposes requirements,
selects tools, writes code, proposes a verification strategy, and reacts to
evidence. It may propose a task harness, but it cannot manufacture a passing
verification result or unilaterally complete a governed task.

### Tools and environment

Tools are interfaces to the environment: filesystem, shell, source tree,
compiler, runtime, browser, Git, installed WASM plugins, and connected MCP
servers. Security, approvals, path jails, private-storage restrictions, and
plugin/MCP policy remain runtime-enforced regardless of task state.

### Goal

A goal remains NAVI's persistent long-horizon objective, budget, and
auto-continuation mechanism. It is not overloaded into the task-contract DSL.
A task may be associated with a goal, and a governed goal may only transition
to `Complete` after its task contract is satisfied.

### Harness

The harness is the runtime control layer. It owns lifecycle transitions,
budgets, permitted actions, verifier scheduling, evidence freshness, recovery,
and completion. Prompt text can guide reasoning; it cannot be the sole
enforcement mechanism where runtime enforcement is possible.

### Verifier and evaluator

A verifier answers a concrete question about the environment and returns
structured evidence. It should normally be deterministic and repeatable:
compiler, test suite, type checker, HTTP probe, formal prover, dependency scan,
or a registered WASM-backed verifier.

An evaluator answers a different question: how good is a working solution?
It may assess architecture, API design, documentation, maintainability, or UX.
It can use an LLM, but its output is heuristic evidence and cannot replace a
required hard verifier.

## Task intake and task contract

For work that changes the environment, the agent first performs task intake:

```text
request → inspect context → infer requirements → assess ambiguity
        → state assumptions → propose evidence → execute
```

The repository is part of the specification. The agent must inspect relevant
architecture, code, tests, dependencies, and project instructions before asking
the user to choose an implementation detail that the repository already
answers.

Ambiguity alone is not a clarification request. The clarification gate weighs:

```text
uncertainty × impact × irreversibility
```

Low-impact, reversible decisions can be assumed and recorded. A high-impact
choice about public protocol, security boundary, persistence, or architecture
requires either project evidence or a user decision.

A task contract records the model's working interpretation in a serializable
form:

```text
objective
requirements and constraints
acceptance criteria
assumptions and their provenance (user, repository, or agent inference)
open material questions
required evidence and verifier policy
budget and completion state
```

The contract can be refined as the agent learns more. Refinement must preserve
the provenance of requirements so an inferred convenience does not silently
replace a user requirement.

## Evidence-backed completion

Every task that mutates code or other project state enters a completion audit.
The model can decide how rich its task-specific verification should be, but it
cannot skip the audit merely by replying "done".

Read-only answers may finish without a task contract. Mutating work gets a
minimal implicit contract; the model may enrich it with behavioural, security,
or integration claims.

Evidence must be current. A write that can affect a claim invalidates prior
evidence for that claim until the applicable verifier runs again.

```text
implement → verify → evidence current?
                 │
        fail ────┘
                 ▼
               repair

all required claims satisfied → completion audit → complete
```

Evidence carries more than a Boolean result:

```text
status: passed | failed | inconclusive | stale
trust: model_assertion | heuristic | static | executable | external_oracle | formal
provenance: existing_project | agent_created | user_provided | external
scope: build | behaviour | security | architecture | performance | other
diagnostics: summary, locations, bounded output, suggested recovery
workspace fingerprint: the state for which it is valid
```

An agent-created test is useful executable evidence, but it is not independent
proof of unstated user intent. Completion reports must expose the covered claims,
assumptions, evidence provenance, and remaining subjective or inconclusive
areas.

## Dynamic, ephemeral task harnesses

The agent may construct a harness appropriate to the task. For example, a file
sharing server can derive build, HTTP behaviour, 404, and path-traversal claims
without a prewritten server-specific harness pack.

Dynamic does not mean arbitrary code execution. The agent proposes a
declarative specification; NAVI validates it and executes only supported,
policy-compliant verifier kinds. A task harness may never:

- grant tools, paths, approvals, network access, MCP servers, or plugins beyond
  the current session policy;
- mark its own verifier as passed;
- weaken explicit user or repository constraints without recording and resolving
  the conflict;
- use a vacuous command as proof of an unrelated claim.

Task-local dynamic harnesses are stored with session state under `{data_dir}`.
They complement, rather than replace, reusable Harness Packs under
`{data_dir}/harnesses/`.

## Task lifecycle

The lifecycle is a general state machine, not a set of mandatory user-selected
modes:

```text
INTAKE → SPECIFY → DESIGN → IMPLEMENT → VERIFY → EVALUATE → COMPLETE
                       ▲         │          │         │
                       │         │          ├─ fail → REPAIR ─┘
                       │         │          └─ inconclusive → ASK_USER / INVESTIGATE
                       └─ redesign ────────────────────────────┘
```

`EVALUATE` is optional; hard verification is not. The state machine governs
lifecycle, allowed transitions, evidence, recovery, and budgets. It does not
dictate a proof strategy, framework, file layout, or algorithm to the model.

Failures are classified before routing recovery:

- implementation failure;
- design failure;
- specification failure;
- environment failure;
- verification failure; or
- budget failure.

Repeated materially identical verification failures must not blindly produce
more patches. They are a signal to inspect assumptions, redesign, or ask a
material clarification.

## Instruction refresh

NAVI keeps a short task anchor available at semantic boundaries: task start,
post-compaction, session resume, verifier failure, and state transition. It
summarizes the objective, constraints, active claims, evidence status, and the
rule that self-assessment is not verification.

Instruction refresh is not tied to a universal context-window number. Models
differ, and compaction already preserves the system prompt. Any token-based
refresh threshold is model-configurable and must be evaluated before becoming a
default.

## Current foundations and gaps

| Foundation | Current state | Required evolution |
| --- | --- | --- |
| `harness.rs` | Tool-loop policy, limits, repeated-error handling | Own task lifecycle and completion gates |
| Goals | Objective, auto-continue, budgets, checklist | Associate contracts; block governed completion without evidence |
| `verifier.rs` | Command runner, result, classifier, store | Canonical verifier model and evidence/provenance/freshness |
| Harness Packs | Loop, soft graph, descriptive verifier recipes | Reuse canonical verifier spec; keep reusable packs distinct from task-local harnesses |
| `GraphSpec` | Soft entry-node tool policy | Optional runtime-executed task state graph after the contract path works |
| Events / SDK | Serializable engine event surface | Versioned task, evidence, and state-transition events across every client surface |

There are currently two incompatible `VerifierSpec` types: the executable
command spec in `navi-core::verifier` and the descriptive recipe in
`navi-core::harness_pack`. They must be consolidated before dynamic harnesses
are introduced.

## Non-goals

- A giant system prompt or mandatory questionnaire for every request.
- Requiring a handwritten domain harness for every task.
- Treating LLM evaluation as formal correctness.
- Allowing arbitrary Python, shell hooks, or unmediated plugin code as a task
  harness extension mechanism.
- A WebSocket daemon as NAVI's primary interface.
- Replacing the existing goal system or putting runtime behaviour in the TUI.

## Implementation

Implementation is intentionally incremental. The [implementation plan](plans/agentic-engineering-harness.md)
starts with a gap analysis and canonical verifier/evidence model, then adds
completion enforcement, dynamic task harnesses, the state machine, evaluators,
recovery, and instruction refresh. Each phase must have a bounded demo and
tests before the next one starts.

