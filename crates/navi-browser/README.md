# navi-browser

Pluggable browser backend for the NAVI agent `browser` tool.

## Architecture

```text
browser tool (navi-core)
    → BrowserSession
        → BrowserEngine  ← implement this in CloakBrowser Rust binding
```

1. **Preferred:** CloakBrowser Rust binding registers a `BrowserEngineFactory`
   via `navi_browser::set_engine_factory`.
2. **Fallback (feature `cdp-fallback`, default):** launch Chrome/CloakBrowser
   *binary* and drive it over CDP until the binding is ready.

## For binding authors

See **[INTEGRATION.md](./INTEGRATION.md)**.

## Config

```toml
[browser]
enabled = true
backend = "auto"   # prefers registered CloakBrowser factory
headless = true
allow_private_network = true
```

## CLI

```bash
navi browser status
navi browser doctor
navi browser install
```
