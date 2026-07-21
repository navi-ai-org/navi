# 02 — Programmatic engine config / data dir on builder

**What to build:** Hosts can construct a `NaviEngine` with an injected config and durable data directory (e.g. app-owned path) without depending only on discovering `.navi/config.toml` under a random project tree.

**Blocked by:** None — can start immediately.

**Repo:** navi (`NaviEngineBuilder::loaded_config` already exists in Rust; expose a safe NAPI path)

**Status:** done

## Acceptance criteria

- [x] NAPI builder can set data dir and/or pass a config payload that maps to `LoadedConfig`
- [x] Host can point Navi state (sessions, credentials path defaults, plugins root) at an app-controlled directory
- [x] `from_project` / simple constructor behavior remains available and unchanged when overrides are omitted
- [x] Invalid config fails with a clear error (not a panic)
- [x] Documented example for Electron/desktop hosts
- [x] Test proves engine uses injected `data_dir` for at least one durable artifact path
