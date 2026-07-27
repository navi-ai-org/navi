# Refactoring: Subagent & Tooling Reform

## Status: Draft
## Date: 2026-07-24

## Summary

This document proposes a comprehensive reform of NAVI's subagent system, tool
exposure model, profile routing, memory extraction, tool visibility limits, and
configuration storage. The overarching goal is to make subagents simpler and
more reliable by removing fragile model-controlled options (tool lists,
approval modes, write scopes) and instead having subagents inherit all base
tools and always run in yolo mode. Tool exposure is simplified by promoting
five power tools from Deferred to Direct so the model can call them without
first discovering them via `tool_search`. The `package_manager` tool is removed
entirely as it duplicates `bash` for dependency operations.

Profile routing is dramatically simplified: instead of eight model profiles
(`cheap_general`, `cheap_code`, `repo_search`, `naming`, `long_context_cheap`,
`research_synthesis`, `simple_code_edit`, `compaction`), only three remain —
`unspecified` (default, uses the chat model), `repo_search`, and
`subagent_research`. Memory extraction changes from using a separate background
model to running in a forked session of the principal session using the main
chat model, hitting the prompt cache to minimize cost. Plugin/MCP tools are
capped at 15 visible to the model at once, with the rest discoverable via
`tool_search`. Finally, configuration moves from SQLite-backed storage back to
a minimal TOML file, keeping the provider registry SQLite as a separate,
legitimate concern.

These changes touch `navi-core` (tool definitions, metadata, subagent,
workflow, background model resolver, config types, registry store, memory
extraction, runtime), `navi-cli` (harness command hardcoded tool lists),
`navi-tui` (model picker, background models modal, keybindings), and
`navi-napi` (config serialization, background model API).

---

## 1. Tool Exposure: Deferred → Direct

### Current state

The `ToolExposure` enum is defined in
`crates/navi-core/src/tool/metadata.rs:228-242` with five variants: `Direct`
(default, visible in schema), `Deferred` (registered but hidden from schema,
discoverable via `tool_search`), `Hidden` (not visible to model, not
searchable), `ModelOnly` (visible to model only), and `Internal` (harness-only).

Five power tools are currently declared `ToolExposure::Deferred` in
`crates/navi-core/src/tool/builtin/metadata.rs`:

| Tool | Line | Current Exposure |
|------|------|-----------------|
| `browser` | 183 | `Deferred` |
| `code` | 222 | `Deferred` |
| `code_edit` | 240 | `Deferred` |
| `code_exec` | 258 | `Deferred` |
| `subagent` | 494 | `Deferred` |

The `inventory_build.rs` module
(`crates/navi-core/src/harness_pack/inventory_build.rs:20-26`) partitions
tools by exposure: `Direct` and `ModelOnly` go into the `direct` list;
`Deferred` and `Hidden` go into the `deferred` list; `Internal` is dropped.

The CLI harness command (`crates/navi-cli/src/harness_cmd.rs:15-58`) has a
hardcoded `default_tool_meta()` function with explicit `direct` and `deferred`
arrays. The five tools above are listed in the `deferred` array at lines 36-43.

The materialize module
(`crates/navi-core/src/harness_pack/materialize.rs:179-223`) has a
`default_capability_inventory()` function with hardcoded `direct` and
`deferred` tool arrays. `browser`, `code`, `code_edit`, `code_exec`, and
`subagent` appear in the deferred array at lines 201-207.

The `ToolRegistry` (`crates/navi-core/src/tool/registry.rs:76-87`) uses
exposure to determine `visible_definitions()` — only `Direct` and `ModelOnly`
tools appear in the model's tool schema. `Deferred` tools are discoverable via
`registry.search()` (line 151) but never auto-promoted into the schema.

### Required changes

Change the exposure for `browser`, `code`, `code_edit`, `code_exec`, and
`subagent` from `ToolExposure::Deferred` to `ToolExposure::Direct` in
`metadata.rs`. This makes them visible in the model schema by default without
requiring `tool_search` discovery first.

Update all hardcoded tool lists that mirror these exposure declarations:

1. **`harness_cmd.rs`** — Move `browser`, `code`, `code_edit`, `code_exec`,
   and `subagent` from the `deferred` array (lines 36-43) to the `direct`
   array (lines 16-34).

2. **`materialize.rs`** — Move the same five tools from the deferred array
   (lines 201-207) to the direct array (lines 182-199) in
   `default_capability_inventory()`.

3. **`registry.rs` tests** — The test `registry_never_auto_promotes_deferred_tools`
   (line 368) references `code` and `browser` as Deferred examples. Update the
   test to use remaining Deferred tools (e.g. `repo_explore`, `ast_search`)
   or remove `code`/`browser` from the test's Deferred set.

### Files affected

- `crates/navi-core/src/tool/builtin/metadata.rs` — lines 183, 222, 240, 258,
  494: change `Deferred` → `Direct`
- `crates/navi-cli/src/harness_cmd.rs` — lines 16-49: move 5 tools from
  `deferred` to `direct` array
- `crates/navi-core/src/harness_pack/materialize.rs` — lines 182-215: move 5
  tools from deferred to direct array in `default_capability_inventory()`
- `crates/navi-core/src/tool/registry.rs` — line 373: update test
  `registry_never_auto_promotes_deferred_tools` to use different Deferred
  examples
- `crates/navi-core/src/tool/tests.rs` — any tests that assert these tools are
  Deferred or not in visible definitions

---

## 2. Remove package_manager Tool

### Current state

The `package_manager` tool is a full builtin tool with its own directory and
metadata entry:

