# ADR 0007 — Compound Capability Risk Analysis

## Status
Accepted

## Context
Individual capabilities may be safe in isolation but dangerous in combination.
filesystem:read is safe. network:POST is safe. Together they form an exfiltration
pipeline. The system must analyze capability combinations, not just individual capabilities.

## Decision
The system MUST compute risk from capability composition per tool. Dangerous combinations
(fs_read + network, fs_read + auth_binding, write + network) MUST be classified as
HIGH or CRITICAL with explicit warnings at install time.

## Risk Severity Levels

The `RiskLevel` enum defines five severity levels:

| Level | Score | Meaning |
|-------|-------|---------|
| `Low` | 1 | Safe in isolation and combination |
| `Medium` | 2 | Read-only access, bounded network |
| `High` | 4 | Write access or authenticated exfiltration risk |
| `Critical` | 8 | Unrestricted exfiltration or write+network |
| `Forbidden` | 16 | Capability combination rejected for community plugins |

`Forbidden` is a hard rejection — the validator will not allow a community plugin to
declare a `Forbidden`-level capability combination. Example: `fs_read + network_wildcard`
(read files + POST to any host) is `Forbidden` because it enables unrestricted data
exfiltration with no mitigating constraint.

## Compound Risk Analysis

The classifier (`classify_tool_risk`) checks **all pairs and all triples** of a tool's
declared capabilities:

**Pair rules:**
- `fs_read + network_wildcard` → `Forbidden`
- `fs_read + network_POST` → `Critical`
- `write + network` → `Critical`
- `fs_read + auth_binding` → `High`
- `fs_read + network_GET` → `High`

**Triple rules:**
- `fs_read + auth_binding + network_POST` → `Critical`

Higher-order combinations (4+) are not checked because the pair/triple rules already
cover the known dangerous patterns. New rules can be added as patterns are identified.

## Per-Tool Risk Isolation

Risk is computed **per tool**, not per plugin. Two tools in the same plugin that each
use a single capability do not elevate each other's risk:

```
plugin "my-plugin":
  tool "search" → [fs_read]     → MEDIUM
  tool "post"   → [network_POST] → HIGH
```

Only when a single tool declares both capabilities does the compound rule fire:
```
  tool "check_config" → [fs_read, network_POST] → CRITICAL
```

This prevents plugins from being penalized for having separate read and network tools
that are individually safe.

## Consequences
Positive:
- Users see clear warnings for dangerous combinations
- Risk is computed per-tool, not per-plugin (more precise)
- Installation UI makes exfiltration risk visible
- Reconsent required if risk increases on update
- `Forbidden` level prevents community plugins from declaring the most dangerous patterns
- Triple-capability analysis catches authenticated exfiltration chains

Negative:
- More complex risk classification logic
- Some legitimate tools may trigger HIGH/CRITICAL warnings
- Users may develop warning fatigue
- `Forbidden` may block some legitimate use cases that need wildcard network + read access
