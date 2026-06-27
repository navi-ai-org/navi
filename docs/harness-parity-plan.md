# Navi Harness Parity Plan

Data: 2026-06-27

Este documento define o plano para levar o Navi a paridade com os harnesses modernos de coding agents. O foco e o harness: tool runtime, tool discovery, policy, sandbox, rollback, verifier, traces, MCP, subagentes e memoria operacional basica.

## Objetivo final

Ao fim deste plano, o Navi deve conseguir executar tarefas reais de repositorio com o mesmo nivel operacional de Claude Code e Codex:

- Tool registry rico, versionado e serializavel.
- Exposicao direta, deferida, oculta e interna de ferramentas.
- `tool.search` funcional para evitar despejar todos os schemas no prompt.
- Execucao de shell, patch, file ops, image inspection, processos persistentes e code mode basico.
- Policy e approvals centralizados, com avaliacao pre-execucao e pos-efeito.
- Sandbox, snapshots e rollback MVP.
- Verifier API integrada ao loop.
- Trace store estruturado para cada turn/tool/verifier.
- MCP client endurecido e integrado ao mesmo modelo de tool/policy.
- Subagentes e batch jobs controlados por escopo, orcamento e profile.
- SDK/TUI/ACP recebendo eventos estruturados sem depender de internals do TUI.

## Escopo e limites

O Navi continua respeitando o boundary do projeto:

- Engine e harness ficam em `navi-core`.
- Superficie de embedding fica em `navi-sdk`.
- TUI e ACP consomem a engine, mas nao recebem logica core.
- MCP server nao entra como requisito de paridade neste plano, porque o boundary atual do repo diz que MCP comeca como client only.
- WebSocket/daemon nao vira interface primaria.
- Navi Tutor e UX educacional ficam fora deste plano.

## Fontes locais obrigatorias para comparacao

Antes de implementar qualquer sprint, o agente executor deve buscar contexto nesses repositorios locais e comparar comportamento, nao copiar codigo diretamente.

### Navi

- `crates/navi-core/src/tool/mod.rs`
- `crates/navi-core/src/security.rs`
- `crates/navi-core/src/event.rs`
- `crates/navi-core/src/turn/mod.rs`
- `crates/navi-core/src/session.rs`
- `crates/navi-sdk/src/engine.rs`
- `crates/navi-mcp/src/lib.rs`
- `docs/harness-engineering-report-2026.md`
- `docs/navi-harness-weaknesses-2026.md`

### Codex local

- `/home/enrell/lab/codex/codex-rs/core/src/tools/router.rs`
- `/home/enrell/lab/codex/codex-rs/core/src/tools/spec_plan.rs`
- `/home/enrell/lab/codex/codex-rs/core/src/tools/spec_plan_tests.rs`
- `/home/enrell/lab/codex/codex-rs/core/src/tools/sandboxing.rs`
- `/home/enrell/lab/codex/codex-rs/core/src/tools/tool_dispatch_trace.rs`
- `/home/enrell/lab/codex/codex-rs/core/src/unified_exec/`
- `/home/enrell/lab/codex/codex-rs/code-mode/src/service.rs`
- `/home/enrell/lab/codex/codex-rs/code-mode/src/service_contract_tests.rs`
- `/home/enrell/lab/codex/codex-rs/code-mode/src/service_tests.rs`

### Claude Code local

- `/home/enrell/lab/claude-code/docs/tools.md`
- `/home/enrell/lab/claude-code/docs/subsystems.md`
- `/home/enrell/lab/claude-code/docs/architecture.md`
- `/home/enrell/lab/claude-code/docs/exploration-guide.md`
- `/home/enrell/lab/claude-code/src/Tool.ts`
- `/home/enrell/lab/claude-code/src/tools.ts`
- `/home/enrell/lab/claude-code/src/hooks/toolPermission/`
- `/home/enrell/lab/claude-code/src/services/mcp/`

Comandos uteis de comparacao:

```bash
rg -n "ToolDefinition|ToolExecutor|ToolKind|SecurityPolicy|RuntimeEvent|HarnessTrace" crates/navi-core crates/navi-sdk crates/navi-mcp
rg -n "ToolExposure|tool_search|ToolRouter|ToolRegistry|CodeModeService|Sandboxable|ApprovalStore" /home/enrell/lab/codex/codex-rs
rg -n "buildTool|isReadOnly|isConcurrencySafe|checkPermissions|ToolSearchTool|MCPTool" /home/enrell/lab/claude-code/src /home/enrell/lab/claude-code/docs
```

