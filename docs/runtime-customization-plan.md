# NAVI Runtime Customization Plan

**Status:** Implemented for core/SDK/NAPI; frontend-specific extensions remain scoped
**Owner:** NAVI core/runtime
**Last updated:** 2026-06-28

This plan tracks the work needed to move NAVI from an opinionated code-agent
framework into a composable agent runtime toolkit. The target is a runtime that
can be assembled by downstream clients such as NAVI TUI, NAVI Tutor,
headless/ACP clients, and future SDK users with custom security, tools,
harness behavior, prompts, compaction, hooks, and UI integration.

The goal is not to remove NAVI's default code-agent behavior. The goal is to
make that behavior the default composition of replaceable runtime components.

## Problem Statement

The current plugin system exposes extension points that are not fully wired:

- `PluginRegistry::register_tool` works and registers executable tools.
- `PluginRegistry::register_agent_policy` records names. The SDK consumes known
  runtime policy names and reports unknown names as warnings.
- `PluginRegistry::register_tui_component` records names but no TUI layer
  consumes them.

At the same time, major runtime decisions are still concrete in `navi-core`:

- security is represented by the concrete `SecurityPolicy` type;
- `ToolExecutor` owns policy validation through that concrete type;
- the turn loop calls concrete harness functions;
- system prompt rendering is selected by concrete prompt code;
- compaction is called directly from the turn loop;
- lifecycle hooks are not a first-class runtime concept.

This makes plugins useful for tools, but not yet sufficient for building a
custom runtime such as `navi-learning`.

## Design Target

NAVI should expose a small set of stable component traits and compose them
through the SDK:

```rust
pub struct RuntimeComponents {
    pub security: Arc<dyn ToolSecurityPolicy>,
    pub harness: Arc<dyn HarnessDriver>,
    pub prompt: Arc<dyn PromptBuilder>,
    pub compaction: Arc<dyn CompactionStrategy>,
    pub hooks: Arc<dyn SessionHooks>,
}
```

Default components must preserve the current code-agent behavior. Custom
components can replace any stage for other products.

For `navi-learning`, "no security policy" should be modeled as an explicit
permissive implementation:

```rust
pub struct PermissiveSecurityPolicy;

impl ToolSecurityPolicy for PermissiveSecurityPolicy {
    fn validate_tool(
        &self,
        _invocation: &ToolInvocation,
        _definition: &ToolDefinition,
    ) -> SecurityDecision {
        SecurityDecision::Allow
    }
}
```

This keeps the runtime pipeline uniform while allowing full tutor autonomy.

## Non-Goals

- Do not make NAVI Tutor depend on `navi-tui`.
- Do not make WebSocket or daemon mode the primary integration surface.
- Do not remove native or WASM plugin support.
- Do not make project config able to enable unsafe native plugin roots.
- Do not break existing TUI, CLI, headless, ACP, SDK, MCP, or plugin-tool flows.

## Phase 0: Inventory And Compatibility Contract

**Status:** Completed

### Tasks

- Map all concrete runtime decisions in `navi-core`.
- List every place that constructs `SecurityPolicy`, `ToolExecutor`,
  `HarnessPolicy`, `SystemPromptRenderer`, and `CompactState`.
- Record current event behavior that must remain stable for TUI and SDK users.
- Add parity tests or snapshots around default runtime behavior before
  refactoring.

### Acceptance Criteria

- A compatibility checklist exists for default runtime behavior.
- Current plugin limitations are documented as explicit gaps.
- The default path remains the reference behavior for later phases.

### Suggested Validation

- `just test-crate navi-core`
- `just test-crate navi-sdk`

## Phase 1: Runtime Component Traits

**Status:** Completed

### Tasks

- Introduce `ToolSecurityPolicy` as a trait.
- Rename or wrap the current concrete policy as `DefaultSecurityPolicy`.
- Add `PermissiveSecurityPolicy` for fully autonomous host-controlled runtimes.
- Introduce `HarnessDriver` for tool-loop decisions, tool filtering, and
  observation formatting.
- Introduce `PromptBuilder` for system prompt construction.
- Introduce `CompactionStrategy` for micro-compact and auto-compact behavior.
- Introduce `SessionHooks` for lifecycle and analytics callbacks.
- Add `RuntimeComponents::default_for_config(...)` to preserve current behavior.

### Acceptance Criteria

- Existing code-agent behavior is provided by default component
  implementations.
- Custom components can be created in Rust tests without changing global config.
- The traits use serializable, UI-agnostic inputs where practical.
- Trait names do not conflict with existing concrete type names.

### Suggested Validation

- `just test-crate navi-core`
- Focused tests for each default component.

## Phase 2: Wire Components Into AgentRuntime

**Status:** Completed

### Tasks

- Add `runtime_components: RuntimeComponents` to `AgentRuntimeOptions`.
- Store components in `AgentRuntime` and pass them into `TurnContext`.
- Replace direct prompt construction in the turn loop with `PromptBuilder`.
- Replace direct compaction calls with `CompactionStrategy`.
- Replace direct harness function calls with `HarnessDriver` where behavior is
  meant to be customizable.
