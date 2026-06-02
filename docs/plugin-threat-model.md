# NAVI Plugin System Threat Model

## Overview
This document identifies threats to the NAVI plugin system and the defenses against them.
Threats are organized by attack surface.

## Threat Categories

### T1: Code Execution
Threat: Plugin runs arbitrary native code in the host process.
Impact: Full system compromise.
Defense: WASM sandbox prevents arbitrary code execution. Community plugins MUST be WASM.
Residual risk: Zero-day WASM escape (extremely unlikely).

### T2: Data Access — Filesystem
Threat: Plugin reads sensitive files outside project scope.
Impact: Credential theft, SSH key theft, config exposure.
Defense: FS broker with canonicalization, symlink resolution, denylist, size caps.
Residual risk: Denylist miss (unknown sensitive file pattern).

### T3: Data Access — Environment
Threat: Plugin reads environment variables containing secrets.
Impact: API key theft, token theft.
Defense: No env.get in WIT. Auth bindings inject secrets without exposing values.
Residual risk: None for MVP (env not exposed).

### T4: Data Access — Memory
Threat: Plugin reads host process memory.
Impact: Credential theft, session data exposure.
Defense: WASM sandbox isolates memory. Plugin runs in separate linear memory.
Residual risk: Spectre-class side channels (theoretical).

### T5: Network — SSRF
Threat: Plugin makes HTTP requests to internal/metadata services.
Impact: Cloud credential theft, internal service access.
Defense: HTTP broker validates hosts, blocks private IPs, validates redirects, pins DNS.
Residual risk: DNS rebinding (mitigated by pinning).

### T6: Network — Exfiltration
Threat: Plugin sends sensitive data to external servers.
Impact: Data theft.
Defense: Compound risk analysis warns on fs_read + network combinations.
Residual risk: User may approve despite warning.

### T7: Network — Auth Abuse
Threat: Plugin uses injected auth credentials on unauthorized hosts.
Impact: Unauthorized API access.
Defense: Auth bindings scoped to host + method + capability.
Residual risk: Response header leakage (mitigated by sanitization).

### T8: Prompt/Model — Tool Poisoning
Threat: Plugin registers tool with malicious description.
Impact: Model manipulation, instruction injection.
Defense: Host generates descriptions. Tool IDs namespaced. Schema sanitized.
Residual risk: Injection via input_schema descriptions (mitigated by sanitization).

### T9: Prompt/Model — Output Injection
Threat: Plugin tool output contains hidden instructions.
Impact: Model follows attacker instructions.
Defense: Output truncated, marked as untrusted, sanitized.
Residual risk: Novel injection patterns not caught by sanitizer.

### T10: Supply Chain — Update Creep
Threat: Plugin update adds dangerous capabilities.
Impact: Privilege escalation via update.
Defense: Lockfile tracks capabilities_hash. Changes require reconsent.
Residual risk: User approves without reading.

### T11: Supply Chain — Key Compromise
Threat: Publisher signing key is compromised.
Impact: Attacker publishes malicious plugin with valid signature.
Defense: Key change blocks update by default.
Residual risk: Key compromise not detected immediately.

### T12: Resource Abuse
Threat: Plugin consumes excessive CPU, memory, or network.
Impact: DoS of NAVI host.
Defense: WASM fuel, memory limits, wall-clock timeout, HTTP rate limits.
Residual risk: Resource abuse within limits.

### T13: UI Spoofing
Threat: Plugin output mimics NAVI approval prompts.
Impact: User tricked into approving dangerous operations.
Defense: TUI is declarative. Plugin cannot control terminal directly.
Residual risk: Markdown rendering may display misleading content.

### T14: Cross-Plugin Interference
Threat: One plugin affects another plugin's data or execution.
Impact: Data corruption, capability escalation.
Defense: WASM sandbox isolates each plugin. Separate linear memories.
Residual risk: None (WASM provides isolation).

### T15: Agent Core Manipulation
Threat: Plugin modifies system prompt, approval policy, or security policy.
Impact: Complete security bypass.
Defense: Agent core is not extensible. No WIT import for policy.
Residual risk: None (not exposed).

## Threat Summary Table

| ID | Threat | Severity | Defense | Residual |
|---|---|---|---|---|
| T1 | Code execution | CRITICAL | WASM sandbox | Zero-day |
| T2 | FS access | HIGH | FS broker | Denylist miss |
| T3 | Env access | CRITICAL | No env.get | None |
| T4 | Memory access | CRITICAL | WASM isolation | Side channels |
| T5 | SSRF | HIGH | HTTP broker | DNS rebinding |
| T6 | Exfiltration | HIGH | Risk analysis | User approval |
| T7 | Auth abuse | HIGH | Auth scoping | Header leakage |
| T8 | Tool poisoning | HIGH | Host descriptions | Schema injection |
| T9 | Output injection | MEDIUM | Output sanitization | Novel patterns |
| T10 | Update creep | MEDIUM | Reconsent | User approval |
| T11 | Key compromise | MEDIUM | Key change block | Detection lag |
| T12 | Resource abuse | MEDIUM | Runtime limits | Within limits |
| T13 | UI spoofing | MEDIUM | Declarative TUI | Markdown |
| T14 | Cross-plugin | LOW | WASM isolation | None |
| T15 | Core manipulation | CRITICAL | Not exposed | None |