## Baseline atual do Navi

O Navi ja tem uma base importante:

- `ToolDefinition` com `name`, `description`, `kind` e `input_schema`.
- `ToolExecutor` com schema validation, normalize paths, lock manager e truncamento.
- Tools builtin como read/search/write/bash/git/package/workflow/runtime/code/init/feature/context/image.
- `SecurityPolicy` com validacao por kind, paths, comandos bloqueados, `.git` e data dir.
- `RuntimeEvent` e `AgentEvent` para tool requested/completed, approvals, compaction, tokens e harness trace.
- MCP client carregado no SDK e registrado como tool.
- Subagent e repo explore registrados via `navi-sdk`.
- Documentos de gaps e harness engineering ja existentes.

O gap de paridade nao e "nao existe nada"; e que a metadata, o roteamento, a verificacao, o sandbox, o rollback e o trace ainda nao formam um sistema operacional completo de harness.

## Matriz de paridade

| Area | Navi atual | Referencia local | Sprint |
|---|---|---|---|
| Tool metadata | Minimalista | Claude `buildTool`, Codex `ToolSpec`/`ToolExposure` | P1 |
| Tool exposure | Todas visiveis por default | Codex direct/deferred/hidden, Claude presets | P2 |
| Tool search | Ausente ou nao central | Codex `tool_search`, Claude `ToolSearchTool` | P2 |
| File/repo ops | Read/search/write/code tools | Claude FileRead/Edit/Write/Grep/Glob, Codex apply/exec | P3 |
| Persistent exec/code mode | Bash background parcial | Codex `code-mode` | P4 |
| Approval/sandbox | Policy por kind/path/command | Codex sandboxing/approvals, Claude permission modes | P5 |
| Effect policy | Basico | Melhor que ambos, mas necessario para parity gate | P6 |
| Verifier API | Feature verification parcial | Estado da arte verifier-first | P7 |
| Trace store | Session events + harness trace | Codex tool dispatch trace | P8 |
| MCP | Client basico | Claude MCP client/server + tool search, Codex deferred MCP | P9 |
| Subagentes | Tool existente, profiles limitados | Codex multi-agent, Claude AgentTool/teams | P10 |
| Gate de paridade | Ausente | Suite comparativa local | P11 |

## Sprint P0 - Inventario e baseline comparativo

### Objetivo do sprint

Criar uma fotografia verificavel do harness atual do Navi e dos pontos equivalentes em Codex/Claude para guiar as mudancas sem regressao.

### Epics

- Inventariar tools builtin, MCP tools e host tools registradas em uma sessao real.
- Inventariar eventos emitidos por uma task simples: read, write, command, approval e erro.
- Mapear equivalencias entre Navi, Codex e Claude em uma tabela mantida no repo.
- Definir naming canonico para tools do Navi sem quebrar compatibilidade existente.

### Entregas

- Documento curto ou teste snapshot com lista de tools, schemas e kind atuais.
- Baseline de eventos para uma task de exemplo.
- Lista de gaps P1-P11 confirmada contra codigo local.

### Definition of Done

- `just test-crate navi-core` passa.
- Nenhuma API publica do SDK e removida.
- O baseline consegue ser regenerado por comando ou teste.

### Objetivo para o proximo sprint

Entrar em P1 com clareza sobre quais campos de metadata sao adicoes compativeis e quais exigem migracao.

## Sprint P1 - Tool Kernel v2

### Objetivo do sprint

Transformar `ToolDefinition` em uma especificacao rica o bastante para routing, policy, UI, traces, concurrency e verifiers.

### Epics

- Adicionar `ToolMetadata` versionada em `navi-core`.
- Campos minimos:
  - `namespace`
  - `risk`
  - `is_read_only`
  - `is_concurrency_safe`
  - `supports_streaming`
  - `supports_batch`
  - `supports_rollback`
  - `max_output_tokens` ou `max_output_bytes`
  - `exposure`
  - `capabilities`
  - `verifier`
  - `examples`
  - `tags`
