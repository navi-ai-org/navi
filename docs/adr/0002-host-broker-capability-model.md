# ADR 0002 — All Sensitive Access Goes Through Host Brokers

## Status
Accepted

## Context
Even with WASM sandboxing, plugins need to read files, make HTTP requests, and access
git. If plugins access these directly (even within WASM), the host cannot enforce
authorization, validation, or resource limits.

## Decision
Plugins MUST NOT access filesystem, network, secrets, git, TUI, or model context directly.
All sensitive access MUST go through host brokers that enforce authorization.

## Consequences
Positive:
- Every sensitive operation is mediated, logged, and checked
- Authorization is enforced at the broker, not trusted to the plugin
- Audit trail for all plugin activity
- Resource limits enforced per-invocation

Negative:
- More code to implement and maintain
- Slightly higher latency for broker-mediated calls
- Broker becomes a critical security component
