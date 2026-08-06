# ADR 0016 — Computer Use / OS Automation Tool Surface

## Status
Proposed

## Context
NAVI agents operate inside the terminal today: files, shell, repo, headless browser
(`navi-browser`). There is no path for an agent to interact with the rest of the user's
desktop — native applications, system windows, or the pointer/keyboard at the OS level.

The OS accessibility/input APIs are powerful and, crucially, **native**:

- macOS — `AXUIElement` (Accessibility API) + `CGEvent` (input) + `CGWindowList` (windows)
- Windows — UI Automation (UIA, COM) + `SendInput` + Win32 window APIs (`EnumWindows`,
  `SetForegroundWindow`, …)
- Linux — AT-SPI2 (accessibility) + X11/XTest or uinput (input); Wayland exposes almost
  nothing without compositor portals

These require in-process FFI to platform frameworks. They cannot run inside the WASM
plugin sandbox (ADR 0013): WASM plugins have no access to COM, ApplicationServices, or
the display server. The broker model (ADR 0002) was designed for fs/http/git mediation,
not for driving the user's GUI.

NAVI already has a proven pattern for exactly this shape of capability: `navi-browser`
is a separate crate that backs a feature-gated builtin tool (`browser`), registered
behind `#[cfg(feature = "browser")]` in `navi-core`. Computer use is the same pattern
applied to the desktop instead of a headless browser.

The existing permission ladder (`Restricted → AcceptEdits → Auto → Yolo`) and the
`ToolExposure` model (`Direct` / `Deferred` / `Hidden`) are sufficient to gate this
surface without inventing a new approval framework — provided the risk category is
recognized and the per-mode behavior is defined.

## Decision

### 1. Computer use is first-party native core, not a plugin

OS automation lives in new first-party crates behind a facade, registered as builtin
tools in `navi-core`. It is **never** a WASM plugin and is **not** exposed through a
host broker. This follows `navi-browser`, not the plugin path.

Crate layout:

```
crates/
  navi-computer-use/      facade: cfg-selects the platform backend, exposes the
                          tool-facing API (capture, enumerate, simulate, inspect)
  navi-os-windows/        UIA + SendInput + Win32 (windows crate)
  navi-os-macos/          AXUIElement + CGEvent + CGWindowList (core-foundation /
                          objc2 / ApplicationServices)
  navi-os-linux/          AT-SPI2 + X11/XTest; Wayland best-effort via portals
```

`navi-core` depends on `navi-computer-use` (the facade), never on a platform backend
directly — mirroring the rule that consumers depend on `navi-providers`, not
`navi-openai`. Backends are selected at compile time via `cfg(target_os = ...)`.

Feature gate: `computer-use = ["dep:navi-computer-use"]` in `navi-core/Cargo.toml`,
**off by default** (unlike `browser`, which is on by default). Rationale: browser
automation is contained to a headless process; OS automation drives the real desktop
and should be opt-in.

### 2. Tool surface and exposure

Tools are registered in `ToolExecutor::register_builtin_tools()` behind
`#[cfg(feature = "computer-use")]`, mirroring `browser.rs`. Initial tools:

| Tool | Kind | Exposure | Notes |
|------|------|----------|-------|
| `capture_screen` | Read | Deferred | Returns image to model via the existing `view_image` path |
| `enumerate_windows` | Read | Deferred | Window tree (title, pid, rect, focused) |
| `inspect_element` | Read | Deferred | Accessibility tree query (UIA/AX/AT-SPI) |
| `simulate_input` | Command | Deferred | Click / type / key / scroll / drag — single multi-action tool |

All computer-use tools are **`Deferred`**: they stay out of the model's tool schema and
must be discovered via `tool_search` then called by name. This keeps the default surface
clean and requires explicit intent, matching how other high-risk power tools are
handled. `simulate_input` is `ToolKind::Command` (high risk); the read tools are
`ToolKind::Read`.

A new `ToolRisk::Critical` is added for `simulate_input` to distinguish it from ordinary
shell commands in metadata and audit logs.

### 3. Security model — new risk category, existing ladder

A new `SecurityRisk::UiAutomation` variant is added to `security.rs`. The existing
`PermissionMode` ladder governs it; no new approval framework is introduced.

Per-mode behavior for `UiAutomation`:

| Mode | Read tools (capture / enumerate / inspect) | `simulate_input` | Deny-list apps |
|------|---------------------------------------------|-------------------|----------------|
| **Restricted** | approve each action | approve each action | blocked always |
| **AcceptEdits** | auto (read-only) | approve each action | blocked always |
| **Auto** | auto | auto, except sensitive fields | blocked always |
| **Yolo** | auto | auto | auto (no blocks) |

Rationale per mode:

- **Restricted** — the user's stated baseline: every step that changes anything asks.
  Read tools also ask in Restricted because even a screenshot can leak credentials on
  screen; keeping it uniform matches "simplest mode, approve everything".
- **AcceptEdits** — mirrors its file behavior: reads auto, writes (input simulation)
  ask. The closest analog to "auto-approve file reads, ask on file writes".
- **Auto** — the existing "guarded" tier. Reads and ordinary input are auto-approved;
  the guard fires on **sensitive fields** — elements the accessibility tree identifies
  as password / confidential / banking — which require approval even in Auto. The
  deny-list of applications is enforced regardless.