- Manter `ToolKind` para compatibilidade, mas parar de trata-lo como unico sinal de seguranca.
- Adicionar builders/helpers para tools builtin nao repetirem boilerplate.
- Expor metadata via `navi-sdk`.

### Comparacao obrigatoria

- Claude: comparar com `buildTool()` em `src/Tool.ts` e catalogo em `docs/tools.md`.
- Codex: comparar com `ToolSpec`, `ToolRegistry`, `ToolExposure` e `supports_parallel_tool_calls`.

### Entregas

- `ToolDefinition` compativel com serde antigo.
- Metadata preenchida para todas as tools builtin.
- Testes de serializacao/deserializacao e defaults.

### Definition of Done

- `just test-crate navi-core`
- `just test-crate navi-sdk`
- Snapshot ou teste garantindo que todas as tools builtin declaram metadata minima.

### Objetivo para o proximo sprint

Permitir que o modelo veja apenas as tools necessarias para a fase atual, e descubra o restante sob demanda.

## Sprint P2 - Tool Registry, exposure modes e tool.search

### Objetivo do sprint

Separar tools registradas de tools visiveis ao modelo e introduzir busca deferida de ferramentas.

### Epics

- Criar `ToolRegistry` explicito em `navi-core`.
- Implementar `ToolExposure`:
  - `direct`
  - `deferred`
  - `hidden`
  - `model_only`
  - `internal`
- Implementar `tool.search` v1 com BM25 simples sobre nome, namespace, descricao, tags, examples e capabilities.
- Criar `ToolSet::for_phase(Phase)` para planning, reading, editing, verifying, reviewing e recovery.
- Fazer MCP e plugin tools entrarem no registry com exposure configuravel.
- Adicionar cache de search que invalida quando tools/MCP/plugins mudam.

### Comparacao obrigatoria

- Codex: estudar `spec_plan_tests.rs`, especialmente direct/deferred MCP, extension tools e `ToolExposure::Deferred`.
- Claude: estudar `ToolSearchTool`, tool presets e MCP dynamic loading nos docs locais.

### Entregas

- O prompt principal nao inclui schemas de tools deferidas.
- `tool.search` retorna schemas completos apenas para matches relevantes.
- Tests de ranking, filtros por risco/capability e invalidacao de cache.

### Definition of Done

- `just test-crate navi-core`
- `just test-crate navi-sdk`
- Um teste prova que uma tool deferred esta registrada, nao esta visivel, aparece em `tool.search` e pode ser chamada depois.

### Objetivo para o proximo sprint

Completar o toolset minimo de repo/file/execution com nomes e comportamentos consistentes.

## Sprint P3 - Toolset base de repo e arquivos

### Objetivo do sprint

Garantir que o Navi tenha o conjunto minimo de ferramentas de arquivo e repositorio que Claude/Codex esperam em tarefas reais.

### Epics

- Consolidar nomes canonicos e aliases:
  - `read_file`
  - `write_file`
  - `edit_file`
  - `apply_patch`
  - `grep`
  - `glob`
  - `list_dir`
  - `inspect_image`
  - `git.status/diff/log`
- Preservar nomes existentes como aliases quando necessario.
- Definir schemas orientados a agente, com exemplos, limites, erro acionavel e idempotencia.
- Separar full write de edit/patch.
- Garantir output budget por tool, nao apenas global.
- Adicionar remediation hints padronizados para erro de schema, path, lock, command e output truncado.

### Comparacao obrigatoria

- Claude: `docs/tools.md` file system tools e UI/rendering expectations.
- Codex: apply patch specs, unified exec visibility e tool formatting.

### Entregas

- Catalogo de tools base documentado no proprio metadata.
- Aliases compativeis.
- Tests para read, write, edit, patch, grep/glob/list e image.

### Definition of Done

- `just test-crate navi-core`
- Nenhuma tool de escrita executa sem `supports_rollback` explicitamente definido como true/false.
- Erros de tool retornam `error_code`, `message`, `hint` e `retryable`.

### Objetivo para o proximo sprint

Construir execucao persistente controlada sem transformar o Navi em shell livre.

## Sprint P4 - Unified exec e code mode MVP

### Objetivo do sprint

Oferecer execucao persistente e observavel para comandos e codigo, inspirada no Code Mode do Codex, mas sob policy do Navi.

### Epics