- **Directory**: `crates/navi-core/src/tool/builtin/package_manager/` containing:
  - `mod.rs` (184 lines) — tool struct, definition, invoke implementation
  - `check.rs` — package status checking
  - `commands.rs` — install/add/remove/update command execution
  - `finders.rs` — lockfile-based package manager detection
  - `tests.rs` — unit tests

- **Metadata**: `crates/navi-core/src/tool/builtin/metadata.rs:441-460` —
  declares `package_manager` with `ToolExposure::Deferred`, namespace
  `"package"`, risk `High`, capabilities `network.package` and
  `repo.write.lockfile`.

- **Module registration**: `crates/navi-core/src/tool/builtin/mod.rs:15` —
  `mod package_manager;` and line 49 —
  `pub(super) use package_manager::PackageManagerTool;`

- **Import in tool/mod.rs**: `crates/navi-core/src/tool/mod.rs:29` —
  `PackageManagerTool` in the `use builtin::{...}` block.

- **Registration**: The tool is registered in
  `crates/navi-core/src/tool/mod.rs` `register_builtin_tools()` (search for
  `PackageManagerTool::new`).

- **CLI harness_cmd.rs**: `crates/navi-cli/src/harness_cmd.rs:43` —
  `"package_manager"` in the `deferred` array.

- **materialize.rs**: `crates/navi-core/src/harness_pack/materialize.rs:208` —
  `"package_manager"` in the deferred array of
  `default_capability_inventory()`.

- **bash.rs**: `crates/navi-core/src/tool/builtin/bash.rs` — 16 references:
  - Line 438: description mentions `package_manager` as a suggestion target
  - Lines 553, 932-933, 948, 953-954, 960, 965, 969, 973, 977, 1032: bash
    redirects npm/bun/cargo/go package commands to the `package_manager` tool
    via `NativeSuggestion`. The functions `suggest_js_package_manager()`
    (line 960) and `package_manager_input()` (line 1032) build suggestions
    that tell the model to use `package_manager` instead of bash.

- **subagent.rs**: `crates/navi-core/src/tool/builtin/subagent.rs:111, 124` —
  `package_manager` listed in `READONLY_DENIED_TOOLS` and
  `WRITE_DENIED_TOOLS` constants.

- **registry.rs tests**: `crates/navi-core/src/tool/registry.rs:373, 381` —
  test references `package_manager` as a Deferred tool example.

- **tool/tests.rs**: `crates/navi-core/src/tool/tests.rs` — 12 references
  including `package_manager_definition_has_expected_schema` (line 1716),
  `package_manager_add_errors_without_packages` (line 1730), and assertions
  at lines 1770, 1778, 2112, 2168-2169.

- **config.rs tests**: `crates/navi-core/src/config.rs:186-197` — test uses
  `"package_manager"` in `deny_tools` example.

### Required changes

1. **Delete the entire directory**
   `crates/navi-core/src/tool/builtin/package_manager/` (all 5 files).

2. **Remove from `builtin/mod.rs`**: Delete `mod package_manager;` (line 15)
   and `pub(super) use package_manager::PackageManagerTool;` (line 49).

3. **Remove from `tool/mod.rs`**: Remove `PackageManagerTool` from the
   `use builtin::{...}` import block (line 29). Remove its registration in
   `register_builtin_tools()`.

4. **Remove from `metadata.rs`**: Delete the `package_manager` metadata entry
   (lines 441-460).

5. **Remove from `harness_cmd.rs`**: Remove `"package_manager"` from the
   `deferred` array (line 43).

6. **Remove from `materialize.rs`**: Remove `"package_manager"` from the
   deferred array (line 208).

7. **Update `bash.rs`**: Remove all `package_manager` suggestion redirects.
   The functions `suggest_js_package_manager()` (line 960) and
   `package_manager_input()` (line 1032) should be deleted. The cargo/go/npm
   command detection at lines 932-978 should either let bash execute these
   commands directly (they are safe shell commands) or return a simple
   "use bash" suggestion. Update the tool description at line 438 to remove
   the `package_manager` reference.

8. **Update `subagent.rs`**: Remove `"package_manager"` from
   `READONLY_DENIED_TOOLS` (line 111) and `WRITE_DENIED_TOOLS` (line 124).

9. **Update `registry.rs` tests**: Remove `package_manager` from test
   `registry_never_auto_promotes_deferred_tools` (lines 373, 381).

10. **Update `tool/tests.rs`**: Remove all `package_manager`-specific tests
    (lines 1713-1778) and references at lines 2112, 2168-2169.

11. **Update `config.rs` tests**: Replace `"package_manager"` with another
    tool name in the `deny_tools` test example (lines 186-197).

### Files affected (full removal list)

| File | Action |
|------|--------|
| `crates/navi-core/src/tool/builtin/package_manager/mod.rs` | Delete |
| `crates/navi-core/src/tool/builtin/package_manager/check.rs` | Delete |
| `crates/navi-core/src/tool/builtin/package_manager/commands.rs` | Delete |
| `crates/navi-core/src/tool/builtin/package_manager/finders.rs` | Delete |
| `crates/navi-core/src/tool/builtin/package_manager/tests.rs` | Delete |
| `crates/navi-core/src/tool/builtin/mod.rs` | Remove module + re-export |
| `crates/navi-core/src/tool/mod.rs` | Remove import + registration |
| `crates/navi-core/src/tool/builtin/metadata.rs` | Remove metadata entry |
| `crates/navi-cli/src/harness_cmd.rs` | Remove from deferred list |
| `crates/navi-core/src/harness_pack/materialize.rs` | Remove from deferred list |
| `crates/navi-core/src/tool/builtin/bash.rs` | Remove suggestion redirects + helper functions |
| `crates/navi-core/src/tool/builtin/subagent.rs` | Remove from denied-tool constants |
| `crates/navi-core/src/tool/registry.rs` | Update tests |
| `crates/navi-core/src/tool/tests.rs` | Remove package_manager tests |
| `crates/navi-core/src/config.rs` | Update test deny_tools example |

