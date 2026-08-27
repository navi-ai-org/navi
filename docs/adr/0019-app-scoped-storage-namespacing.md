# ADR 0019 — App-Scoped Storage Namespacing in `{data_dir}`

## Status
Proposed

## Context

ADR 0018 defines the first-party application boundary and depends on an
`app.storage.namespace` that does not exist. This ADR supplies it.

`{data_dir}` is today **flat and engine-scoped**. `NaviEngineBuilder::data_dir()`
sets it once per engine; nothing below it is scoped per session or per consumer.
The actual inventory:

| Path | Contents | Source |
|---|---|---|
| `credentials.toml` | provider API keys, OAuth accounts | credential store |
| registry SQLite cache | provider/model catalog | `navi-core/src/registry/` |
| `sessions/` | session transcripts | `session.rs:320` |
| `plans/` | plan-mode markdown | `plan_store.rs:25` |
| `memory/` | `memories.db`, `global-memory.db`, `models/` (embeddings), `{hash}.json` | `memory/mod.rs:74`, `session.rs:540` |
| `harnesses/` | harness packs | `harness_pack/store.rs:13` |
| `skills/` | user skills | `skills/store.rs:41` |
| `plugins/` | installed WASM packages | `plugin_cmd.rs:217` |
| `sprints/{project_hash}/` | long-running harness artifacts | `long_running.rs:344` |
| `voice/` | ASR models | `navi-voice/src/paths.rs:7` |
| `browser/` | headless browser data | `browser_cmd.rs:73` |
| `traces/` | trajectory traces | `trace.rs:361` |
| `logs/` | `navi.log` | `logging.rs:48` |

### The consequence: two bad options and nothing in between

An application embedding or driving NAVI today can only choose between:

