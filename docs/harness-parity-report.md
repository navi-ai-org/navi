# Navi Harness Parity Report

Data: 2026-06-27

## Status Summary

| Sprint | Tema | Status | Implementado | Testes |
|--------|------|--------|-------------|--------|
| P0 | Baseline inventory | ✅ | Mapeei harness atual + referências Codex/Claude | — |
| P1 | Tool Kernel v2 | ✅ | `ToolMetadata`, `ToolExposure`, `ToolRisk`, `builtin_metadata` para 30+ tools | 25 |
| P2 | Tool Registry + tool.search | ✅ | `ToolRegistry`, `ToolSearchTool`, exposure filtering, phase-based toolsets | 25 |
| P3 | Structured errors | ✅ | `ToolErrorBuilder` com `error_code`/`message`/`recoverable`/`retryable`/`hint` | 5 |
| P4 | Unified exec / code mode | ✅ | `ProcessTool` (exec/stdin/wait/list/cancel) com quotas | 16 |
| P5 | Sandbox / snapshot / rollback | ✅ | `SandboxManager`, `WorkspaceSnapshot`, `ChangeSet`, rollback | 13 |
| P6 | Effect-based permissions | ✅ | `EffectAnalyzer`, `EffectReport`, `BlastRadius`, `PostDecision` | 16 |
| P7 | Verifier API | ✅ | `VerifierSpec`, `VerifierRunner`, `VerificationStore` | 18 |
| P8 | Trace Store | ✅ | `TurnTrace`, `TraceStore` (JSONL), `TurnMetrics`, `TurnOutcome` | 8 |
| P9 | MCP hardening | ✅ | Deferred metadata, schema validation, output truncation, allowlist | 3 |
| P10 | Subagent profiles | ✅ | `AgentProfile`, `ApprovalMode`, ReadOnly/Escalate enforcement | 2 |
| P11 | Parity Gate | ✅ | Integration test suite (22 tests), this report | 22 |

## Total: 781 testes (688 unit + 22 parity + 55 navi-sdk + 15 navi-mcp + 1 doc) — 0 falhas, 0 warnings

## Sprints Implementados

### P1 - Tool Kernel v2
- `ToolMetadata` com: `namespace`, `risk`, `is_read_only`, `is_concurrency_safe`, `supports_streaming`, `supports_batch`, `supports_rollback`, `max_output_bytes`, `exposure`, `capabilities`, `verifier`, `examples`, `tags`, `extensions`
- `ToolExposure`: Direct, Deferred, Hidden, ModelOnly, Internal
- `ToolRisk`: Unspecified, Low, Medium, High, Critical
- `builtin_metadata()` com lookup para 30+ tools
- Builders: `reader()`, `writer()`, `command()`, `deferred()`, `internal()`
- Integrado no `ToolExecutor.register_tool()`

### P2 - Tool Registry + tool.search
- `ToolRegistry` centralizado com search BM25, exposure control, phase filtering
- `visible_definitions()` filtra por Direct/ModelOnly
- `ToolSearchTool` (deferred) descobre tools ocultas
- `ToolSet::for_phase()` com fases: planning, reading, editing, verifying, reviewing, recovery

### P3 - Structured Errors
- `ToolErrorBuilder` com `.recoverable()`, `.hint()`, `.stderr()`, `.build()`
- Erros padronizados: `error_code`, `message`, `recoverable`, `retryable`, `hint`

### P4 - Unified Exec
- `ProcessTool` com ações: exec, stdin, wait, list, cancel
- `ProcessManager` com quotas: max_processes (8), default_timeout_ms (30000), max_output_bytes (65536)

### P5 - Sandbox / Snapshot / Rollback
- `SandboxManager::create_snapshot()` — captura estado antes de writes
- `SandboxManager::compute_changes()` — detecta arquivos criados/modificados/deletados
- `SandboxManager::rollback()` — reverte para o snapshot
- `SandboxTool` (deferred) com ações: snapshot, rollback, reset, status

### P6 - Effect-Based Permissions
- `EffectAnalyzer::analyze()` — analisa paths por sensibilidade
- `EffectReport` — `files_created/modified/deleted`, `blast_radius`, `key_files_affected`
- `PostDecision` — Allow, Ask, Deny, Rollback
- `BlastRadius` — SingleFile, MultipleFiles, DependencyChange, CiConfig, SecuritySensitive

### P7 - Verifier API
- `VerifierSpec` — verifier_type, command, cwd, timeout_ms, required
- `VerifierResult` — status, command, duration_ms, stdout, stderr, exit_code, error_class, suggested_next_action
- `VerifierRunner` — executa comandos via `tokio::process::Command`
- `VerificationStore` — armazena resultados por feature_id
- `VerifierTool` (deferred) com ações: run, status, list

### P8 - Trace Store
- `TurnTrace` — trace estruturado por turno com tool calls, approvals, verifiers, métricas
- `TraceStore` — persistência append-only JSONL em `<data_dir>/traces/`
- `TurnMetrics` — tool_call_count, failed_tool_calls, approval_count, verifier_count, retry_count, rollback_count, wall_time_ms
- `TurnOutcome` — Success, PartialSuccess, Stopped, Failed

### P9 - MCP Hardening
- MCP tools com metadata: namespace "mcp", exposure Deferred, risk Medium
- Schema validation: invalid schemas são tratados gracefully
- Output truncation a 64KB
- Allowlist por workspace em `SecurityConfig`

### P10 - Subagent Profiles
- `AgentProfile`: Explorer, Implementer, Reviewer, Verifier, Summarizer
- `ApprovalMode`: Inherit, Escalate, ReadOnly, DenyWrite
- `SubagentOptions` com profile, model, tools, approval, max_tokens
- ReadOnly e Escalate enforcement no dispatch de tools

### P11 - Parity Gate
- 22 testes de integração em `tests/parity_check.rs`
- Cobertura: metadata, exposure, search, sandbox, effects, verifier, trace, MCP, subagent profiles

## Thresholds

| Métrica | Target | Status |
|---------|--------|--------|
| Tool metadata para todas builtin tools | 100% | ✅ |
| Deferred tools invisíveis no prompt | 100% | ✅ |
| tool.search funcional | Sim | ✅ |
| Sandbox/rollback operacional | Sim | ✅ |
| Effect analysis funcional | Sim | ✅ |
| Verifier API funcional | Sim | ✅ |
| Trace store funcional | Sim | ✅ |
| MCP tools com metadata adequada | 100% | ✅ |
| Subagent profiles com approval | Sim | ✅ |
| secret_exposure_events = 0 | Sim | ✅ (tested) |
| unsafe_guarded_effects_auto_approved = 0 | Sim | ✅ (tested) |

## Gaps Remanescentes

- **Branch racing** (P3 no beyond-parity): não implementado
- **Code mode com SDK tipado** (B2): MVP apenas
- **Multi-model routing** (B10): não implementado
- **Auto-skill mining** (B8): não implementado
- **Operational memory layers** (B7): não implementado
- **AST/LSP code graph** (B1): não implementado

Estes gaps são escopo do plano beyond-parity (`docs/harness-beyond-parity-plan.md`).
