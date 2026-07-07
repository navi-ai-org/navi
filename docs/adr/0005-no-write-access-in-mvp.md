# ADR 0005 — File Write Access: Patch Proposal Model

## Status
Superseded — write access is implemented. This ADR is retained for historical context.

## Context
Write access to the filesystem is the most dangerous capability for a code agent plugin.
A malicious plugin could modify source code with backdoors, alter git configuration,
write to shell startup files, or inject prompt injection content into project files.

The original decision deferred write access to post-MVP. Write access has since been
implemented through the built-in tools `WriteFileTool` and `ApplyPatchTool`.

## Decision (Historical — MVP Phase)
MVP supported read-only filesystem access only. Write access was deferred with a
patch proposal model (plugin proposes changes, host shows diff, user approves).

## Current Implementation

Write access is now available through two built-in tools:

| Tool | Purpose | Security |
|------|---------|----------|
| `write_file` | Whole-file replacement | Requires approval (SecurityPolicy) |
| `apply_patch` | Targeted unified diff via `git apply` | Requires approval (SecurityPolicy) |

The **patch proposal model** is implemented:
1. The model/tool proposes changes as a unified diff or file content.
2. `SecurityPolicy::validate` returns `NeedsApproval` for write operations.
3. The TUI presents an approval prompt (or headless mode gates by default).
4. Only after user approval does the write execute.

For plugin filesystem access, the `FsBroker` mediates all writes. Community plugins
(`TrustLevel::Community`) are restricted to `ReadOnly` filesystem capability by the
validator. Higher trust levels may declare `ReadWrite` access.

## Consequences (Historical)
Positive:
- Dramatically reduced attack surface during MVP
- Eliminated entire classes of attacks (backdoor injection, git manipulation)
- Simpler FS broker implementation (read-only)

Negative:
- Some legitimate plugins could not be built (code formatters, refactoring tools)
- Write access was eventually needed
- Future write model needed careful design (diff approval)

## Consequences (Current)
Positive:
- Patch proposal model provides user-controlled write gating
- `apply_patch` enables targeted, reviewable changes
- Approval flow is consistent across TUI and headless modes
- Plugin write access is still gated by trust level and capability declarations

Negative:
- Write tools are a critical security surface — approval flow bugs could allow
  unintended writes
- The approval UX must be clear enough that users actually review diffs
