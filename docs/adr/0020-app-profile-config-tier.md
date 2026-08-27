# ADR 0020 — App Profile as a Third Config Tier, and the App Bundle Format

## Status
Proposed

## Context

ADR 0018 sends an app profile over `initialize`. ADR 0019 gives it a storage
namespace. Neither says where the profile *comes from*, what it is *allowed to
change*, or how it relates to the two config tiers that already exist.

### Today's two tiers

Load order is `defaults → ~/.config/navi/config.toml → .navi/config.toml`.
Project config may override `model`, `harness`, `approvals`, `security`, `skills`
and providers, but per `AGENTS.md` it **cannot** enable `plugins`, `wasm_plugins`
or `mcp` — those keys are ignored with a warning.

That restriction is correct and must stay. Project config is trusted **because of
location**, which is the weakest possible basis: anyone can hand you a repository,
and `cd` is not consent. A hostile `.navi/config.toml` must not be able to turn on
capability.

### Why that blocks applications

A first-party application must ship its own domain tools. The sanctioned
no-rebuild paths for that are MCP servers and WASM plugins — precisely the three
keys project config may not touch. And `AGENTS.md` non-negotiable #3 forbids
auto-creating `.navi/` anyway, so an application cannot write project config even
if it were allowed to.

So an application today has no legitimate way to declare its own capability. The
missing piece is not a loophole in the project tier; it is a tier with a **different
trust basis**.

### The distinction that licenses a new tier

| Tier | Trust basis | Strength |
|---|---|---|
| Global user config | the user wrote it | strong |
| **App profile** | **the user installed this application** | **strong** |
| Project config | the current directory contains it | weak |

Installation is an explicit act of consent about a named, versioned artifact.
That is categorically different from a directory happening to contain a file, and
it is what justifies granting powers the project tier is denied.

## Decision

### 1. A third config tier

Load order becomes:

```
defaults → global user config → app profile → project config
```

with per-key precedence rules rather than blanket last-wins (§3).

### 2. App bundle format

An application is a directory. Nothing in it is compiled against NAVI.

```
organize-anything/
  app.toml                    # the profile (below)
  harness/                    # loop.toml, graph.toml, CAPABILITY.md, SKILL.md
  skills/                     # markdown skills
  mcp/organize-tools(.exe)    # domain tools, any language
  plugins/*.wasm              # optional sandboxed tools
  bin/navi(.exe)              # optional bundled engine (ADR 0021)
  ui/                         # frontend, opaque to NAVI
```

```toml
# app.toml
[app]
id = "organize-anything"            # validated per ADR 0019 §2
version = "0.3.1"
requires-engine = ">=1.2 <2.0"      # ADR 0021

[agent]
prompt = "prompts/system.md"
harness = "harness/"
tool-profile = "app_only"           # app_only | core | all
allow-tools = ["read_file", "search"]
deny-tools = []

[storage]
namespace = "organize-anything"     # ADR 0019
share-credentials = true

[model]
provider = "…"
name = "…"
user-overridable = true

[[mcp]]
id = "organize-tools"
command = "mcp/organize-tools"

[security]
permission-mode = "restricted"      # floor only; see §3
```

Selection is `navi acp --app <dir>` / `navi server --app <dir>`, or `NAVI_APP=<dir>`.
The engine reads the bundle; the bundle is never copied into `{data_dir}`, so
updating an app is replacing its directory.

### 3. Powers and limits

**The app profile MAY:**

- set the system prompt, harness pack, and tool allow/deny selection
- enable **its own bundled** MCP servers and WASM plugins — by path inside the
  bundle only, never arbitrary system paths
- set default model and provider (subject to user override)
- set its storage namespace
- **raise** the permission floor (e.g. force `Restricted`)

**The app profile MAY NOT:**

- lower the permission mode below the user's global setting
- disable approval for a tool kind the user requires approval for
- remove OS-security or NAVI-self-protection entries from the computer-use
  deny-list — append-only, mirroring ADR 0016 §4
- disable session secret redaction
- enable MCP servers or plugins outside its own bundle
- read or write outside its ADR 0019 scope plus the declared shared artifacts

Security settings compose as **the most restrictive wins**. An app can only tighten.
This is what makes the tier safe to grant capability to: the app gains *reach*
(its own tools) without gaining *permission* (a weaker security posture).

