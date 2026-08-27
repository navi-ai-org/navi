# Agentic Engineering Harness & Verification — Implementation Plan

**Status:** Proposed implementation plan  
**Audience:** `navi-core` owners and embedding-surface maintainers  
**Related:** [Architecture RFC](../agentic-engineering-harness.md) · [Harness System Vision](../harness-system.md) · [Goal System](../goal-system.md) · ADR 0024 · ADR 0025 · ADR 0026

## Goal

Evolve NAVI from a tool-using LLM loop into an evidence-backed task runtime:

```text
informal request → task contract → execute → verify → evidence
                 → repair / redesign / ask → completion audit
```

The scope is deliberately incremental. Do not add a generic hook language, a
new daemon, project-local state, or a user-facing mandatory mode system.

## Architectural constraints

- Runtime semantics, security, evidence, and completion gates live in
  `navi-core`; `navi-sdk` exposes stable serializable APIs; NAPI/CLI/TUI follow
  whenever the capability reaches their users.
- All durable task state is session-private under `{data_dir}`. Never create or
  mutate project `.navi/` bookkeeping.
- Dynamic harnesses may narrow permissions but never widen the active tool,
  approval, path, plugin, MCP, or network policy.
- Plugins remain WASM-only. MCP remains client-only; a task contract cannot
  silently enable either.
- Every new tool and callable OS/backend surface follows the repository's unit,
  adversarial edge-case, and skip-safe integration test requirements.
- Existing goals, Harness Packs, and soft graphs remain compatible throughout
  the migration.

## Baseline gap analysis

| Existing foundation | Location | Gap to close |
| --- | --- | --- |
| Tool-loop safeguards | `navi-core/src/harness.rs` | No task lifecycle, evidence state, or completion gate |
| Goals and auto-continue | `navi-core/src/goal/` | Model may complete a goal without machine-enforced contract evidence |
| Executable command runner | `navi-core/src/verifier.rs` | Result is command-centric; lacks canonical identity, evidence provenance, trust, and freshness |
| Descriptive pack verifier | `navi-core/src/harness_pack/types.rs` | A second `VerifierSpec`; recipes do not execute through a shared model |
| Pack application | `navi-core/src/harness_pack/apply.rs` | Applies loop caps and entry-node tool policy only; no hard edge execution |
| Runtime events / SDK | `navi-core/src/event.rs`, `navi-sdk` | No task-contract/evidence/state-transition event family |
| Compaction | `navi-core/src/compact.rs` | No task-anchor refresh boundary |

## Phase 0 — Freeze the contract and measure the baseline

**Objective:** produce an implementation-ready gap analysis before changing
runtime behaviour.

1. Review the RFC and ADRs with engine, SDK, and client owners; accept, amend,
   or reject each proposed ADR explicitly.
2. Trace current completion paths: model `update_goal(complete)`, host goal APIs,
   headless termination, and session replay.
3. Inventory `VerifierSpec` consumers and pack recipe users; identify migration
   compatibility requirements.
4. Add representative eval fixtures without changing product behaviour:
   read-only explanation, small code edit, failing test repair, vague feature,
   environment failure, and an intentionally ambiguous architectural request.
5. Define measurable baselines: false completion, unnecessary clarification,
   verifier execution cost, repeated identical repairs, and user-visible latency.

**Exit:** signed-off ADR statuses, migration inventory, and baseline eval corpus.

## Phase 1 — Task contract model and intake record

**Objective:** make task interpretation serializable and inspectable without
changing completion enforcement yet.

1. Add a `task_contract` module in `navi-core` with types for objective,
   requirement, constraint, acceptance criterion, assumption, open question,
   provenance, and contract revision.
2. Add session-owned `TaskRun` state that optionally references `GoalId`; retain
   `SessionGoal` as the budget/auto-continue abstraction.
3. Define host APIs and model proposal operations. The model may create/refine
   claims and propose verifiers, but cannot write evidence status.
4. Persist and replay task state with sessions; add redaction rules for embedded
   diagnostics and user data.
5. Add versioned core events (`task.contract_updated`, initially) and SDK
   projection. Do not expose a TUI workflow until the event semantics settle.

**Tests:** serde backward compatibility, malformed/empty/unicode contract input,
large contracts, replay round-trips, concurrency around simultaneous host/model
updates, and redaction tests.

**Exit:** a headless/SDK caller can create, revise, snapshot, and replay a task
contract; no completion behaviour changes yet.

