# Integrating a CloakBrowser Rust binding with NAVI

Upstream PR: [CloakHQ/CloakBrowser#438](https://github.com/CloakHQ/CloakBrowser/pull/438)
(community Rust client under `rust/`, playwright-rs + stealth binary).

NAVI does **not** depend on Python/Node CloakBrowser wrappers for the preferred
path. The agent-facing tool (`browser`) talks only to the traits in this crate.

## Built-in adapter (feature `cloakbrowser`)

With `navi-browser` feature `cloakbrowser` enabled, NAVI ships
`engines/cloakbrowser.rs` which implements the contract against the PR crate:

```toml
# crates/navi-browser/Cargo.toml (already wired)
cloakbrowser = { path = "../../../lab/CloakBrowser-rust/rust/cloakbrowser", optional = true }
```

Enable from the binary:

```bash
cargo run -p navi-cli --features browser-cloak
# or
cargo build -p navi-core --features browser-cloak
```

After the PR merges, switch the dep to git/crates.io and drop the local path.

## What you implement

### 1. `BrowserEngine` — live session

Map CloakBrowser APIs onto:

| Method | Purpose |
|---|---|
| `open` | Launch browser / ensure page |
| `goto` | Navigate (http/https already validated by session) |
| `snapshot` | Text/a11y summary for the model |
| `screenshot_png` | Raw PNG bytes |
| `click` / `type_text` / `press` | Interaction |
| `content` | Body text or HTML (truncated) |
| `evaluate` | Optional JS |
| `close` | Tear down |
| `status` | Diagnostics |

### 2. `BrowserEngineFactory` — construction

| Method | Purpose |
|---|---|
| `id` | Stable id, use `"cloakbrowser"` |
| `available` | Binary/license present for this config? |
| `doctor` | JSON-friendly diagnostics + hints |
| `create` | Return `Arc<dyn BrowserEngine>` using `EngineContext` |

`EngineContext` provides:

- `data_dir` — NAVI data root
- `session_id`
- `profile_dir` — put user-data / persistent profile here (not in the project tree)
- `artifacts_dir` — screenshots, etc.

### 3. Register at process start

In the host that links both crates (`navi-cli`, `navi-sdk` consumer, tests):

```rust
use std::sync::Arc;

fn main() {
    // Your crate:
    navi_browser::set_engine_factory(Arc::new(cloakbrowser_navi::Factory::default()));

    // Then start NAVI as usual…
}
```

With `backend = "auto"`, the primary factory is preferred when `available` is true.
CDP fallback (feature `cdp-fallback`) is only used if no primary is registered or it is unavailable.

## Suggested crate layout

```text
cloakbrowser/           # low-level FFI / binding to stealth Chromium
cloakbrowser-navi/      # optional: implements BrowserEngineFactory
navi (navi-browser)     # traits + session + tool
```

Or implement the traits directly inside the binding crate under a `navi` feature.

## Config knobs (already in NAVI)

```toml
[browser]
enabled = true
backend = "auto"          # or "cloakbrowser" once factory id matches
headless = true
allow_private_network = true
proxy = ""
timeout_ms = 30000
binary_path = ""          # optional override for the stealth binary
```

Pass `BrowserRuntimeConfig` fields into your launcher (headless, proxy, binary_path, timeout).

## URL safety

`BrowserSession::goto` already runs `validate_navigation_url` (http/https only;
private nets gated by `allow_private_network`). Engines may assume URLs are
pre-validated but should still fail closed on `file://` if called directly.

## Checklist for the binding author

- [ ] Implement `BrowserEngine` + `BrowserEngineFactory` (`id = "cloakbrowser"`)
- [ ] Honor `EngineContext.profile_dir` / `artifacts_dir` (no project-local state)
- [ ] Support headless + optional proxy from `BrowserRuntimeConfig`
- [ ] Call `navi_browser::set_engine_factory` from the host binary (or provide a
      `navi-sdk` hook / feature that does it)
- [ ] `doctor()` returns clear install/license hints
- [ ] Tests with a mock page or recorded fixture (no network in CI if possible)

## Enabling in Cargo (when ready)

In `crates/navi-browser/Cargo.toml`:

```toml
[features]
cloakbrowser = ["dep:your-cloakbrowser-crate"]

[dependencies]
your-cloakbrowser-crate = { path = "../../../cloakbrowser", optional = true }
```

Then implement `src/engines/cloakbrowser.rs` as a thin adapter, **or** keep
registration host-side only (preferred for decoupling versioning).
