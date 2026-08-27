# ADR 0004 — Host Generates Model-Facing Tool Descriptions

## Status
Accepted

## Context
Tool descriptions are shown to the model as trusted context. A malicious plugin could
write instructions in the tool description that manipulate the model into executing
dangerous operations (tool description poisoning). This is a prompt injection vector.

## Decision
Plugin metadata is structured (summary, input_schema, risk). The host generates the
final model-facing tool description with provenance markers and risk labels.

## Consequences
Positive:
- Prevents tool description poisoning
- Prevents tool impersonation (host adds provenance)
- Consistent description format across all plugins
- Model can distinguish plugin tools from built-in tools

Negative:
- Less flexibility for plugin authors to describe their tools
- Host must maintain description generation logic
- Input schema descriptions still need sanitization