## Phase 2 — Canonical verifier and evidence model

**Objective:** remove competing verifier semantics and produce bounded,
attributed evidence.

1. Design `VerificationSpec`, `VerificationResult`, and `EvidenceRecord` with
   stable IDs, status, trust, provenance, scope, diagnostics, timeout, working
   directory policy, and workspace fingerprint.
2. Adapt the current command `VerifierRunner` as the first backend; preserve its
   failure classification and bounded output behaviour.
3. Convert Harness Pack recipes through adapters to the canonical model. Keep
   existing pack files loadable; write a documented migration only when a stable
   replacement is ready.
4. Add a verifier registry for built-in, command-template, browser, and WASM
   plugin-backed verifier kinds. Do not execute arbitrary model-generated hook
   code.
5. Emit `verification.started` and `verification.completed` with safe, redacted
   evidence summaries.

**Tests:** existing `verifier.rs` tests retained; schema compatibility; invalid
specs; timeout; command-not-found; output truncation; Windows shell/path cases;
secret redaction; fingerprint stability; pack adapter round trips.

**Exit:** command verification and existing Harness Pack verifier recipes share
one execution/result/evidence path.

## Phase 3 — Completion contracts and stale-evidence gate

**Objective:** make verified completion an engine decision.

1. Add `CompletionContract` that maps task claims to required evidence policy.
2. Track evidence freshness. Invalidate affected evidence after writes, tool
   commands with side effects, or explicit workspace-state changes; prefer a
   conservative invalidation policy initially.
3. Intercept all completion paths. When a governing contract is unsatisfied,
   reject model and host completion with structured missing, failed, or stale
   evidence; keep the task/goal active.
4. Add a completion-audit operation that schedules required verifiers and emits
   `task.completion_denied` or `task.completed_with_evidence`.
5. Surface the contract/evidence state through SDK and NAPI; add CLI/TUI status
   only after their consumers have the stable event schema.

**Tests:** passing contract, missing evidence, stale after edit, inconclusive
evidence, mixed required/optional claims, model completion denial, host API
denial, replay, cancel during verification, and capability parity tests.

**Exit:** a task with code mutations cannot become `Complete` solely because the
model says it is done.

## Phase 4 — Dynamic task harness proposal and validation

**Objective:** let the agent turn an open task into a constrained verification
plan.

1. Define the declarative `DynamicTaskHarness`: claim bindings, verifier refs,
   triggers, evidence policy, and bounded recovery hints.
2. Implement a validator that rejects unknown verifier IDs, unsupported claim
   coverage, untrusted command shapes, permission widening, plugin/MCP
   activation, and invalid state transitions.
3. Require model-proposed command verifiers to use registered templates or the
   normal `run` approval/security path.
4. Persist approved dynamic harnesses alongside their task contract; emit
   proposal, rejection, and activation events.
5. Extend intake prompts/tool descriptions so the model can propose a harness
   when it has enough repository evidence, without a fixed questionnaire.

**Tests:** malicious/vacuous verifier proposals, path and tool escalation,
unknown plugin/MCP references, unicode/large schemas, deterministic validator
behaviour, and real command-template integration tests.

**Exit:** an agent can propose build/test/HTTP/security evidence for a new task,
but NAVI is the only component that validates and runs it.

## Phase 5 — Runtime task state machine

**Objective:** execute task lifecycle transitions rather than relying on prose
or current soft graph edges.

1. Add engine-owned task states: `intake`, `specify`, `design`, `implement`,
   `verify`, `repair`, `evaluate`, `ask_user`, `complete`, plus bounded
   terminal failure/budget states.
2. Define valid transitions and idempotent transition events. Start with one
   linear default loop; retain `GraphSpec` as a separate soft-pack feature.
3. Route verifier outcomes into `repair`, `investigate`, `ask_user`, or
   `redesign` based on deterministic result classification and bounded history.
4. Add optional per-state tool narrowing only after the transition engine works;
   it must intersect with existing security and active-harness policy.
5. Version, persist, and replay state transitions through core, SDK, NAPI, CLI,
   and TUI where task progress is presented.

**Tests:** full transition matrix, invalid transition recovery hints, repeated
identical failure threshold, budget/cancel/approval interactions, session
resume, concurrent steering, and end-to-end SDK event streams.

**Exit:** `implement → verify → repair → verify → complete` works without the
model narrating lifecycle state as the source of truth.

