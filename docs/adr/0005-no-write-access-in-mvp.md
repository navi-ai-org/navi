# ADR 0005 — File Write Access Is Out of MVP

## Status
Accepted

## Context
Write access to the filesystem is the most dangerous capability for a code agent plugin.
A malicious plugin could modify source code with backdoors, alter git configuration,
write to shell startup files, or inject prompt injection content into project files.

## Decision
MVP supports read-only filesystem access only. Write access is deferred to post-MVP
with a patch proposal model (plugin proposes changes, host shows diff, user approves).

## Consequences
Positive:
- Dramatically reduces attack surface in MVP
- Eliminates entire classes of attacks (backdoor injection, git manipulation)
- Simpler FS broker implementation (read-only)

Negative:
- Some legitimate plugins cannot be built (code formatters, refactoring tools)
- Write access will eventually be needed
- Future write model needs careful design (diff approval)