**Share everything** (use the user's `data_dir`). The app's sessions appear in the
user's TUI history; the app's harness pack appears in `navi harness list`; the
app's memories are injected into the coding agent's context and vice versa; the
app's config changes overwrite the user's settings.

**Share nothing** (set a private `data_dir`). The user must re-authenticate every
provider for every app, each app re-downloads the provider registry, and voice/ASR
models are duplicated per app at hundreds of megabytes.

Neither is acceptable. The distinction that matters is not per-app vs per-user but
**who owns the artifact**: credentials and model catalogs belong to the *user*;
sessions, memory and harness belong to the *application*.

Precedent for sub-scoping already exists: `sprints/{project_hash}/` and
`memory/{hash}.json` are keyed by project. This ADR generalises that idea to apps.

## Decision

### 1. App scope

Introduce `{data_dir}/apps/<app-id>/` as an application's private root. Each
artifact class is classified as **shared** (user-owned) or **scoped** (app-owned).

| Artifact | Scope | Rationale |
|---|---|---|
| `credentials.toml` | **shared** | The keys are the user's property. Per-app re-authentication is hostile and would multiply secret copies on disk. |
| registry cache | **shared** | Pure cache with no app-specific content; expensive to rebuild. |
| `voice/`, `browser/` | **shared** | Large binary assets with no app-specific semantics. |
| `sessions/` | **scoped** | An app's transcripts must not appear in the TUI session picker, and vice versa. |
| `plans/` | **scoped** | Bound 1:1 to sessions. |
| `memory/` | **scoped** | Load-bearing: auto-memory is *injected into the prompt*. Mod-install memories reaching the coding agent is a correctness and privacy defect, not untidiness. |
| `harnesses/` | **scoped**, with shared read | The app's pack must not appear in the user's `navi harness list`; the app may still read user-authored packs by id. |
| `skills/` | **scoped**, with shared read | Same reasoning. |
| `sprints/`, `traces/` | **scoped** | Derived from app sessions. |
| `plugins/` | **shared install, scoped activation** | A WASM package is a user-installed artifact; installing it N times per app wastes disk and fragments the trust/lockfile model (ADR 0010). Which plugins are *active* is app-scoped. |
| `logs/` | **shared file, app-tagged records** | One log is materially easier to support. Records carry an app id field. |

When no app scope is active, **paths are unchanged**. The user's TUI keeps writing
to `{data_dir}/sessions/`, not `{data_dir}/apps/tui/sessions/`. This avoids
migrating existing installations: app scoping is purely additive.

### 2. App id is untrusted input

`app-id` arrives from an application manifest and becomes a **path component**. It
is therefore validated before use:

- matches `^[a-z0-9][a-z0-9._-]{0,63}$`
- rejected if it contains a path separator, is `.` or `..`, or normalises outside
  `{data_dir}/apps/`
- the resolved path is canonicalised and re-checked against the `apps/` prefix
  (defence against symlink escape)

A failed validation is a hard error at `initialize`, never a fallback to the shared
root — silently falling back would leak app state into the user's scope.

### 3. Path jail

`{data_dir}/apps/` is NAVI-private storage. The existing rule that denies tool
writes to `{data_dir}/plugins` (`security.rs:328`, `security.rs:712`) is extended:

- an app's tools may not read or write **another** app's scope
- an app's tools may not write their own scope directly — engine APIs mediate it
- the shared root (`credentials.toml`, registry cache) is denied to all tools

This keeps the guarantee that an agent cannot exfiltrate credentials or corrupt a
sibling application by path manipulation.

### 4. Lifecycle

- **Create** on first `initialize` with a namespace, lazily per artifact class.
- **Uninstall**: `navi app remove <id>` deletes `{data_dir}/apps/<id>/` only.
  Credentials, registry cache and installed plugins are untouched.
- **Inspect**: `navi app list` reports id, size on disk, session count, last use.
- No implicit cross-app migration. Moving state between apps is an explicit,
  user-initiated operation or nothing.

### 5. Scope resolution is engine-level, not session-level

The namespace is resolved once at `initialize` from the app profile and is
immutable for the connection's lifetime. Sessions do **not** carry their own
namespace.

Rationale: `data_dir` is already engine-scoped, and making it per-session would
require threading a scope through every store (`SessionStore`, `AutoMemoryStore`,
`PlanStore`, `HarnessPackStore`) and every path helper. Connection-level scoping
gets the isolation property with a single resolution point.

**Consequence for multi-tenant SaaS:** one connection per tenant, therefore one
engine instance per tenant. That is a real cost and is accepted here rather than
solved: full per-session storage scoping is a larger refactor and is deferred
until a SaaS deployment proves it necessary. This is a known limitation, not an
oversight.

## Consequences

Positive:
- Unblocks ADR 0018 §5
- The user's TUI is unaffected by any number of installed apps
- Credentials are authenticated once and shared, so apps need no auth UX of their own
- Memory isolation is a correctness fix, not a nicety — cross-domain memory
  injection is a real prompt-pollution bug today
- Uninstall is a directory delete with no orphaned credentials
- Purely additive: no migration for existing installations
- Extends the existing NAVI-private-storage deny rules rather than inventing a new
  security concept

Negative:
- Embedding models and memory databases are duplicated per app (tens to hundreds of
  MB each) — the price of isolation
- Legitimate cross-app memory sharing becomes impossible. A user might reasonably
  want NAVI Learning to know things learned during TUI use; that would need an
  explicit opt-in mechanism, out of scope here
- Every path helper gains a scope parameter — broad, mechanical, error-prone churn
  across `navi-core`
- Two path regimes coexist (scoped and unscoped) for the lifetime of the product,
  since the TUI is never migrated
- One engine per tenant for SaaS until per-session scoping is done (§5)
- Shared plugin install with scoped activation means the activation state is a new
  thing to store and reason about

## Alternatives rejected

| Alternative | Why rejected |
|---|---|
| Separate `data_dir` per app | Forces per-app re-authentication and duplicates registry and ASR models; the reason the current design is unusable |
| Fully shared `data_dir` | Session, memory and harness pollution across apps; memory injection across domains is a correctness bug |
| Migrate the TUI into `apps/tui/` for symmetry | Symmetry is not worth migrating every existing installation; unscoped is a valid default |
| Per-session storage scope | Correct end state and better for SaaS, but requires threading scope through every store; deferred (§5) |
| Namespace by process/PID | Not durable; sessions must survive restarts |
| Share memory, scope everything else | Memory is exactly the artifact where cross-contamination causes wrong model behaviour |

## References
- ADR 0018 — App boundary; §5 depends on this namespace
- ADR 0010 — Plugin Lockfile & Approval (why plugins stay a shared install)
- ADR 0006 — Security Defaults Are Mandatory (deny rules are defaults, not options)
- `navi-core/src/security.rs:328,712` — existing NAVI-private-storage deny precedent
- `navi-core/src/tool/builtin/long_running.rs:344`, `navi-core/src/session.rs:540` —
  existing per-project hashing precedent
- `AGENTS.md` non-negotiable #3 — all state under `{data_dir}`, never project-local