---

## 3. Subagent Reformulation

### Current state (option schema, tool assignment, approval)

The subagent tool is implemented in
`crates/navi-core/src/tool/builtin/subagent.rs` (1632 lines). Its option
schema is defined in the `SubagentOptions` struct (lines 66-96):

```rust
pub struct SubagentOptions {
    pub profile: Option<AgentProfile>,      // line 74 (agent_profile)
    pub model: Option<String>,               // line 77
    pub tools: Option<Vec<String>>,          // line 80
    pub approval: ApprovalMode,              // line 83
    pub max_tokens: Option<usize>,           // line 86
    pub write_allow: Option<Vec<String>>,    // line 89
    pub path_deny: Option<Vec<String>>,      // line 91
    pub create_files: Option<bool>,          // line 93
    pub create_dirs: Option<bool>,           // line 95
}
```

The JSON schema exposed to the model is in the `definition()` method
(lines 313-407). The `options` object includes all fields above plus
`agent_profile` enum values: `planner`, `explorer`, `implementer`, `reviewer`,
`security_reviewer`, `verifier`, `summarizer` (line 344).

**Tool assignment** (`resolve_allowed_tool_names`, lines 1248-1270):
- If `options.tools` is set, uses that list; otherwise uses all executor tool
  names.
- Always strips `NESTED_AGENT_TOOLS` (`subagent`, `workflow`) — line 101.
- `ReadOnly` mode strips `READONLY_DENIED_TOOLS` (lines 103-116).
- `DenyWrite` mode strips `WRITE_DENIED_TOOLS` (lines 117-127).
- Returns `Some(allowed)` always so nested tools stay filtered even for
  `Inherit`.

**Approval handling** (`resolve_approval_mode`, lines 1206-1219):
- Explicit `options.approval` wins.
- Otherwise, profile determines mode: `Explorer`/`Reviewer`/`Verifier`/
  `Summarizer`/`Planner`/`SecurityReviewer` → `ReadOnly`; `Implementer` or
  `None` → `Inherit`.
- The `ApprovalMode` enum (lines 50-63): `Inherit` (default), `Escalate`,
  `ReadOnly`, `DenyWrite`.

**Write scope** (`write_scope_from_options`, lines 1185-1200): When
`write_allow`, `path_deny`, `create_files`, or `create_dirs` are set, a
`WritePathScope` is created and the executor is forked with
`fork_with_policy_and_tools()` (lines 494-504).

**Workflow bridge** (`crates/navi-core/src/tool/builtin/workflow/backends.rs`):
The `SubagentBridgeBackend` (line 329) builds subagent input via
`build_subagent_bridge_input()` (line 453) which passes `tools`,
`approval`, `write_allow`, `path_deny`, `create_files`, `create_dirs`,
`model`, and `max_tokens` into the subagent options.

**Turn context**: The subagent creates a `TurnContext` (lines 524-562 for
foreground, 691-728 for background) with `is_subagent: true` and
`allowed_tool_names` set to the resolved list.

**NESTED_AGENT_TOOLS** in `tool/mod.rs` (line 101 of subagent.rs): `&["subagent",
"workflow"]` — always stripped from subagent tool lists to prevent recursive
spawning.

**fork_with_policy_and_tools** in `tool/mod.rs:562-578`: Always strips
`subagent` and `workflow` from worker forks (line 576).

### Required changes

**Remove these options from `SubagentOptions`:**
- `tools` (line 80) — subagents inherit ALL base tools
- `approval` (line 83) — subagents always run in yolo mode
- `max_tokens` (line 86)
- `write_allow` (line 89)
- `create_files` (line 93)
- `create_dirs` (line 95)

**Keep these options:**
- `profile` / `agent_profile` (line 74) — but simplified to new profile set
  (see §4)
- `model` (line 77)
- `path_deny` (line 91)
- `prompt`, `description`, `background`, `task_id`, `action` — top-level
  fields in the tool schema (lines 325-398)

**Tool inheritance**: Replace `resolve_allowed_tool_names()` to always return
`None` (unrestricted) after stripping only `NESTED_AGENT_TOOLS`. The subagent
gets all base tools the parent executor has, minus `subagent` and `workflow`.

**Yolo enforcement**: Instead of `resolve_approval_mode()`, always set the
subagent's security policy to yolo mode. This means:
- The `ApprovalMode` enum and `resolve_approval_mode()` function can be
  removed.
- The subagent's `TurnContext` should use a yolo-mode `SecurityPolicy` (or
  the approval resolver should auto-approve everything).
- The `build_subagent_context_static()` function (lines 908-1023) should
  remove the approval-mode-specific system prompt text and the escalate
  routing logic (lines 997-1018).
- The `READONLY_DENIED_TOOLS` and `WRITE_DENIED_TOOLS` constants become
  unnecessary.

**Write scope removal**: The `write_scope_from_options()` function and the
executor forking with `WritePathScope` (lines 494-504, 666-676) should be
removed since `write_allow`, `create_files`, and `create_dirs` are gone.
`path_deny` remains and can still be applied via `SecurityPolicy` if needed.

**Workflow bridge update**: `build_subagent_bridge_input()` in
`backends.rs` (line 453) must stop passing `tools`, `approval`,
`write_allow`, `create_files`, `create_dirs`, and `max_tokens`. The workflow
policy intersection in `policy.rs` still controls `path_deny` and `model`,
but tool restriction and approval are no longer per-agent options. The
`AgentPolicyOpts` struct (policy.rs:38-51) should have `tools`, `approval`,
`create_files`, `create_dirs`, `write_allow`, and `max_tokens` removed.
`EffectiveAgentPolicy` (policy.rs:54-64) similarly loses these fields.

