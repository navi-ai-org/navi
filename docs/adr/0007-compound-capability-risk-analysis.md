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

## Consequences
Positive:
- Users see clear warnings for dangerous combinations
- Risk is computed per-tool, not per-plugin (more precise)
- Installation UI makes exfiltration risk visible
- Reconsent required if risk increases on update

Negative:
- More complex risk classification logic
- Some legitimate tools may trigger HIGH/CRITICAL warnings
- Users may develop warning fatigue
