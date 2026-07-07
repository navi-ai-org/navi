# ADR 0006 — Security Defaults Are Mandatory

## Status
Accepted

## Context
Wasmtime, HTTP clients, and filesystem access have many configuration options. If
security limits are optional or configurable by plugins, they will be misconfigured.
A plugin with no fuel limit can loop forever. A plugin with no memory limit can cause OOM.

## Decision
Security defaults (fuel, memory, timeout, response caps, rate limits, IP blocking,
redirect limits) are mandatory and cannot be disabled or weakened by plugins.

## Consequences
Positive:
- No plugin can run without resource limits
- No plugin can bypass IP blocking or redirect validation
- Predictable resource consumption
- Defense in depth (even if one layer fails, others catch it)

Negative:
- Some legitimate use cases may hit limits (large file processing, long computations)
- Limits need tuning based on real-world usage
- More configuration to maintain