**Schema update**: The `definition()` method (lines 313-407) must remove
`tools`, `approval`, `max_tokens`, `write_allow`, `create_files`, and
`create_dirs` from the `options.properties` object. The `agent_profile` enum
should be updated to the new profile set (see §4).

### Files affected

- `crates/navi-core/src/tool/builtin/subagent.rs` — major rewrite of
  `SubagentOptions`, `ApprovalMode`, `resolve_allowed_tool_names()`,
  `resolve_approval_mode()`, `write_scope_from_options()`,
  `build_subagent_context_static()`, `definition()`, `run_foreground()`,
  `spawn_background()`, and all related tests
- `crates/navi-core/src/tool/builtin/workflow/backends.rs` — update
  `build_subagent_bridge_input()`, `SubagentBridgeBackend::run_agent()`
- `crates/navi-core/src/tool/builtin/workflow/policy.rs` — simplify
  `AgentPolicyOpts`, `EffectiveAgentPolicy`, `intersect_agent_policy()`
- `crates/navi-core/src/tool/builtin/workflow/types.rs` — update
  `NESTED_WORKFLOW_TOOLS` if needed
- `crates/navi-core/src/tool/mod.rs` — `fork_with_policy_and_tools()` may
  need simplification (still strips nested tools)
- `crates/navi-core/src/tool/builtin/mod.rs` — update re-exports if
  `ApprovalMode` is removed
- `crates/navi-tui/src/dispatch.rs` — subagent event handling unchanged but
  verify no approval-mode-specific rendering
- `crates/navi-tui/src/state.rs` — `SubagentTranscript` unchanged

---

## 4. Profile Simplification

### Current profiles

Profiles are defined in two places:

1. **SQLite registry** (`crates/navi-core/src/registry/store.rs:1132-1180`):
   `seed_default_profiles()` seeds six profiles into the registry DB:
   - `cheap_general` (line 1135) — general-purpose cheap, min_context 32K,
     max_price $0.50
   - `cheap_code` (line 1142) — cheap code with tools, min_context 64K,
     max_price $1.00
   - `repo_search` (line 1149) — fast repo exploration, min_context 64K,
     max_price $0.50
   - `naming` (line 1156) — session titles, min_context 8K, max_price $0.20
   - `long_context_cheap` (line 1163) — compaction/summarization,
     min_context 128K, max_price $1.00
   - `research_synthesis` (line 1170) — research subagent with tools,
     min_context 64K, max_price $1.00

2. **Config TOML** (`crates/navi-core/src/config/types.rs:1066-1101`):
   `BackgroundModelsConfig` has fields for each task type:
   - `default` (line 1071)
   - `naming` (line 1073)
   - `memory_extraction` (line 1077)
   - `repo_search` (line 1079)
   - `compaction` (line 1081)
   - `subagent_research` (line 1083)
   - `simple_code_edit` (line 1085)

3. **BackgroundModelResolver** (`crates/navi-core/src/background_model.rs:69-77`):
   Maps task names to default profiles:
   - `"naming"` → `"naming"`
   - `"memory_extraction"` → `"cheap_general"`
   - `"repo_search"` → `"repo_search"`
   - `"compaction"` → `"long_context_cheap"`
   - `"subagent_research"` → `"research_synthesis"`
   - `"simple_code_edit"` → `"cheap_code"`
   - default → `"cheap_general"`

4. **Subagent tool schema** (`subagent.rs:335`): The `profile` field enum
   lists: `cheap_general`, `cheap_code`, `repo_search`, `naming`,
   `long_context_cheap`, `research_synthesis`.

5. **TUI**: 
   - `crates/navi-tui/src/view/modals.rs:2139-2145` — background models modal
     lists tasks: `memory_extraction`, `compaction`, `repo_search`,
     `subagent_research`, `simple_code_edit`
   - `crates/navi-tui/src/keybindings/modals.rs:2116-2122` — same list
   - `crates/navi-tui/src/view/model_picker.rs:393-400` —
     `bg_model_is_current_override()` checks task-specific config fields
   - `crates/navi-tui/src/keybindings/commands.rs:614` — references
     `compaction`

6. **NAPI** (`crates/navi-core/src/../../navi-napi/src/lib.rs:1976-1981`):
   Config serialization includes `naming`, `memoryExtraction`, `repoSearch`,
   `compaction`, `subagentResearch`, `simpleCodeEdit`.

### New profiles

Only three profiles remain:

| Profile | Description | Model Resolution |
|---------|-------------|-----------------|
| `unspecified` / `none` (default) | Uses the same model as the main chat | No profile lookup needed — uses `config.model` |
| `repo_search` | Repository exploration subagent | User config `background_models.repo_search`; if not set, falls back to current chat model |
| `subagent_research` | Research-oriented subagent | User config `background_models.subagent_research`; if not set, falls back to current chat model (configure explicitly for best results) |

**Remove**: `simple_code_edit`, `compaction`, `cheap_general`, `cheap_code`,
`naming`, `long_context_cheap`, `research_synthesis` (as profile names in the
subagent schema and registry), and the `memory_extraction` task route (see §5).

### Required changes

1. **Registry store** (`registry/store.rs:1132-1180`): Remove
   `seed_default_profiles()` entries for `cheap_general`, `cheap_code`,
   `naming`, `long_context_cheap`, `research_synthesis`. Keep `repo_search`.
   Add `subagent_research` if it should be a registry-queryable profile.