### 4. Project config is ignored under an app profile

When an app profile is active, `.navi/config.toml` is **not loaded** unless the app
sets `allow-project-config = true`.

Rationale: applications are not repository-scoped. `organize-anything` operates on a
game directory; a mod folder must not be able to reconfigure the application's
model, harness or security. Apps that are genuinely project-oriented (a coding app)
opt in explicitly.

When opted in, project config keeps its existing restrictions — it still cannot
enable plugins or MCP, and it cannot loosen security.

### 5. User visibility and disclosure

On first run of an app profile, the engine surfaces what the app is requesting:
identity, tool count, MCP servers, permission floor, storage namespace. Subsequent
runs are silent unless the profile's capability-relevant content changes, which is
detected by hashing the relevant `app.toml` sections.

**This is defence by disclosure, not cryptography.** A malicious `app.toml` cannot
weaken security (§3) but can still request a wide tool surface with a persuasive
prompt. The mitigations are the security floor and the fact that first-party apps
ship as a unit with their own binary, so an attacker who can rewrite `app.toml`
could rewrite the application anyway.

If third-party apps are ever supported, this is insufficient and the bundle must be
signed — ADR 0008's Ed25519 plugin signing is the model to reuse. Explicitly out of
scope: this ADR governs first-party applications.

### 6. Precedence table

| Key | Winner |
|---|---|
| provider credentials | user only — app cannot set |
| `permission_mode` | most restrictive of user and app |
| approval-required tool kinds | union of user and app |
| computer-use deny-list | union; OS-security entries not removable |
| `redact_secrets_in_sessions` | true if either requires it |
| model / provider | user, if explicitly pinned; else app default |
| prompt, harness, tool selection | app |
| MCP / WASM enablement | app, bundle-relative paths only |
| storage namespace | app |

## Consequences

Positive:
- Applications can declare their own capability through a tier whose trust basis
  actually supports it, without weakening the project tier
- The project tier's security rule is preserved unchanged
- An app is a directory of data — no rebuild, satisfying ADR 0018 R9
- Apps can only tighten security, so installing one cannot silently widen the
  agent's reach
- Updating an app is replacing a directory; nothing is copied into `{data_dir}`
- Project config no longer applies where it makes no sense (§4)

Negative:
- A third tier makes config resolution meaningfully harder to reason about and to
  debug; `--print-config` must report provenance per key or this becomes unsupportable
- Per-key precedence (§6) is more complex than last-wins and is easy to implement
  subtly wrong — it needs its own test matrix
- Disclosure-based trust does not scale beyond first-party apps (§5)
- Two ways to express nearly the same thing (app profile vs global config) will
  invite drift in defaults
- Ignoring project config by default will surprise anyone expecting the existing
  precedence, and the opt-in is a footgun in the other direction
- `app.toml` becomes a compatibility surface: fields cannot be renamed once apps
  ship against them

## Alternatives rejected

| Alternative | Why rejected |
|---|---|
| Relax project config to allow MCP/plugins | Destroys the rule that protects users from hostile repositories; `cd` is not consent |
| App writes `.navi/config.toml` | Forbidden by `AGENTS.md` #3; also inherits the weak trust basis |
| CLI flags only, no manifest | Unversioned, undiscoverable, cannot express harness or MCP sets, cannot be disclosed to the user |
| App profile as a fourth "highest priority" tier that wins everything | Would let an app force `yolo`; §3's tighten-only rule is the entire safety argument |
| Install app profiles into `{data_dir}` | Makes updates stateful and creates drift between the bundle and installed copy |
| Sign all bundles now | Correct for third-party, disproportionate for first-party; deferred to ADR 0008's model if needed |

## References
- ADR 0018 — App boundary; the profile travels over `initialize`
- ADR 0019 — Storage namespacing; `[storage]` maps onto it
- ADR 0021 — Engine discovery; `requires-engine` is consumed there
- ADR 0016 §4 — append-only deny-list precedent for tighten-only composition
- ADR 0008 — Plugin signing, the model if third-party apps are ever supported
- ADR 0006 — Security defaults are mandatory
- `navi-core/src/harness_pack/` — the declarative harness format `[agent].harness` points at
- `AGENTS.md` — config load order, project-tier restrictions, non-negotiable #3