## Phase 6 — Evaluator abstraction

**Objective:** add optional quality feedback without confusing it with hard
verification.

1. Define evaluator inputs/results separately from verifiers, including
   heuristic score, rationale, scope, and provenance.
2. Allow built-in/static evaluators first; make LLM evaluators explicitly L1
   evidence and never sufficient for a hard requirement by default.
3. Route evaluator outcomes to accept, repair, redesign, or user review without
   overriding passed hard evidence.

**Tests:** evaluator never upgrades trust, evaluator unavailable/inconclusive,
result redaction, and state-routing policy.

**Exit:** NAVI can report quality feedback separately from correctness evidence.

## Phase 7 — Failure taxonomy and recovery policy

**Objective:** prevent endless patch loops and make escalation observable.

1. Add failure categories: implementation, design, specification, environment,
   verification, and budget.
2. Combine deterministic verifier classification, repeated evidence signatures,
   contract conflicts, and runtime failures into recovery policy.
3. Add bounded escalation: same failure N times → inspect/rethink; material
   unresolved ambiguity → ask user; environment failure → report environment,
   not code failure.
4. Record the chosen route and rationale as task events and evidence context.

**Tests:** equivalent failure normalization, threshold boundaries, false-positive
avoidance, recovery prompt formation, and user-question escalation.

**Exit:** repeated failure results in a meaningful state change, not merely a
new patch attempt.

## Phase 8 — Semantic instruction refresh and model-specific experiments

**Objective:** reinforce task constraints without turning the system prompt into
an ever-growing workflow manual.

1. Build a compact task anchor from current contract, state, constraints,
   evidence, and next required transition.
2. Inject it at task creation, compaction completion, session resume, verifier
   failure, and state transition.
3. Add optional model-profile token-distance refresh behind an experiment flag;
   do not hardcode a universal context threshold.
4. Evaluate anchored versus baseline behaviour on the Phase 0 corpus, including
   instruction retention, verification rate, token cost, and unnecessary questions.

**Tests:** anchor redaction, bounded token size, compaction/replay insertion
order, disabled feature behaviour, and provider/model config parsing.

**Exit:** instruction reinforcement improves measured task outcomes without
regressing normal conversations or leaking private diagnostics.

## Phase 9 — Domain verifier compilation and reusable harnesses

**Objective:** make successful task patterns reusable without requiring one
prewritten harness per domain.

1. Add built-in verifier templates for common project patterns only after
   observing real dynamic-harness demand.
2. Allow users to promote reviewed dynamic harnesses into reusable Harness Packs
   under `{data_dir}/harnesses/`; never auto-promote them.
3. Add WASM verifier registration through the existing plugin broker and
   approval path.
4. Consider hard execution of selected pack graphs only after the task state
   machine has reliable evidence and transition semantics.

**Exit:** reviewed task evidence strategies can become reusable safely, while
one-off work remains task-local.

## End-to-end demonstration

The release gate is a fixture-level task equivalent to:

> Create an HTTP server that shares a selected local directory.

The observed trace must show:

1. repository inspection and recorded assumptions;
2. clarification only for material boundary choices;
3. a contract with behaviour and traversal-security claims;
4. an approved dynamic harness;
5. a build/test/integration failure with actionable evidence;
6. repair or redesign;
7. fresh passing evidence for server start, listing/download, 404, and traversal
   protection; and
8. completion accepted by the runtime, not simply asserted by the model.

Additional fixtures must cover a read-only request, an environment failure, a
formal-prover task, and a deliberately subjective UX request that completes with
explicitly partial/human-required evidence.

## Verification and rollout

Each phase ships independently with package-scoped checks first:

```powershell
cargo fmt --all -- --check
cargo check -p navi-core
cargo test -p navi-core -- --test-threads=4
```

Run SDK/NAPI/CLI/TUI tests whenever their public API or event projection changes.
Add targeted eval fixtures before enabling a new lifecycle gate by default.
Feature-gate behaviour that would reject existing goal completion until migration
and host API compatibility are verified. Do not lower critical-coverage floors.

## Explicit non-goals

- Implementing all phases in one change.
- Letting model-authored shell/Python hooks bypass engine security.
- Replacing current Harness Packs before their replacement is executable.
- Treating an LLM evaluator as a correctness oracle.
- Requiring a user-selected work mode or a full manual specification.