2. **Config types** (`config/types.rs:1066-1101`): Simplify
   `BackgroundModelsConfig` to:
   ```rust
   pub struct BackgroundModelsConfig {
       pub default: Option<BackgroundModelEntry>,
       pub repo_search: Option<BackgroundModelEntry>,
       pub subagent_research: Option<BackgroundModelEntry>,
   }
   ```
   Remove `naming`, `memory_extraction`, `compaction`, `simple_code_edit`.
   Update `resolve()` to only handle `repo_search` and `subagent_research`.

3. **BackgroundModelResolver** (`background_model.rs:49-75`): Simplify the
   task-to-profile mapping. Remove `naming`, `memory_extraction`,
   `compaction`, `simple_code_edit` mappings. For `repo_search` and
   `subagent_research`, resolve from user config or registry profile first,
   then fall back to the main chat model.

4. **Subagent tool schema** (`subagent.rs:335`): Change the `profile` enum
   to `["repo_search", "subagent_research"]`. Omitting `profile` means
   `unspecified` — uses the chat model.

5. **TUI modals** (`view/modals.rs:2139-2145`,
   `keybindings/modals.rs:2116-2122`): Update the background models task list
   to only show `repo_search` and `subagent_research`. Remove
   `memory_extraction`, `compaction`, `simple_code_edit` rows.

6. **TUI model_picker.rs** (`view/model_picker.rs:393-400`): Update
   `bg_model_is_current_override()` to only check `repo_search` and
   `subagent_research`.

7. **NAPI** (`navi-napi/src/lib.rs:1974-1982`): Update config serialization
   to only include `repoSearch` and `subagentResearch` in
   `backgroundModels`. Remove `naming`, `memoryExtraction`, `compaction`,
   `simpleCodeEdit`.

8. **Runtime** (`runtime/mod.rs:2143-2145`): The `compaction_model_name` and
   `compaction_provider` fields in `TurnContext` may become unused if
   compaction always uses the main model. Clean up these fields.

9. **Session/turn references**: Search for all remaining references to
   removed profile names and update or remove them.

### Files affected

- `crates/navi-core/src/registry/store.rs` — lines 1132-1180
- `crates/navi-core/src/config/types.rs` — lines 1066-1101
- `crates/navi-core/src/background_model.rs` — lines 49-84, 209-218
- `crates/navi-core/src/tool/builtin/subagent.rs` — line 335 (profile enum)
- `crates/navi-tui/src/view/modals.rs` — lines 2139-2145
- `crates/navi-tui/src/keybindings/modals.rs` — lines 2116-2122
- `crates/navi-tui/src/keybindings/commands.rs` — line 614
- `crates/navi-tui/src/view/model_picker.rs` — lines 393-400
- `crates/navi-tui/src/view/input.rs` — line 1743
- `crates/navi-tui/src/dispatch.rs` — line 1269
- `crates/navi-tui/src/app.rs` — line 273
- `crates/navi-napi/src/lib.rs` — lines 1278, 1976-1981
- `crates/navi-core/src/runtime/mod.rs` — lines 2143-2145
- `crates/navi-core/src/turn/mod.rs` — lines 80-87 (compaction fields)
- `crates/navi-core/src/session.rs` — lines 802, 1086-1088
- `crates/navi-core/src/compact.rs` — compaction model references
- `crates/navi-sdk/src/engine.rs` — lines 177-179
- `crates/navi-sdk/src/routing_ops.rs` — lines 19-22, 46, 123-124, 153-157,
  181-185
- `crates/navi-core/src/runtime_components.rs` — lines 20, 30, 176
- `crates/navi-core/src/runtime/session_state.rs` — line 187
- `crates/navi-core/src/event.rs` — lines 63, 173-182, 191, 415, 605-614, 623
- `crates/navi-lite/src/lib.rs` — line 114

---

## 5. Memory Extraction → Forked Main Session

### Current state

Memory extraction is implemented in
`crates/navi-core/src/memory/extract.rs` (287 lines). The
`extract_memories()` function (line 49) takes a `conversation` string, a
`model_provider`, a `model_name`, and an `AutoMemoryStore`. It sends a single
model request with a specialized system prompt (`EXTRACT_SYSTEM`, line 15)
and user payload (`EXTRACT_USER`, line 26) to extract durable memories as a
JSON array.

**Model selection**: Memory extraction uses a **separate, dedicated model**
configured by the user. The runtime field `memory_extraction_model`
(`crates/navi-core/src/runtime/mod.rs:326, 379`) is an
`Option<MemoryExtractionModel>` (line 335) containing a provider and model
name. This is opt-in: if not configured, extraction is **disabled** (line
1902-1905) — it never falls back to the chat model to avoid invisible credit
consumption.

**Trigger**: `try_extract_memories()` (line 1899) is called after each
completed turn. It spawns a fire-and-forget tokio task (line 1927) that calls
`extract_memories()`.

**Background model resolver**: The `BackgroundModelResolver`
(`background_model.rs:71`) maps `"memory_extraction"` task to the
`"cheap_general"` profile. The config field
`background_models.memory_extraction` (`config/types.rs:1077`) allows explicit
override.

**TUI**: The background models modal lists `memory_extraction` as a
configurable task (`view/modals.rs:2140`, `keybindings/modals.rs:2117`). The
setup wizard has a `MemoryModel` phase (`keybindings/modals.rs:2127-2132`)
that requires choosing a memory extraction model.

### Required changes

The new design: memory extraction runs using the **main chat model** in a
**forked session** of the principal session (to hit the prompt cache,
minimizing cost). No separate model for memory extraction.

1. **Remove `memory_extraction_model` from runtime**: Delete the
   `MemoryExtractionModel` struct (line 335) and the
   `memory_extraction_model` field from `AgentRuntime` (line 379) and
   `AgentRuntimeOptions` (line 326).

