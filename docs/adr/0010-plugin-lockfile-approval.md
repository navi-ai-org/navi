# ADR 0010 — Plugin Lockfile and Approval Persistence

## Status
Accepted

## Context
When a user approves a plugin's capabilities at install time, that approval must
persist across sessions. Re-approving on every startup is poor UX. However, if a
plugin is updated and its capabilities change, the user must re-consent. The system
needs a durable record of what was approved and when.

## Decision
An aggregate lockfile (`navi-plugins.lock`) records all installed plugins, their
integrity hashes, and their approved capabilities.

### Lockfile Format

The lockfile is TOML, stored in the NAVI data directory:

```toml
[[plugins]]
id = "my-plugin"
version = "1.0.0"
publisher = "gh:username"
wasm_hash = "sha256:..."
capabilities_hash = "sha256:..."
tools_hash = "sha256:..."
approved_capabilities = ["fs_read", "net_get"]
approved_at = "2026-06-01T00:00:00Z"
```

### Lockfile Fields

| Field | Purpose |
|-------|---------|
| `id` | Plugin identifier |
| `version` | Installed version |
| `publisher` | Publisher identity |
| `wasm_hash` | SHA-256 of the `.wasm` binary |
| `capabilities_hash` | SHA-256 of the capabilities declaration |
| `tools_hash` | SHA-256 of the tools declaration |
| `approved_capabilities` | List of capability IDs the user approved |
| `approved_at` | ISO 8601 timestamp of approval |

### Approval Persistence

- On install, the user approves capabilities. The lockfile entry is created with
  `approved_capabilities` and `approved_at`.
- On subsequent loads, the lockfile is checked. If the entry exists and the hashes
  match, the plugin is loaded without re-prompting.
- If the lockfile is missing or the entry is absent, the user is prompted for approval.

### Update Reconsent

When a plugin is updated (version change), the system compares:

1. `wasm_hash` — did the binary change?
2. `capabilities_hash` — did capabilities change?
3. `tools_hash` — did tool definitions change?

If **any** hash differs from the lockfile entry, the user MUST re-consent. The lockfile
entry is updated only after successful re-approval.

This prevents a plugin update from silently gaining new capabilities (e.g., adding
network access to a previously read-only plugin).

### Lockfile Operations

| Operation | Behavior |
|-----------|----------|
| `load` | Parse TOML; return empty if file missing |
| `save` | Serialize to TOML with pretty formatting |
| `find` | Lookup by plugin ID |
| `upsert` | Add or replace entry by ID |
| `remove` | Delete entry by ID |

### Legacy Migration

There is no legacy lockfile format. Plugins installed before the lockfile system
will not have entries and will require re-approval on first load after the upgrade.

## Consequences
Positive:
- Approval is durable — no re-prompting on every startup
- Hash-based change detection triggers reconsent only when needed
- Single aggregate file is easy to inspect, back up, and version-control
- Prevents silent capability escalation on plugin updates

Negative:
- Lockfile is a single point of failure — corruption requires re-approval of all plugins
- No atomic multi-plugin install (each plugin is a separate lockfile entry)
- Hash comparison is all-or-nothing — a minor tool description change triggers full
  reconsent (could be refined to capability-level diffing in the future)