- Ensure subagent and repo-explore tools inherit the same runtime component
  choices where appropriate.
- Keep `HarnessPolicy` as data/config, not as the customization boundary.

### Acceptance Criteria

- `AgentRuntime::new` can build the current runtime without callers specifying
  components.
- SDK and TUI sessions still use identical defaults.
- Tests can start a runtime with permissive security and a custom prompt
  builder.
- Runtime events remain stable.

### Suggested Validation

- `just test-crate navi-core`
- `just test-crate navi-sdk`
- `just test-crate navi-tui` if TUI session setup changes.

## Phase 3: SDK Builder Surface

**Status:** Completed

### Tasks

- Add component setters to `NaviEngineBuilder`.
- Allow host tools and runtime components to be configured together.
- Define what is engine-scoped versus session-scoped.
- Add examples to `docs/sdk-agents.md`.
- Make it clear that custom tools still go through the selected security
  component.

### Target API Sketch

```rust
let engine = NaviEngineBuilder::from_project(".")
    .security(Arc::new(PermissiveSecurityPolicy))
    .harness(Arc::new(LearningHarness::default()))
    .prompt(Arc::new(TutorPromptBuilder::new("pt-BR")))
    .compaction(Arc::new(StudyCompaction::default()))
    .hooks(Arc::new(StudyHooks::new(analytics)))
    .host_tool(Arc::new(material_lookup_tool))
    .build()?;
```

### Acceptance Criteria

- SDK users can assemble a custom runtime without depending on TUI internals.
- Existing `NaviEngineBuilder::from_project(".").build()` behavior is
  unchanged.
- Host tools, MCP tools, WASM plugins, and native plugin tools all share the
  configured runtime policy.

### Suggested Validation

- `just test-crate navi-sdk`
- SDK integration test with custom prompt, permissive security, and host tool.

## Phase 4: Plugin Registry Wiring

**Status:** Partially completed

### Tasks

- [x] Replace placeholder `register_agent_policy` behavior with a real component
  registration path.
- [x] Decide whether native plugins register concrete components directly or
  register named factories.
- [x] Keep plugin ABI compatibility strategy explicit because trait objects across
  native plugin boundaries are fragile.
- [x] Define a real TUI extension boundary before wiring
  `register_tui_component`.
- [x] Keep TUI plugins scoped to `navi-tui`; do not move ratatui concepts into
  `navi-sdk` or `navi-core`.

### Acceptance Criteria

- Plugin reports distinguish loaded tools, loaded runtime components, and
  declared-but-unsupported capabilities.
- No placeholder registry fields are silently ignored.
- Native and WASM plugin extension limits are documented.

### Suggested Validation

- `just test-crate navi-plugin-api`
- `just test-crate navi-plugin-host`
- `just test-crate navi-sdk`

### Current State

- Native plugin tools still register normally.
- `register_agent_policy` is consumed by the SDK for known named policies:
  `learning_tutor`, `navi_learning`, `tutor`, `default`, and `code_agent`.
- Unknown agent policy names are reported as warnings.
- `register_tui_component` declarations are preserved by the SDK and exposed
  per session through `list_tui_components`; actual terminal widget rendering
  remains a `navi-tui` frontend responsibility.
- Native plugins register named policies rather than cross-library Rust trait
  objects, preserving ABI boundaries.

## Phase 5: NAPI / TypeScript Host Integration

**Status:** Partial

### Tasks

- [x] Add a NAPI wrapper crate only after the Rust SDK customization surface is
  stable.
- [x] Expose host tools as TypeScript callbacks or classes.
- [x] Expose runtime events as a stream or async iterator.
- [x] Expose safe component options first: permissive security, prompt templates,
  tool filtering, hooks, and compaction rules.
- [x] Decide which components need full TypeScript callback implementations versus
  structured configuration objects.

### Target TypeScript Sketch

```ts
export function createTutorEngine(workspace: string) {
  return new AgentRuntimeBuilder()
    .security(new PermissiveSecurity())
    .harness(new LearningHarness({ maxConsecutiveErrors: 5 }))
    .prompt(new TutorPromptBuilder({ language: "pt-BR", style: "socratic" }))
    .compaction(new StudyCompaction({ keepAllAssessments: true }))
    .hooks(new StudyHooks({ onToolCall: analytics.trackToolCall }))
    .hostTool(new ConsultarMateriais(materialDb))
    .hostTool(new Questionario(questionBank))
    .build(workspace);
}
```

### Acceptance Criteria

- [x] `navi-learning` can run without native `.so` or `.dylib` plugins.
- [x] TypeScript host tools can be added without modifying `navi-core`.
- [x] The web client does not depend on terminal UI internals.
- [x] The Rust SDK remains the source of truth for runtime behavior.

### Current State

- `crates/navi-napi` exposes `NaviNapiEngineBuilder`.
- `NaviNapiEngineBuilder::set_learning_tutor(true)` enables the learning
  runtime preset from the Rust SDK.