- Evoluir `bash` para uma familia `process`:
  - `process.exec`
  - `process.stdin`
  - `process.wait`
  - `process.list`
  - `process.cancel`
- Manter aliases para `bash` existente.
- Criar `code.exec` MVP com sessao persistente por Navi session.
- Permitir nested tool calls apenas por SDK tipado e allowlist, nao por shell arbitrario.
- Adicionar quotas:
  - wall time
  - idle time
  - stdout/stderr bytes
  - file writes
  - nested tool calls
- Emitir progress events.

### Comparacao obrigatoria

- Codex: `/home/enrell/lab/codex/codex-rs/code-mode/src/service.rs` e tests de `execute_to_pending`.
- Codex: `unified_exec/process_manager.rs` para lifecycle e network approval.
- Claude: `BashTool`, `REPLTool` e concurrency declarations.

### Entregas

- Process manager com sessoes persistentes.
- `code.exec` com estado entre cells e encerramento limpo.
- Tests de timeout, cancel, wait, stdin e sessao isolada.

### Definition of Done

- `just test-crate navi-core`
- Um comando long-running pode ser iniciado, observado, receber stdin e ser cancelado.
- `code.exec` nao acessa tools nested sem capability explicita.

### Objetivo para o proximo sprint

Prender toda execucao mutavel a sandbox, snapshot e rollback.

## Sprint P5 - Sandbox, snapshot e rollback MVP

### Objetivo do sprint

Adicionar uma camada de execucao segura para writes e commands com snapshot antes, efeitos depois e rollback quando necessario.

### Epics

- Criar `WorkspaceSnapshot` antes de write/command de risco.
- Criar `ChangeSet` por tool:
  - arquivos criados
  - arquivos modificados
  - arquivos deletados
  - diff
  - comandos executados
- Implementar rollback por reverse patch para casos simples.
- Proteger staged changes existentes: nunca sobrescrever, misturar ou reverter sem consentimento explicito.
- Adicionar `sandbox.reset`, `sandbox.snapshot`, `sandbox.rollback`.
- Definir estrategia de sandbox por plataforma:
  - MVP: workspace snapshot + command cwd restriction.
  - Depois: process sandbox OS-specific.

### Comparacao obrigatoria

- Codex: `tools/sandboxing.rs`, `unified_exec/`, Windows sandbox e approval cache.
- Claude: `EnterWorktreeTool`/`ExitWorktreeTool` nos docs como isolamento de trabalho.

### Entregas

- Snapshot antes de tool write.
- Rollback manual via tool.
- Rollback automatico configuravel quando verifier falha.
- Eventos `snapshot.created`, `rollback.started`, `rollback.completed`.

### Definition of Done

- `just test-crate navi-core`
- Tests cobrem arquivo criado, modificado, deletado e staged change protegido.
- Rollback falho e reportado como falha segura, nao como sucesso silencioso.

### Objetivo para o proximo sprint

Fazer a policy avaliar efeitos reais, nao apenas nome da tool ou comando.

## Sprint P6 - Effect-based permissions v1

### Objetivo do sprint

Elevar approvals de "tool/command based" para "effect based" em writes e commands.

### Epics

- Criar `EffectReport` depois da execucao:
  - arquivos criados/modificados/deletados
  - lockfile alterado
  - CI alterado
  - scripts de build/test alterados
  - auth/security paths tocados
  - secrets-like files tocados
  - dependency manifests alterados
  - executaveis/binarios adicionados
  - network acionada
- Policy decide `allow`, `ask`, `deny`, `rollback`, `escalate`.
- Approval prompt deve mostrar efeito, intencao e blast radius.
- Guarded effects continuam exigindo aprovacao mesmo em modo auto/yolo.
- Registrar decisions no trace.

### Comparacao obrigatoria

- Navi: `SecurityPolicy::validate_tool_invocation` atual.
- Codex: `ApprovalStore`, `ExecApprovalRequirement`, network approval.
- Claude: permission modes e wildcard rules.

### Entregas

- Pre-policy continua existindo.
- Post-policy roda depois de writes/commands.
- Sensitive effects exigem approval ou rollback.

### Definition of Done

- `just test-crate navi-core`
- Tests para lockfile, CI, `.env`, auth path, dependency add e deleted file.
- Approval event contem effect summary redigido.

### Objetivo para o proximo sprint

