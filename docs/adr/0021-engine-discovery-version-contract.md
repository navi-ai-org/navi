# ADR 0021 — Engine Discovery, Version Contract, and Bundled Fallback

## Status
Proposed

## Context

The product model in ADR 0018 is that **NAVI is a runtime**: an application drives
whatever NAVI the user already has installed, and ships its own copy only as a
fallback. ADR 0020 introduces `requires-engine` in `app.toml` but does not define
how it is checked or what happens when it fails.

Three problems have to be answered together:

**Discovery.** An application must locate an engine. `navi` may be on `PATH`,
installed somewhere unusual, or absent entirely.

**Version skew.** The application is *data* — a harness pack, a prompt, a tool
selection. Its assumptions are **behavioural**, not ABI-level. A prompt tuned
against engine 1.4's tool-loop semantics can silently produce different results on
1.1 or 1.9: no crash, no error, just worse behaviour. This is a harder failure mode
than a linker error, because nothing announces it.

**Fallback.** Shipping a bundled engine solves absence, but introduces a second
copy of a security-sensitive binary on disk.

The protocol layer already handles the *compatible* case: ACP negotiates
`protocolVersion` and capabilities, and ADR 0018 §7 makes the extension profile
additive. This ADR handles the *incompatible* case, which negotiation cannot fix.

## Decision

### 1. Discovery order

An application resolves an engine in this order, first success wins:

1. explicit override — `--engine <path>` or `NAVI_ENGINE=<path>`
2. `navi` on `PATH`
3. platform install locations (npm global prefix, Homebrew, `%LOCALAPPDATA%`)
4. the bundled engine in the app bundle, `bin/navi`

The override exists first so that a developer can point an app at a local build,
and so that a broken discovery never leaves a user stuck.

### 2. Two-phase compatibility check

**Phase 1 — cheap probe, for selection.** `navi --version --format json` returns
engine version, ACP protocol version range, and advertised extension namespaces
without starting a session. Used only to *choose* a candidate.

**Phase 2 — authoritative check, at `initialize`.** The engine evaluates
`requires-engine` against itself and either proceeds or fails with a typed error:

```jsonc
{ "code": -32000,
  "message": "engine version 1.1.3 does not satisfy requires-engine \">=1.2 <2.0\"",
  "data": { "_meta": { "navi.dev": {
      "reason": "engineVersionUnsatisfied",
      "engineVersion": "1.1.3",
      "required": ">=1.2 <2.0",
      "protocolVersions": [2] } } } }
```

Phase 2 is authoritative because the probe can be wrong: a wrapper script, a stale
shim, or a different binary on `PATH` than the one that answers. The launcher treats
a Phase 2 failure as "this candidate is unusable" and continues down the discovery
order, ending at the bundled engine.

Phase 1 exists purely to avoid paying process-spawn plus handshake cost for
candidates that obviously cannot work.

### 3. `requires-engine` is a range, and both bounds matter

Applications declare a semver range, not a minimum.

- The **lower** bound is the app's behavioural floor.
- The **upper** bound is a *major* bound (`<2.0`), not a pin. Additive protocol
  changes and capability negotiation cover minor drift; pinning minors would make
  every engine release break every app, which defeats the purpose of the runtime model.

An app whose behaviour genuinely depends on a minor version has a design problem in
its harness, not a versioning problem, and should be fixed rather than pinned.

### 4. The engine never self-updates on an app's behalf

- The bundled engine is **not** installed onto `PATH`, does not register as the
  system `navi`, and does not write to the user's config.
- An application never triggers an update of the user's installed engine.
- The user's installed engine is preferred whenever it satisfies the range —
  including over a newer bundled copy — so that a user's deliberate upgrade is not
  silently bypassed.

### 5. The bundled engine shares user-owned state

A bundled engine resolves `{data_dir}` exactly as an installed one does, so
credentials and the registry cache are shared (ADR 0019 §1). Falling back to the
bundle must not force re-authentication; if it did, the fallback would be a visibly
worse product and users would be pushed toward installing NAVI just to avoid it.

### 6. Degradation, not silent divergence

When the engine is compatible but does not advertise an extension the app declared
(ADR 0018 §7), the app must degrade with a user-visible notice rather than proceed
as if the feature existed. Example: no `_navi.dev/plan/review` support means plan
mode is not offered, not that plans are silently auto-approved.

Silent degradation of an *approval* path is a security defect, not a UX compromise.

## Consequences

Positive:
- Works whether or not NAVI is installed, with no install prompt
- Behavioural skew fails loudly at `initialize` instead of producing quietly wrong
  agent behaviour
- A user's own installed engine is respected and preferred
- Fallback costs the user nothing: credentials and registry stay shared
- Discovery override makes local development and support debugging tractable
- Major-bound ranges mean routine engine releases do not break shipped apps

Negative:
- **Every app carries a full engine binary.** NAVI is not small; this dominates
  download size for a small app and duplicates on disk when NAVI is installed anyway
- **Bundled binaries are a security-patch liability.** A user who updates their
  installed NAVI does not patch bundled copies. If an app's range excludes the
  patched version, the app keeps running vulnerable code. Mitigation is preferring
  the installed engine and treating app updates as the patch vehicle — which means
  app release cadence is now coupled to engine security releases
- Two-phase checking is two code paths and two failure modes to test
- `--version --format json` becomes a stability commitment: launchers parse it
- Discovery across platforms (npm prefix, Homebrew, Windows paths) is fiddly and
  will need per-platform maintenance
- A user can end up running an engine version they did not choose and cannot easily
  see; `navi app list` must report which engine each app resolved to

## Alternatives rejected

| Alternative | Why rejected |
|---|---|
| Require NAVI installed; no bundle | Turns every app install into a two-step with a prerequisite; unacceptable for a consumer app |
| Always use the bundled engine | Wastes the user's install, multiplies disk and patch liability, and abandons the runtime model |
| Pin an exact engine version per app | Every engine release breaks every app; the runtime model becomes static linking with extra steps |
| Minimum version only, no upper bound | A future major with changed semantics would be accepted and fail unpredictably |
| Probe only (no check at `initialize`) | The probed binary may not be the one that answers; wrappers and shims make probes unreliable |
| Check only at `initialize` (no probe) | Pays spawn plus handshake per candidate during discovery |
| Let the app update the user's engine | An application must not mutate user-level installs |
| Proceed with a warning on skew | Behavioural divergence is invisible; a warning nobody reads is not a control |

## References
- ADR 0018 §7 — protocol version negotiation and capability degradation
- ADR 0019 §1 — shared credentials and registry cache, which §5 depends on
- ADR 0020 — `requires-engine` is declared in `app.toml`
- `crates/navi-cli/src/main.rs` — `--version` surface that §2 formalises
- `AGENTS.md` — `{data_dir}` resolution rules