- `NaviNapiEngineBuilder::configureLearning(...)` maps TypeScript options to
  `LearningHarnessConfig`, `TutorPromptOptions`, and `StudyCompactionConfig`.
- `NaviNapiEngineBuilder::hostTool(...)` registers TypeScript async callbacks
  as SDK host tools.
- `NaviNapiEngineBuilder::onSessionStart/onTurnStart/onToolCall/onToolResult/onTurnEnd/onSessionEnd`
  register fire-and-forget TypeScript lifecycle hooks backed by
  `SessionHooks`.
- Host tool callbacks receive `{ invocationId, input }` and return a promise
  resolving to `{ ok, output }`.
- Session lifecycle methods expose start, send turn, snapshot, and close.
- `NaviNapiEngine::subscribeEvents(sessionId)` returns an event stream object
  with async `next()` over serialized `RuntimeEvent` payloads.
- The NAPI engine exposes stable runtime controls: cancel turn, resolve
  approval, add context packet, list models, list TUI component declarations,
  and set model.
- TypeScript component strategy is split deliberately: host tools and
  lifecycle hooks use callbacks, while security/harness/prompt/compaction use
  structured options that map to Rust component implementations.

### Suggested Validation

- NAPI unit tests for host tool result parsing, tool kind mapping, runtime
  event serialization, and structured learning runtime options.
- End-to-end tutor session smoke test.

## Phase 6: NAVI Learning Runtime

**Status:** Core runtime implemented; domain tools are host-provided

### Tasks

- [x] Implement `LearningHarness`.
- [x] Implement `TutorPromptBuilder`.
- [x] Implement `StudyCompaction`.
- [x] Define study-domain host tool boundary; concrete product tools are
  provided by `navi-learning` through NAPI `hostTool`.
- [ ] Implement study-domain host tools in the learning product:
  - material lookup;
  - image generation;
  - quiz generation;
  - canvas diagram operations;
  - grading rubric;
  - scheduler/cron;
  - assessment history;
  - student progress;
  - lesson planning;
  - content recommendation.
- [ ] Implement analytics and persistence hooks in the learning product.
- [x] Define which tools are exempt from compaction.

### Acceptance Criteria

- Tutor runtime can guide a learning session autonomously.
- Study tools are TypeScript-hosted or otherwise web-compatible.
- Assessments and learning state are never lost to generic code-agent
  compaction.
- The agent can operate with permissive security inside the host-controlled
  learning environment.

### Current State

- `learning_runtime_components()` builds the default learning composition.
- `NaviEngineBuilder::learning_tutor()` exposes that composition to SDK users.
- `LearningHarness` tolerates repeated study tools and raises consecutive
  error tolerance.
- `TutorPromptBuilder` switches the system prompt to tutor behavior.
- `StudyCompactionStrategy` preserves quiz, rubric, assessment, schedule, and
  progress tool results during micro-compaction.
- Domain-specific tools remain host-provided through `host_tool`; the core does
  not ship product-specific study tools.

### Suggested Validation

- Tutor smoke flow: start session, consult material, ask quiz, grade answer,
  update progress.
- Regression test that study assessment events survive compaction.

## Migration Rules

- Keep default runtime behavior stable at every phase.
- Add new component seams before moving behavior behind them.
- Prefer adapter layers over large rewrites.
- Do not expose TUI concepts through `navi-sdk`.
- Do not let plugin placeholders remain silently successful once real component
  support exists.
- Treat permissive security as explicit host intent, never as a default.

## Progress Log

Update this section during each implementation step.

| Date | Phase | Status | Notes |
|---|---|---|---|
| 2026-06-28 | Planning | Created | Initial plan for runtime component customization. |
| 2026-06-28 | 0 | Completed | Inventory confirmed concrete security, harness, prompt, compaction, and plugin placeholder call sites. |
| 2026-06-28 | 1 | Completed | Added `RuntimeComponents`, `ToolSecurityPolicy`, `HarnessDriver`, `PromptBuilder`, `CompactionStrategy`, `SessionHooks`, defaults, and `PermissiveSecurityPolicy`. |
| 2026-06-28 | 2 | Completed | Wired components through `AgentRuntime`, `TurnContext`, prompt rendering, tool filtering, compaction, tool observations, subagents, and repo-explore. |
| 2026-06-28 | 3 | Completed | Added SDK builder setters for security, harness, prompt, compaction, hooks, and full runtime components. |
| 2026-06-28 | 4 | Partial | Native plugin `register_agent_policy` declarations are consumed by the SDK for known runtime policies (`learning_tutor`, `navi_learning`, `tutor`, `default`, `code_agent`). TUI component declarations remain frontend-scoped and are reported as warnings until `navi-tui` owns a TUI plugin registry. |
| 2026-06-28 | 6 | Completed | Added learning runtime preset with permissive security, learning harness, tutor prompt builder, study compaction, and SDK `.learning_tutor()`. |
| 2026-06-28 | 5 | Partial | Added `navi-napi` crate, TypeScript engine builder, structured learning runtime options, session/control methods, async TypeScript host-tool callbacks, and event stream `next()` support. Full TypeScript component callback strategy remains pending. |