2. **Change `try_extract_memories()`**: Instead of using a separate provider,
   fork the principal session's messages and use the main chat model
   (`self.shared_model_provider` / `self.shared_model_name`). The forked
   session should reuse the existing conversation messages so the prompt
   cache is hit. The extraction prompt is appended as a new user message.

3. **Remove opt-in gate**: Memory extraction should always run (when memory
   is enabled) using the chat model, not be gated on a separate model being
   configured. Remove the "no memory extraction model configured" early
   return (lines 1902-1905).

4. **Remove from config**: Remove `memory_extraction` field from
   `BackgroundModelsConfig` (`config/types.rs:1077`).

5. **Remove from BackgroundModelResolver**: Remove the `"memory_extraction"`
   task mapping (`background_model.rs:71`).

6. **Remove from TUI**: Remove `memory_extraction` from the background models
   task list (`view/modals.rs:2140`, `keybindings/modals.rs:2117`). Remove
   the `MemoryModel` setup phase
   (`keybindings/modals.rs:2127-2132`).

7. **Remove from NAPI**: Remove `memoryExtraction` from config serialization
   (`navi-napi/src/lib.rs:1977`). Remove `set_background_model` support for
   `memory_extraction` task (line 1278).

8. **Update extract.rs**: The `extract_memories()` function signature stays
   the same (it already takes a provider and model name), but the caller
   now passes the main chat model. Consider adding a forked-session variant
   that takes the existing message history plus the extraction prompt, so
   the prompt cache is hit.

### Files affected

- `crates/navi-core/src/runtime/mod.rs` — lines 324-338, 379, 1896-1946
- `crates/navi-core/src/memory/extract.rs` — potentially add forked-session
  variant
- `crates/navi-core/src/config/types.rs` — line 1077 (remove
  `memory_extraction` field)
- `crates/navi-core/src/background_model.rs` — line 71 (remove
  `memory_extraction` mapping)
- `crates/navi-tui/src/view/modals.rs` — line 2140
- `crates/navi-tui/src/keybindings/modals.rs` — lines 2117, 2127-2132
- `crates/navi-tui/src/view/model_picker.rs` — line 395
- `crates/navi-tui/src/view/setup.rs` — line 52
- `crates/navi-tui/src/providers.rs` — lines 209, 217
- `crates/navi-tui/src/state.rs` — line 436
- `crates/navi-napi/src/lib.rs` — lines 1278, 1977
- `crates/navi-sdk/src/engine.rs` — lines 420-421, 569, 622-651
- `crates/navi-sdk/src/routing_ops.rs` — lines 18, 46, 123, 153, 181
- `crates/navi-core/src/runtime/tests.rs` — lines 158, 227, 344, 403, 448, 526
- `crates/navi-core/src/memory/tests.rs` — lines 757, 759
- `docs/auto-memory.md` — documentation update

---

## 6. Tool Visibility Limits (base unlimited, plugin/MCP capped at 15)

### Current state

The tool schema sent to the model is built by `ToolExecutor::definitions()`
(`crates/navi-core/src/tool/mod.rs:364-390`). This method:
1. Gets `visible_tool_names()` from the `ToolRegistry`
   (`registry.rs:90-95`) — only `Direct` and `ModelOnly` tools.
2. Filters live tools by whether their name is in the visible set.
3. Applies `model_friendly_definition()` to simplify schemas.
4. Merges enriched metadata from the registry.
5. Sorts by name for prefix-cache stability.

There is **no limit** on the number of tools visible to the model. All
`Direct`/`ModelOnly` tools appear in the schema regardless of count.

**MCP tools** are loaded in `navi-sdk/src/engine.rs:438-448`:
`load_configured_mcp_servers()` returns `LoadedMcpServers` with a `tools` Vec
of `Arc<dyn Tool>`. Each tool is registered via `executor.register_tool()`.
MCP tool names are prefixed with the server ID (e.g. `mcp__memory__get`).

**Plugin tools** (WASM) are registered similarly via
`executor.register_tool()` during engine setup and plugin reload
(`engine.rs:1770`).

**`tool_search`** is implemented as the `ToolSearchTool` in
`crates/navi-core/src/tool/builtin/extra_tools.rs`. It calls
`ToolExecutor::search_tools()` which delegates to
`ToolRegistry::search()` (`registry.rs:151-186`) — a BM25-inspired keyword
search across name, description, tags, capabilities, and examples.

**Deferred tools** are already excluded from the schema but remain
searchable. The `MCP_TOOL_DEFER_THRESHOLD` constant (`registry.rs:31`) is set
to 100 but is described as "historical" and not actively used for
auto-promotion.

The turn layer in `turn/mod.rs:324, 531` calls
`ctx.tool_executor.definitions()` to get the tool list, then applies
`harness.filter_tools()` and `allowed_tool_names` filtering.

### Required changes

Implement a 15-tool cap for plugin/MCP tools while keeping navi base tools
unlimited:

1. **Distinguish base tools from plugin/MCP tools**: Add a method to
   `ToolRegistry` or `ToolExecutor` that partitions visible tools into
   "base" (builtin navi tools) and "external" (plugin/MCP). MCP tools can be
   identified by the `mcp__` prefix; plugin tools by the `plugin__` prefix.

2. **Cap external tools at 15**: In `ToolExecutor::definitions()` (or a new
   method), after collecting all visible tools:
   - Include all base tools (unlimited).
   - Include at most 15 plugin/MCP tools in the schema. Select which 15 by
     some deterministic priority (e.g. alphabetical, or most-recently-used,
     or configured priority).
   - The remaining plugin/MCP tools stay registered (callable by name) and
     searchable via `tool_search`, but are not in the model's schema.