Integrar verificacao como parte normal do loop, nao como recomendacao final.

## Sprint P7 - Verifier API e verifier-first loop v1

### Objetivo do sprint

Fazer o harness rodar verificadores estruturados depois de passos relevantes e usar o resultado para continuar, retry, rollback ou perguntar.

### Epics

- Criar `VerifierSpec` em tool metadata e plan/features.
- Criar `VerifierResult`:
  - `status`
  - `command`
  - `duration`
  - `stdout/stderr` truncados
  - `files_related`
  - `error_class`
  - `suggested_next_action`
- Verifiers MVP:
  - `verify.build`
  - `verify.test`
  - `verify.typecheck`
  - `verify.lint`
  - `verify.command`
- Integrar `mark_feature_done` para aceitar somente verificacao executada.
- Emitir runtime events para verifier started/completed.

### Comparacao obrigatoria

- Navi: `mark_feature_done`, `test_runner`, `build_runner`, `tool_workflow`.
- Codex: tool dispatch trace e exec output formatting.
- Claude: planner/generator/evaluator docs locais e task/todo tools.

### Entregas

- API no `navi-sdk` para rodar verifier.
- Loop `tool -> effect -> verifier -> policy -> model`.
- TUI mostra verifier em compact/full tool output.

### Definition of Done

- `just test-crate navi-core`
- `just test-crate navi-sdk`
- Uma task de write so pode ser marcada done com verifier pass ou skip justificado.

### Objetivo para o proximo sprint

Persistir cada decisao do harness em traces reutilizaveis.

## Sprint P8 - Trace Store e metricas de harness

### Objetivo do sprint

Transformar sessoes em traces estruturados para depuracao, replay futuro e aprendizado.

### Epics

- Criar `TraceStore` separado de `SessionStore`.
- Persistir:
  - task
  - repo state summary
  - model/provider
  - visible tools
  - deferred tools discovered
  - tool calls
  - effects
  - approvals
  - verifier results
  - retries
  - rollbacks
  - token/cost/time
  - outcome
- Export JSONL.
- Redaction obrigatoria reaproveitando `security.rs`.
- Criar metricas:
  - `verified_success_rate`
  - `tokens_per_success`
  - `tool_calls_per_success`
  - `approval_prompts_per_task`
  - `rollback_success_rate`
  - `unsafe_action_block_rate`

### Comparacao obrigatoria

- Codex: `tool_dispatch_trace.rs` e tests.
- Navi: `SessionStore`, `AgentEvent`, `HarnessTrace`.
- Claude: cost tracker e tool duration nos arquivos locais.

### Entregas

- Trace por turn salvo em data dir.
- CLI/headless consegue exportar traces.
- Debug modal ou runtime event expõe metricas basicas.

### Definition of Done

- `just test-crate navi-core`
- Trace redige secrets em inputs/outputs/diffs.
- Tests garantem compatibilidade de schema versionado.

### Objetivo para o proximo sprint

Fazer MCP entrar no mesmo sistema de registry, discovery, policy e trace.

## Sprint P9 - MCP client parity e hardening inicial

### Objetivo do sprint

Integrar MCP tools como tools de primeira classe, com exposure, search, policy, redaction e auditoria.

### Epics

- Normalizar MCP tool metadata para `ToolMetadata`.
- Permitir MCP direct ou deferred.
- Validar schemas invalidos e nao registrar tool quebrada.
- Allowlist por workspace.
- Output budget e redaction por MCP tool.
- Registrar server id, tool id e version/hash quando disponivel.
- Expor `list_mcp_servers`, `list_mcp_tools` e status em eventos/SDK.

### Comparacao obrigatoria

- Navi: `crates/navi-mcp/src/lib.rs`.
- Codex: `spec_plan_tests.rs` para deferred MCP e invalid MCP tools.
- Claude: `docs/subsystems.md` MCP client e `ToolSearchTool`.

### Entregas

- MCP tools aparecem em `tool.search`.
- MCP tool calls passam por approval/policy.
- MCP outputs entram em trace com origem marcada.

### Definition of Done

- `just test-crate navi-mcp`
- `just test-crate navi-core`
- Tests para MCP schema invalido, deferred MCP e output truncation.

### Objetivo para o proximo sprint

Completar orchestration minima com subagentes seguros e batch jobs.

