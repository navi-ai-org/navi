# Integrating an external browser engine with NAVI

`navi-browser` exposes a small `BrowserEngine` / `BrowserEngineFactory` trait
contract. The built-in backend uses CDP (Chrome DevTools Protocol) to drive a
local Chrome/Chromium process or connect to an existing `cdp_url`. If you want
to add a custom browser engine, implement the traits and register your factory
at process startup.

## What you implement

### 1. `BrowserEngine` — live session

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
| `id` | Stable backend id |
| `available` | Can this factory serve the current config? |
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
    navi_browser::set_engine_factory(Arc::new(my_engine::Factory::default()));

    // Then start NAVI as usual...
}
```

With `backend = "auto"`, the registered factory is preferred when `available`
is true. The built-in CDP fallback is used when no external factory is
registered or it reports unavailable.

## Config knobs (already in NAVI)

```toml
[browser]
enabled = true
backend = "auto"          # or "cdp" for the built-in CDP backend
headless = true
allow_private_network = true
proxy = ""
timeout_ms = 30000
binary_path = ""          # optional override for the browser binary
cdp_url = ""              # existing CDP endpoint, e.g. http://127.0.0.1:9222
```

## URL safety

`BrowserSession::goto` already runs `validate_navigation_url` (http/https only;
private nets gated by `allow_private_network`). Engines may assume URLs are
pre-validated but should still fail closed on `file://` if called directly.

## Checklist for the binding author

- [ ] Implement `BrowserEngine` + `BrowserEngineFactory`
- [ ] Honor `EngineContext.profile_dir` / `artifacts_dir` (no project-local state)
- [ ] Support headless + optional proxy from `BrowserRuntimeConfig`
- [ ] Call `navi_browser::set_engine_factory` from the host binary (or provide a
      `navi-sdk` hook / feature that does it)
- [ ] `doctor()` returns clear install/license hints
- [ ] Tests with a mock page or recorded fixture (no network in CI if possible)
