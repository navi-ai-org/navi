# navi-browser

CDP-only browser backend for the NAVI agent `browser` tool.

## Architecture

```text
browser tool (navi-core)
    → BrowserSession
        → BrowserEngine  ← CDP fallback (Chrome / Chromium / any CDP browser)
```

The built-in engine launches a local Chrome/Chromium process over the Chrome
DevTools Protocol, or connects to an existing `cdp_url`. External engines can
still be registered via `navi_browser::set_engine_factory`.

## Config

```toml
[browser]
enabled = true
backend = "auto"   # launches local Chrome/Chromium or uses cdp_url
headless = true
allow_private_network = true
cdp_url = ""        # e.g. "http://127.0.0.1:9222"
```

## CLI

```bash
navi browser status
navi browser doctor
navi browser install
```