3. **Expose overflow via tool_search**: The `ToolSearchTool` already searches
   all registered tools (including Deferred and non-visible). Ensure that
   plugin/MCP tools beyond the 15-cap are still searchable. The model can
   discover them via `tool_search` and then call them by name.

4. **Config option**: Consider adding a config setting
   (e.g. `tui.max_visible_plugin_tools = 15`) to make the cap configurable,
   defaulting to 15.

5. **Turn layer**: The turn layer (`turn/mod.rs:324, 531`) calls
   `definitions()` which would now apply the cap automatically. No change
   needed if the cap is in `definitions()`.

### Files affected

- `crates/navi-core/src/tool/mod.rs` — `definitions()` method (lines 364-390):
  add base/external partitioning and 15-tool cap
- `crates/navi-core/src/tool/registry.rs` — potentially add helper methods
  for base/external classification
- `crates/navi-core/src/tool/builtin/extra_tools.rs` — verify `tool_search`
  covers capped tools (line 416 area)
- `crates/navi-core/src/config/types.rs` — optional new config field for cap
- `crates/navi-sdk/src/engine.rs` — verify MCP/plugin tool registration still
  works with the cap (lines 438-448, 1770)

---

## 7. Config Migration: SQLite → TOML

### Current state (what's in SQLite vs TOML)

**TOML config** (`~/.config/navi/config.toml` and `.navi/config.toml`):
The `NaviConfig` struct (`config/types.rs:13-72`) is entirely TOML-based.
Loading is in `config/persistence.rs:10-47` — reads global then project
TOML, merges. Saving is in `save_global_config()` (line 117) and
`save_project_config()` (line 131). The config includes: model, harness,
approvals, security, logging, providers, plugins, memory, voice, skills,
mcp, wasm_plugins, plugin_marketplace, registry, tui, background_models,
goals, updates, browser, acp, acp_agents, workflow.

**SQLite registry** (`<data_dir>/registry.db`):
The `RegistryStore` (`registry/store.rs:43-45`) is a SQLite database storing
**provider and model metadata** — not user configuration. It contains:
- Providers (id, label, kind, api_key_env, base_url, models, etc.)
- Model capabilities, pricing, profiles
- Model-profile associations for routing
- Transcription providers

The registry is populated from an embedded snapshot at build time
(`registry.lock` + `build.rs`) and can be updated via `navi registry sync`.
This is a **separate concern** from user config — it's a catalog/cache of
provider metadata, not user preferences.

**Other SQLite databases** (not config):
- `memory/auto_memory.rs` — auto-memory store (`memories.db`)
- `memory/history_store.rs` — conversation history
- `memory/global_memory.rs` — global memory store
- `session.rs` — session persistence
- `skills/store.rs` — legacy skill store migration

**Assessment**: The user's concern about "config in SQLite" likely refers to
the fact that the registry SQLite stores provider/model metadata that could
arguably be in TOML, and that `BackgroundModelsConfig` profiles are resolved
through the SQLite registry rather than being purely TOML-driven. The actual
`NaviConfig` is already TOML. The registry SQLite is a legitimate cache for
provider catalog data (hundreds of models with pricing, context windows,
capabilities) that would be unwieldy in TOML.

### Required changes

1. **Keep the registry SQLite** for provider/model catalog data — this is a
   cache, not user config. It holds 142+ models with pricing, context windows,
   and capability tags. Moving this to TOML would create a massive,
   hard-to-maintain file.

2. **Move profile definitions out of SQLite**: Currently
   `seed_default_profiles()` (`registry/store.rs:1132`) seeds profile
   definitions (cheap_general, cheap_code, etc.) into SQLite. With the
   profile simplification (§4), only `repo_search` and `subagent_research`
   remain. These can be hardcoded constants or minimal TOML entries rather
   than SQLite rows. Remove `seed_default_profiles()` and define profiles as
   constants.

3. **Simplify `BackgroundModelsConfig`**: The TOML config for background
   models should be minimal — just `repo_search` and `subagent_research`
   entries with optional provider+model overrides. No profile-based registry
   queries needed for the simplified profile set.

4. **Minimal TOML schema**: The config TOML should only include settings
   that users actually need to customize. Many current fields have sensible
   defaults and rarely need explicit config. The minimal user-facing TOML
   should be:

### Draft TOML schema

```toml
# ~/.config/navi/config.toml — minimal user config

[model]
provider = "openai"        # required: provider ID from registry
name = "gpt-4.1"           # required: model name

[security]
permission_mode = "accept-edits"  # restricted | accept-edits | auto | yolo

[background_models]
# Optional: override models for specific subagent profiles.
# Omit to use the main chat model.
repo_search = { provider = "openai", model = "gpt-4.1-mini" }
subagent_research = { provider = "anthropic", model = "claude-sonnet-4-20250514" }

[memory]
enabled = true

[skills]
enabled = true

# ── Optional sections (omit to use defaults) ──

[mcp]
enabled = true

[[mcp.servers]]
id = "memory"
command = "memory-mcp-server"
args = ["--stdio"]
enabled = true

[tui]
theme = "oscura-night"
yolo_mode = false

[workflow]
enabled = true
max_parallel = 16
```

Settings that stay internal/defaulted (not in minimal TOML unless overridden):
- `harness` (profile, loop limits, compaction thresholds)
- `logging` (level, file settings)
- `providers` (mostly from registry; only custom overrides)
- `plugins`, `wasm_plugins` (managed via `navi plugin install`)
- `plugin_marketplace`
- `registry` (sync settings)
- `goals` (enabled, max turns)
- `updates` (check interval)
- `browser` (engine config)
- `acp`, `acp_agents`
- `attachment_models`
- `voice`