- **Yolo** — total. No blocks, including the deny-list and sensitive fields. This is
  the user's explicit choice; the ADR records it rather than overrides it.

### 4. Deny-list of sensitive applications

A configurable deny-list of applications that computer-use tools will not target by
default (process name / window title / bundle id match). Enforced in all modes except
Yolo. Default entries:

- Password managers (1Password, Bitwarden, KeePass, Apple Passwords, …)
- Banking / finance apps
- OS security settings (Windows Defender / Security Center, macOS System Settings →
  Privacy & Security, Network & Firewall panes)
- Keychain / Credential Manager / `seahorse`
- The NAVI process itself (by pid / window match)

The deny-list is a `SecurityConfig` field (`computer_use.deny_apps: Vec<String>`) with
sensible defaults; users may extend it. It is **not** editable by the agent and is
**not** weakened by project `.navi/config.toml` for the OS-security entries (those are
append-only from project config — project config can add entries, never remove the
defaults for OS security / NAVI self-protection).

Note on Yolo: per the decision above, Yolo bypasses the deny-list entirely. The
self-protection entries for NAVI and OS security are therefore **not** a hard floor —
they are defaults that Yolo overrides. This is accepted as the cost of a truly
unrestricted mode.

### 5. Events

New `RuntimeEventKind` variants, versioned for TUI/Tutor replay:

- `ScreenCaptured { width, height, target: WindowRef? }`
- `WindowsEnumerated { count }`
- `UiElementInspected { ref }`
- `InputSimulated { action, target: WindowRef? }`

These flow through the existing `AgentEvent` projection. Screenshot payloads are stored
in the attachment store (already used by `view_image`) and referenced by path, not
inlined into the event log — keeping session JSON small and consistent with redaction
(session redaction applies: screenshots may contain secrets and are treated as
sensitive content, redacted on save when redaction is on).

### 6. Surface sync

Per AGENTS.md non-negotiable #2, a new engine capability is wired through the full
stack. For computer use:

- `navi-core` — tools, `SecurityRisk::UiAutomation`, events, config types
- `navi-sdk` — `NaviEngine` exposes computer-use availability and config
- `navi-napi` — bindings for the same
- `navi-cli` — `--computer-use` flag / config to enable the feature
- `navi-tui` — render screenshot previews (via existing image preview path), approval
  prompts for `simulate_input`, deny-list hit notices

No half-wired features: either all five surfaces are updated or the feature stays
behind the gate and is not advertised.

### 7. Platform priority

- **Windows** — first implementation. UIA + SendInput + Win32 give coverage comparable
  to macOS, and the workspace already builds/tests on `windows-latest` in CI with
  `windows-sys` present. The `windows` crate (full UIA + input) is the dependency.
- **macOS** — second. AXUIElement + CGEvent + CGWindowList via `objc2` /
  `core-foundation`. Requires Accessibility permission (prompted by the OS on first
  use); the engine reports a clear doctor-check failure when missing.
- **Linux** — third, best-effort. AT-SPI2 + X11/XTest under X11; Wayland is
  best-effort via `xdg-desktop-portal` where the compositor supports it, otherwise
  the tools report unsupported and the feature degrades gracefully (tools present but
  return an error on invoke, not a compile failure).

On platforms without a backend compiled in, the tools are **not registered** (the
`computer-use` feature compiles the facade, but the facade returns
`UnsupportedPlatform` and `register_builtin_tools` skips registration). This keeps the
feature optional per platform without `cfg`-gating the whole feature off.

## Consequences
Positive:
- Agents gain desktop interaction on Windows and macOS at parity with macOS-native
  "computer use" tools, with NAVI's existing approval/redaction model intact
- Reuses the proven `navi-browser` pattern — no new architectural concept
- `Deferred` exposure keeps the default tool schema clean; high-risk tools require
  explicit discovery
- Existing permission ladder is reused; no new approval framework to learn
- Opt-in feature gate means default builds and default sessions are unaffected
- Platform backends are isolated crates; `navi-core` stays free of platform framework
  deps

Negative:
- Three platform backends with very different APIs — each is a real implementation
  effort and a maintenance surface
- Linux/Wayland coverage is inherently weak; the feature will be uneven across OSes
- OS automation is a large trust surface; even with approvals, a compromised or
  misbehaving agent can do anything the logged-in user can (especially in Yolo)
- New `SecurityRisk` variant and deny-list add security-model complexity
- Full surface sync (core → sdk → napi → cli → tui) is required for any shippable
  slice, raising the minimum cost of the first usable version
- Screenshot redaction is best-effort: redaction can drop the image, but cannot scrub
  secrets already captured into the model's context

## References
- ADR 0002 — Host Broker Capability Model (computer use is **not** broker-mediated;
  it is native core, by exception, like `bash` and `browser`)
- ADR 0013 — WASM-Only Plugins (confirms computer use cannot be a plugin)
- ADR 0006 — Security Defaults Are Mandatory (the deny-list defaults and
  `UiAutomation` risk category are mandatory defaults, not plugin-configurable)
- `navi-browser` crate + `browser` feature gate — the pattern this ADR generalizes
- `AGENTS.md` non-negotiable #2 (surface sync) and the Tools & Security section
