# navi-plugin-broker

[![Crates.io](https://img.shields.io/crates/v/navi-plugin-broker)](https://crates.io/crates/navi-plugin-broker)
[![License](https://img.shields.io/crates/l/navi-plugin-broker)](../LICENSE)

Plugin installation and transport broker for [NAVI](https://github.com/navi-ai-org/navi).

`navi-plugin-broker` handles the **acquisition, validation, and storage** of WASM and native plugins — fetching from HTTP registries, verifying integrity, and staging files into the local plugin directory.

## Modules

| Module | Purpose |
|--------|---------|
| `fs_broker` | Local filesystem operations — staging, copying, and audit logging |
| `http_broker` | HTTP downloads with capability checks and response validation |
| `git_broker` | Git-based plugin source operations |
| `install_approval` | Install-time approval flow with risk assessment and consent |
| `output_sanitizer` | Sanitizes plugin output before storage |

## Install flow

```text
registry catalog → stage_plugin_by_id → http_broker::fetch
    → verify hash → install_approval → fs_broker::write → lockfile update
```

Each step produces audit entries and risk assessments that the host can surface to the user for approval.

## Part of the NAVI workspace

This crate depends on [`navi-plugin-manifest`](https://crates.io/crates/navi-plugin-manifest).

**Full project:** <https://github.com/navi-ai-org/navi>
**License:** Apache-2.0