### Files affected

- `crates/navi-core/src/registry/store.rs` — remove `seed_default_profiles()`
  (lines 1132-1180), remove profile-related queries if profiles become
  constants
- `crates/navi-core/src/config/types.rs` — simplify
  `BackgroundModelsConfig` (lines 1066-1101)
- `crates/navi-core/src/background_model.rs` — simplify resolver to not
  query SQLite for profiles; use config entries directly
- `crates/navi-core/src/config/persistence.rs` — no structural change but
  ensure minimal TOML round-trips correctly
- `crates/navi-core/src/config/defaults.rs` — verify defaults are sensible
  for omitted sections
- `crates/navi-napi/src/lib.rs` — update config serialization (lines
  1974-1982)

---

## Migration & Rollout

### Ordering and dependencies

1. **Phase 1 — Remove `package_manager` (§2)**: Independent change. No
   dependencies on other phases. Remove all code, update bash.rs suggestions,
   fix tests.

2. **Phase 2 — Tool exposure changes (§1)**: Independent of §2 but should be
   done after §2 since `package_manager` is in the deferred list. Change
   metadata, update hardcoded lists, fix tests.

3. **Phase 3 — Profile simplification (§4)**: Depends on §2 (package_manager
   removal affects denied-tool lists). Remove profiles from registry, config,
   resolver, TUI, NAPI. This is a large cross-cutting change.

4. **Phase 4 — Subagent reformulation (§3)**: Depends on §3 (profiles
   simplified) and §2 (package_manager removed from denied-tool lists).
   Major rewrite of subagent options, approval, tool inheritance.

5. **Phase 5 — Memory extraction change (§5)**: Depends on §3 (profiles
   removed) and §4 (subagent forked session pattern established). Change
   extraction to use forked main session.

6. **Phase 6 — Tool visibility limits (§6)**: Independent of §3-5 but should
   be done after §1 (exposure changes) so the base/external partition is
   stable. Add 15-tool cap for plugin/MCP tools.

7. **Phase 7 — Config migration (§7)**: Depends on §3 (profiles simplified)
   and §5 (memory extraction config removed). Finalize minimal TOML schema,
   remove SQLite profile seeding.

### Risk areas

- **Subagent yolo mode**: Running subagents in yolo mode means no approval
  gates. Ensure the security policy still enforces path jailing and blocked
  commands. The `SecurityPolicy` itself remains active; only the approval
  flow is bypassed.

- **Workflow bridge**: The `SubagentBridgeBackend` and workflow policy
  intersection (`policy.rs`) are tightly coupled to the current options
  (`tools`, `approval`, `write_allow`). Removing these requires careful
  rethinking of how workflow workers are constrained. Workflow workers may
  still need `path_deny` and model overrides.

- **Prompt cache for memory extraction**: Forking the principal session for
  memory extraction requires the forked messages to share the same system
  prompt prefix as the principal session. The current subagent creates a
  fresh `PromptCache` (line 547) — the memory extraction fork should instead
  reuse the principal session's cache or at least share the prefix.

- **NAPI compatibility**: The NAPI bindings expose `BackgroundModelsConfig`
  fields and `set_background_model` API. Removing fields is a breaking change
  for Node.js/Electron clients. Consider deprecation warnings or keeping
  no-op fields for one release.

- **TUI setup wizard**: The `MemoryModel` setup phase
  (`keybindings/modals.rs:2127`) must be removed or replaced. If memory
  extraction no longer needs a separate model, the setup wizard should skip
  this step.

- **Registry profile queries**: `query_models_by_profile()` in the registry
  store is used by `BackgroundModelResolver`. If profiles are removed from
  SQLite, this query path becomes dead code. Ensure no other callers depend
  on it.

---

## Open Questions

1. **Workflow worker constraints**: With `tools`, `approval`, `write_allow`,
   `create_files`, and `create_dirs` removed from subagent options, how should
   workflow workers be constrained? Should the workflow policy intersection
   (`policy.rs`) still enforce `path_deny` and model selection, or should
   workflow workers also inherit all tools and run in yolo? The workflow
   system's `RunPolicy` and `AgentPolicyOpts` need clarification on what
   remains.

2. **`path_deny` semantics**: `path_deny` is kept as a subagent option. Should
   it still create a `WritePathScope` with only `path_deny` set (no
   `write_allow`), or should it be applied differently in yolo mode?

3. **Memory extraction fork depth**: When forking the principal session for
   memory extraction, should the fork include the full conversation history or
   just the latest turn? Full history hits cache better but costs more input
   tokens. Just the latest turn is cheaper but may miss context.

4. **15-tool cap selection**: When more than 15 plugin/MCP tools exist, which
   15 are shown? Options: alphabetical first 15, most-recently-used, user
   configured priority list, or configured in TOML. Need user preference.

5. **Registry SQLite scope**: Confirm that the "config in SQLite" concern
   refers specifically to profile definitions in the registry (which should
   move to constants/TOML) and not the provider/model catalog (which should
   stay in SQLite as a cache). The provider catalog with 142+ models and
   pricing data is not practical in TOML.

6. **`naming` profile removal**: Session title generation currently uses the
   `naming` profile/background model. If removed, title generation should use
   the main chat model. Is this acceptable, or should a lightweight
   `naming` route remain?

7. **Compaction model**: Compaction currently can use a separate
   `compaction` background model. With profile simplification, should
   compaction always use the main chat model? The turn layer
   (`turn/mod.rs:447-449`) already uses the session's own model for
   auto-compaction. Confirm this is the desired behavior.

8. **NAPI breaking changes**: Is a clean break acceptable for the NAPI
   bindings, or should deprecated fields be kept as no-ops for backward
   compatibility?