## Sprint P10 - Subagentes e batch jobs de paridade

### Objetivo do sprint

Tornar subagentes usaveis em tarefas reais sem perder controle de contexto, approvals e custo.

### Epics

- Adicionar profiles de subagente:
  - `explorer`
  - `implementer`
  - `reviewer`
  - `verifier`
  - `summarizer`
- Permitir model override por profile.
- Permitir toolset por profile.
- Aprovals de subagente:
  - `inherit`
  - `escalate`
  - `readonly`
  - `deny_write`
- `subagent.spawn`, `subagent.wait`, `subagent.message`.
- `batch.spawn_on_items` para executar a mesma tarefa em N arquivos/itens.
- Trace hierarquico parent/child.

### Comparacao obrigatoria

- Navi: `SubagentTool` atual.
- Codex: multi-agent tests em `spec_plan_tests.rs` e doctor thread inventory.
- Claude: AgentTool, SendMessageTool, Team tools e Task tools nos docs.

### Entregas

- Subagente read-only real.
- Reviewer subagent com verifier obrigatorio.
- Batch jobs com limite de concorrencia e cancelamento.

### Definition of Done

- `just test-crate navi-core`
- Tests para approval inheritance, model override, toolset restriction e trace parent/child.

### Objetivo para o proximo sprint

Fechar paridade com uma suite de tarefas reais e metricas comparaveis.

## Sprint P11 - Parity Gate

### Objetivo do sprint

Provar que o Navi atingiu paridade operacional em tarefas de repositorio comuns.

### Epics

- Criar suite local de tarefas:
  - bugfix pequeno
  - type error
  - teste quebrado
  - refactor multi-arquivo
  - dependencia/config change
  - doc update validado
  - MCP read-only
  - subagent reviewer
- Para cada tarefa, capturar:
  - trace
  - verifier result
  - diff
  - approvals
  - rollbacks
  - tokens
  - wall time
- Comparar com uma baseline linear do Navi antigo quando possivel.

### Entregas

- `docs/harness-parity-report.md` ou artefato gerado equivalente.
- Thresholds minimos:
  - `verified_success_rate >= 0.80` na suite local inicial.
  - `rollback_success_rate >= 0.95` em casos reversiveis.
  - `secret_exposure_events = 0`.
  - `unsafe_guarded_effects_auto_approved = 0`.

### Definition of Done

- `just verify` se as mudancas tocaram runtime compartilhado.
- Se o escopo final for apenas um crate, usar o menor gate justificado conforme `AGENTS.md`.
- Relatorio lista gaps remanescentes e decide o que sobe para o plano beyond parity.

### Objetivo para o proximo plano

Com a fundacao estabilizada, iniciar o plano de superioridade: AST/code graph, branch racing, capability ledger, memoria procedural, skill mining, trace-to-eval e multi-model routing.

## Ordem de execucao resumida

1. P0 - baseline comparativo.
2. P1 - tool metadata v2.
3. P2 - registry/exposure/tool.search.
4. P3 - toolset base de repo/file.
5. P4 - unified exec/code mode MVP.
6. P5 - sandbox/snapshot/rollback.
7. P6 - effect-based permissions.
8. P7 - verifier API.
9. P8 - trace store.
10. P9 - MCP hardening.
11. P10 - subagentes/batch.
12. P11 - parity gate.

## Metricas de paridade

| Metrica | Uso |
|---|---|
| `verified_success_rate` | Sucesso confirmado por verifier |
| `tokens_per_success` | Eficiencia de contexto/modelo |
| `tool_calls_per_success` | Qualidade de routing |
| `wall_time_per_success` | Latencia real |
| `approval_prompts_per_task` | Friccao de seguranca |
| `unsafe_action_block_rate` | Efetividade da policy |
| `false_approval_rate` | Risco do auto/approval |
| `rollback_success_rate` | Confiabilidade de reversao |
| `trace_coverage_rate` | Percentual de tasks com trace completo |

## Criterio final de paridade

O Navi atinge paridade quando uma task comum de repositorio segue este fluxo sem hacks de TUI:

```text
task
-> phase-scoped tools
-> deferred tool discovery when needed
-> tool execution through policy
-> snapshot/effect capture
-> verifier
-> rollback/retry/escalation if needed
-> trace persisted
-> final answer with verified status
```

